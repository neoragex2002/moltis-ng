//! Sub-agent tool: lets the LLM delegate tasks to a child agent loop.

use std::sync::Arc;

use {anyhow::Result, async_trait::async_trait, tracing::info};

use moltis_agents::{
    model::{ChatMessage, LlmProvider, UserContent},
    prompt::{
        build_openai_responses_developer_prompts, build_system_prompt_minimal_runtime,
        build_system_prompt_with_session_runtime,
    },
    providers::ProviderRegistry,
    runner::{RunnerEvent, run_agent_loop_with_context, run_agent_loop_with_context_prefix},
    tool_registry::{AgentTool, ToolRegistry},
};

/// Maximum nesting depth for sub-agents (prevents infinite recursion).
const MAX_SPAWN_DEPTH: u64 = 3;

/// Tool parameter injected via `tool_context` to track nesting depth.
const SPAWN_DEPTH_KEY: &str = "_spawn_depth";

/// A tool that spawns a sub-agent running its own agent loop.
///
/// The sub-agent executes synchronously (blocks until done) and its result
/// is returned as the tool output. Sub-agents get a filtered copy of the
/// parent's tool registry (without the `spawn_agent` tool itself) and a
/// focused system prompt.
/// Callback for emitting events from the sub-agent back to the parent UI.
pub type OnSpawnEvent = Arc<dyn Fn(RunnerEvent) + Send + Sync>;

struct LoadedPersona {
    identity: moltis_config::AgentIdentity,
    user: moltis_config::UserProfile,
    identity_md_raw: Option<String>,
    soul_text: Option<String>,
    agents_text: Option<String>,
    tools_text: Option<String>,
}

fn load_persona(persona_id: Option<&str>) -> LoadedPersona {
    let config = moltis_config::discover_and_load();

    let mut identity = config.identity.clone();
    let file_identity = persona_id
        .and_then(moltis_config::load_persona_identity)
        .or_else(moltis_config::load_identity);
    if let Some(file_identity) = file_identity {
        if file_identity.name.is_some() {
            identity.name = file_identity.name;
        }
        if file_identity.emoji.is_some() {
            identity.emoji = file_identity.emoji;
        }
        if file_identity.creature.is_some() {
            identity.creature = file_identity.creature;
        }
        if file_identity.vibe.is_some() {
            identity.vibe = file_identity.vibe;
        }
    }

    let mut user = config.user.clone();
    if let Some(file_user) = moltis_config::load_user() {
        if file_user.name.is_some() {
            user.name = file_user.name;
        }
        if file_user.timezone.is_some() {
            user.timezone = file_user.timezone;
        }
    }

    LoadedPersona {
        identity,
        identity_md_raw: persona_id
            .and_then(moltis_config::load_persona_identity_md_raw)
            .or_else(moltis_config::load_identity_md_raw),
        user,
        soul_text: persona_id
            .and_then(moltis_config::load_persona_soul)
            .or_else(moltis_config::load_soul),
        agents_text: persona_id
            .and_then(moltis_config::load_persona_agents_md)
            .or_else(moltis_config::load_agents_md),
        tools_text: persona_id
            .and_then(moltis_config::load_persona_tools_md)
            .or_else(moltis_config::load_tools_md),
    }
}

pub struct SpawnAgentTool {
    provider_registry: Arc<tokio::sync::RwLock<ProviderRegistry>>,
    default_provider: Arc<dyn LlmProvider>,
    tool_registry: Arc<ToolRegistry>,
    on_event: Option<OnSpawnEvent>,
}

impl SpawnAgentTool {
    pub fn new(
        provider_registry: Arc<tokio::sync::RwLock<ProviderRegistry>>,
        default_provider: Arc<dyn LlmProvider>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            provider_registry,
            default_provider,
            tool_registry,
            on_event: None,
        }
    }

    /// Set an event callback so sub-agent activity is visible to the UI.
    pub fn with_on_event(mut self, on_event: OnSpawnEvent) -> Self {
        self.on_event = Some(on_event);
        self
    }

    fn emit(&self, event: RunnerEvent) {
        if let Some(ref cb) = self.on_event {
            cb(event);
        }
    }
}

#[async_trait]
impl AgentTool for SpawnAgentTool {
    fn name(&self) -> &str {
        "spawn_agent"
    }

    fn description(&self) -> &str {
        "Spawn a sub-agent to handle a complex, multi-step task autonomously. \
         The sub-agent runs its own agent loop with access to tools and returns \
         the result when done. Use this to delegate tasks that require multiple \
         tool calls or independent reasoning."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task to delegate to the sub-agent"
                },
                "context": {
                    "type": "string",
                    "description": "Additional context for the sub-agent (optional)"
                },
                "model": {
                    "type": "string",
                    "description": "Model ID to use (e.g. a cheaper model). If not specified, uses the parent's current model."
                },
                "persona_id": {
                    "type": "string",
                    "description": "Persona ID to use for the sub-agent. Defaults to the system default persona; does not implicitly inherit from the parent session."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        let task = params["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: task"))?;
        let context = params["context"].as_str().unwrap_or("");
        let model_id = params["model"].as_str();
        let persona_id = params.get("persona_id").and_then(|v| v.as_str());
        let persona_id = match persona_id.map(str::trim) {
            Some("") | None => None,
            Some("default") => None,
            Some(other) => Some(other),
        };

        // Check nesting depth.
        let depth = params
            .get(SPAWN_DEPTH_KEY)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if depth >= MAX_SPAWN_DEPTH {
            anyhow::bail!("maximum sub-agent nesting depth ({MAX_SPAWN_DEPTH}) exceeded");
        }

        // Resolve provider.
        let provider = if let Some(id) = model_id {
            let reg = self.provider_registry.read().await;
            reg.get(id)
                .ok_or_else(|| anyhow::anyhow!("unknown model: {id}"))?
        } else {
            Arc::clone(&self.default_provider)
        };

        // Capture model ID before provider is moved into the sub-agent loop.
        let model_id = provider.id().to_string();

        info!(
            task = %task,
            depth = depth,
            model = %model_id,
            "spawning sub-agent"
        );

        self.emit(RunnerEvent::SubAgentStart {
            task: task.to_string(),
            model: model_id.clone(),
            depth,
        });

        // Build filtered tool registry (no spawn_agent to prevent recursive spawning).
        let sub_tools = self.tool_registry.clone_without(&["spawn_agent"]);

        let persona = load_persona(persona_id);

        // Build tool context with incremented depth and propagated session key.
        let mut tool_context = serde_json::json!({
            SPAWN_DEPTH_KEY: depth + 1,
        });
        if let Some(session_key) = params.get("_session_key") {
            tool_context["_session_key"] = session_key.clone();
        }
        if let Some(session_id) = params.get("_session_id") {
            tool_context["_session_id"] = session_id.clone();
        }

        // Run the sub-agent loop (no event forwarding, no hooks, no history).
        let user_text = if context.trim().is_empty() {
            task.to_string()
        } else {
            format!("{task}\n\nContext:\n{context}")
        };
        let user_content = UserContent::text(user_text);

        let is_openai_responses = provider.name().trim().eq_ignore_ascii_case("openai-responses");
        let result = if is_openai_responses {
            let include_tools = !sub_tools.list_schemas().is_empty();
            let persona_label = persona_id.unwrap_or("default");
            let prompts = build_openai_responses_developer_prompts(
                &sub_tools,
                provider.supports_tools(),
                None,
                &[],
                persona_label,
                include_tools,
                Some(&persona.identity),
                Some(&persona.user),
                persona.identity_md_raw.as_deref(),
                persona.soul_text.as_deref(),
                persona.agents_text.as_deref(),
                persona.tools_text.as_deref(),
                None,
            );
            let mut runtime_snapshot = prompts.runtime_snapshot;
            runtime_snapshot.push_str("\n## Sub-agent\n\nYou are a sub-agent spawned to complete the user's task thoroughly and return a clear result.\n");
            let prefix_messages = vec![
                ChatMessage::system(prompts.system),
                ChatMessage::system(prompts.persona),
                ChatMessage::system(runtime_snapshot),
            ];

            run_agent_loop_with_context_prefix(
                provider,
                &sub_tools,
                prefix_messages,
                &user_content,
                None,
                None,
                Some(tool_context),
                None,
            )
            .await
        } else {
            let native_tools = provider.supports_tools();
            let system_prompt = if native_tools {
                build_system_prompt_with_session_runtime(
                    &sub_tools,
                    native_tools,
                    None,
                    &[],
                    Some(&persona.identity),
                    Some(&persona.user),
                    persona.soul_text.as_deref(),
                    persona.agents_text.as_deref(),
                    persona.tools_text.as_deref(),
                    None,
                )
            } else {
                build_system_prompt_minimal_runtime(
                    None,
                    Some(&persona.identity),
                    Some(&persona.user),
                    persona.soul_text.as_deref(),
                    persona.agents_text.as_deref(),
                    persona.tools_text.as_deref(),
                    None,
                )
            };

            run_agent_loop_with_context(
                provider,
                &sub_tools,
                &system_prompt,
                &user_content,
                None,
                None, // no history
                Some(tool_context),
                None, // no hooks for sub-agents
            )
            .await
        };

        // Emit SubAgentEnd regardless of success/failure.
        let (iterations, tool_calls_made) = match &result {
            Ok(r) => (r.iterations, r.tool_calls_made),
            Err(_) => (0, 0),
        };
        self.emit(RunnerEvent::SubAgentEnd {
            task: task.to_string(),
            model: model_id.clone(),
            depth,
            iterations,
            tool_calls_made,
        });

        let result = result?;

        info!(
            task = %task,
            depth = depth,
            iterations = result.iterations,
            tool_calls = result.tool_calls_made,
            "sub-agent completed"
        );

        Ok(serde_json::json!({
            "text": result.text,
            "iterations": result.iterations,
            "tool_calls_made": result.tool_calls_made,
            "model": model_id,
        }))
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        moltis_agents::model::{ChatMessage, CompletionResponse, StreamEvent, Usage},
        std::pin::Pin,
        tokio_stream::Stream,
    };

    /// Mock provider that returns a fixed response.
    struct MockProvider {
        response: String,
        model_id: String,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn id(&self) -> &str {
            &self.model_id
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> Result<CompletionResponse> {
            Ok(CompletionResponse {
                text: Some(self.response.clone()),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            })
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::empty())
        }
    }

    fn make_empty_provider_registry() -> Arc<tokio::sync::RwLock<ProviderRegistry>> {
        Arc::new(tokio::sync::RwLock::new(
            ProviderRegistry::from_env_with_config(&Default::default()),
        ))
    }

    #[tokio::test]
    async fn test_sub_agent_runs_and_returns_result() {
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            response: "Sub-agent result".into(),
            model_id: "mock-model".into(),
        });
        let tool_registry = Arc::new(ToolRegistry::new());
        let spawn_tool = SpawnAgentTool::new(
            make_empty_provider_registry(),
            Arc::clone(&provider),
            tool_registry,
        );

        let params = serde_json::json!({ "task": "do something" });
        let result = spawn_tool.execute(params).await.unwrap();

        assert_eq!(result["text"], "Sub-agent result");
        assert_eq!(result["iterations"], 1);
        assert_eq!(result["tool_calls_made"], 0);
        assert_eq!(result["model"], "mock-model");
    }

    #[tokio::test]
    async fn test_depth_limit_rejects() {
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            response: "nope".into(),
            model_id: "mock".into(),
        });
        let tool_registry = Arc::new(ToolRegistry::new());
        let spawn_tool =
            SpawnAgentTool::new(make_empty_provider_registry(), provider, tool_registry);

        let params = serde_json::json!({
            "task": "do something",
            "_spawn_depth": MAX_SPAWN_DEPTH,
        });
        let result = spawn_tool.execute(params).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nesting depth"));
    }

    #[tokio::test]
    async fn test_spawn_agent_excluded_from_sub_registry() {
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            response: "ok".into(),
            model_id: "mock".into(),
        });

        // Create a registry with spawn_agent in it.
        let mut registry = ToolRegistry::new();

        struct DummyTool;
        #[async_trait]
        impl AgentTool for DummyTool {
            fn name(&self) -> &str {
                "spawn_agent"
            }

            fn description(&self) -> &str {
                "dummy"
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }

            async fn execute(&self, _: serde_json::Value) -> Result<serde_json::Value> {
                Ok(serde_json::json!("dummy"))
            }
        }

        struct EchoTool;
        #[async_trait]
        impl AgentTool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }

            fn description(&self) -> &str {
                "echo"
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }

            async fn execute(&self, p: serde_json::Value) -> Result<serde_json::Value> {
                Ok(p)
            }
        }

        registry.register(Box::new(DummyTool));
        registry.register(Box::new(EchoTool));

        let filtered = registry.clone_without(&["spawn_agent"]);
        assert!(filtered.get("spawn_agent").is_none());
        assert!(filtered.get("echo").is_some());

        // Also verify schemas don't include spawn_agent.
        let schemas = filtered.list_schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "echo");

        // Ensure original is unaffected.
        assert!(registry.get("spawn_agent").is_some());

        // The SpawnAgentTool itself should work with the filtered registry.
        let spawn_tool =
            SpawnAgentTool::new(make_empty_provider_registry(), provider, Arc::new(registry));
        let result = spawn_tool
            .execute(serde_json::json!({ "task": "test" }))
            .await
            .unwrap();
        assert_eq!(result["text"], "ok");
    }

    #[tokio::test]
    async fn test_context_passed_to_sub_agent() {
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            response: "done with context".into(),
            model_id: "mock".into(),
        });
        let spawn_tool = SpawnAgentTool::new(
            make_empty_provider_registry(),
            provider,
            Arc::new(ToolRegistry::new()),
        );

        let params = serde_json::json!({
            "task": "analyze code",
            "context": "The code is in src/main.rs",
        });
        let result = spawn_tool.execute(params).await.unwrap();
        assert_eq!(result["text"], "done with context");
    }

    #[tokio::test]
    async fn test_missing_task_parameter() {
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            response: "nope".into(),
            model_id: "mock".into(),
        });
        let spawn_tool = SpawnAgentTool::new(
            make_empty_provider_registry(),
            provider,
            Arc::new(ToolRegistry::new()),
        );

        let result = spawn_tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("task"));
    }
}

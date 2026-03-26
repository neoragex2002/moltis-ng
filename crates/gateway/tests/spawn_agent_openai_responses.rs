use std::{pin::Pin, sync::Arc};

use async_trait::async_trait;
use moltis_agents::{
    model::{ChatMessage, CompletionResponse, LlmProvider, StreamEvent, Usage},
    providers::ProviderRegistry,
    tool_registry::AgentTool,
    tool_registry::ToolRegistry,
};
use moltis_tools::spawn_agent::SpawnAgentTool;
use tokio_stream::Stream;

struct CapturingProvider {
    captured: Arc<tokio::sync::Mutex<Vec<ChatMessage>>>,
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    fn name(&self) -> &str {
        "openai-responses"
    }

    fn id(&self) -> &str {
        "mock-model"
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        *self.captured.lock().await = messages.to_vec();
        Ok(CompletionResponse {
            text: Some("ok".into()),
            tool_calls: vec![],
            usage: Usage::default(),
        })
    }

    fn stream(
        &self,
        _messages: Vec<ChatMessage>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        Box::pin(tokio_stream::empty())
    }
}

#[tokio::test]
async fn spawn_agent_uses_single_system_message_for_openai_responses_provider() {
    let captured: Arc<tokio::sync::Mutex<Vec<ChatMessage>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let provider: Arc<dyn LlmProvider> = Arc::new(CapturingProvider {
        captured: Arc::clone(&captured),
    });
    let provider_registry: Arc<tokio::sync::RwLock<ProviderRegistry>> = Arc::new(
        tokio::sync::RwLock::new(ProviderRegistry::from_env_with_config(&Default::default())),
    );
    let tool_registry = Arc::new(ToolRegistry::new());
    let tool = SpawnAgentTool::new(provider_registry, provider, tool_registry);

    let params = serde_json::json!({ "task": "do something", "_sessionId": "test" });
    let _ = tool.execute(params).await.unwrap();

    let msgs = captured.lock().await;
    assert!(
        msgs.len() >= 2,
        "expected system + user task, got {}",
        msgs.len()
    );
    assert!(matches!(msgs[0], ChatMessage::System { .. }));
    assert!(matches!(msgs[1], ChatMessage::User { .. }));
}

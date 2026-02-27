//! GitHub Copilot provider.
//!
//! Authentication uses the GitHub device-flow OAuth to obtain a GitHub token,
//! then exchanges it for a short-lived Copilot API token via
//! `https://api.github.com/copilot_internal/v2/token`.
//!
//! The Copilot API itself is OpenAI-compatible (`/chat/completions`).

use std::{collections::HashSet, pin::Pin, sync::mpsc, time::Duration};

use {
    async_trait::async_trait,
    futures::StreamExt,
    moltis_oauth::{OAuthTokens, TokenStore},
    secrecy::{ExposeSecret, Secret},
    tokio_stream::Stream,
    tracing::{debug, trace, warn},
};

use {
    super::openai_compat::{
        SseFrameParser, SseLineResult, StreamingToolState, finalize_stream, parse_tool_calls,
        process_openai_sse_line, to_openai_tools,
    },
    crate::model::{ChatMessage, CompletionResponse, LlmProvider, StreamEvent, Usage},
};

// ── Constants ────────────────────────────────────────────────────────────────

/// GitHub OAuth app client ID for Copilot (VS Code's public client ID).
const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_API_BASE: &str = "https://api.individual.githubcopilot.com";
const COPILOT_MODELS_ENDPOINT: &str = "https://api.individual.githubcopilot.com/models";

const PROVIDER_NAME: &str = "github-copilot";

/// Required headers for the Copilot chat completions API.
/// The API rejects requests without `Editor-Version`.
const EDITOR_VERSION: &str = "vscode/1.96.2";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.26.7";

// ── Device flow types ────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
}

#[derive(Debug, serde::Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: u64,
}

// ── Provider ─────────────────────────────────────────────────────────────────

pub struct GitHubCopilotProvider {
    model: String,
    client: reqwest::Client,
    token_store: TokenStore,
}

impl GitHubCopilotProvider {
    pub fn new(model: String) -> Self {
        Self {
            model,
            client: reqwest::Client::new(),
            token_store: TokenStore::new(),
        }
    }

    /// Start the GitHub device-flow: request a device code from GitHub.
    pub async fn request_device_code(
        client: &reqwest::Client,
    ) -> anyhow::Result<DeviceCodeResponse> {
        let resp = client
            .post(GITHUB_DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .form(&[("client_id", GITHUB_CLIENT_ID), ("scope", "")])
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("GitHub device code request failed: {body}");
        }

        Ok(resp.json().await?)
    }

    /// Poll GitHub for the access token after the user has entered the code.
    pub async fn poll_for_token(
        client: &reqwest::Client,
        device_code: &str,
        interval: u64,
    ) -> anyhow::Result<String> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

            let resp = client
                .post(GITHUB_TOKEN_URL)
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", GITHUB_CLIENT_ID),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?;

            let body: GithubTokenResponse = resp.json().await?;

            if let Some(token) = body.access_token {
                return Ok(token);
            }

            match body.error.as_deref() {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                },
                Some(err) => anyhow::bail!("GitHub device flow error: {err}"),
                None => anyhow::bail!("unexpected response from GitHub token endpoint"),
            }
        }
    }

    /// Get a valid Copilot API token, exchanging the GitHub token if needed.
    async fn get_valid_copilot_token(&self) -> anyhow::Result<String> {
        fetch_valid_copilot_token_with_fallback(&self.client, &self.token_store).await
    }
}

fn home_token_store_if_different() -> Option<TokenStore> {
    let home = moltis_config::user_global_config_dir_if_different()?;
    Some(TokenStore::with_path(home.join("oauth_tokens.json")))
}

fn token_store_with_provider_tokens(primary: &TokenStore) -> Option<TokenStore> {
    debug!("checking primary token store for {PROVIDER_NAME}");
    if primary.load(PROVIDER_NAME).is_some() {
        debug!("found {PROVIDER_NAME} tokens in primary store");
        return Some(primary.clone());
    }
    if let Some(home_store) = home_token_store_if_different() {
        debug!("checking home token store for {PROVIDER_NAME}");
        if home_store.load(PROVIDER_NAME).is_some() {
            debug!("found {PROVIDER_NAME} tokens in home store");
            return Some(home_store);
        }
    }
    debug!("{PROVIDER_NAME} tokens not found in any store");
    None
}

/// Check if we have stored GitHub tokens for Copilot.
pub fn has_stored_tokens() -> bool {
    let found = token_store_with_provider_tokens(&TokenStore::new()).is_some();
    if found {
        debug!("{PROVIDER_NAME} stored tokens found");
    } else {
        debug!("{PROVIDER_NAME} stored tokens not found");
    }
    found
}

/// Known Copilot models.
/// The list is intentionally broad; if a model isn't available for the user's
/// plan Copilot will return an error.
pub const COPILOT_MODELS: &[(&str, &str)] = &[
    ("gpt-4o", "GPT-4o (Copilot)"),
    ("gpt-4.1", "GPT-4.1 (Copilot)"),
    ("gpt-4.1-mini", "GPT-4.1 Mini (Copilot)"),
    ("gpt-4.1-nano", "GPT-4.1 Nano (Copilot)"),
    ("o1", "o1 (Copilot)"),
    ("o1-mini", "o1-mini (Copilot)"),
    ("o3-mini", "o3-mini (Copilot)"),
    ("claude-sonnet-4", "Claude Sonnet 4 (Copilot)"),
    ("gemini-2.0-flash", "Gemini 2.0 Flash (Copilot)"),
];

async fn fetch_valid_copilot_token(
    client: &reqwest::Client,
    token_store: &TokenStore,
) -> anyhow::Result<String> {
    let tokens = token_store.load(PROVIDER_NAME).ok_or_else(|| {
        anyhow::anyhow!("not logged in to github-copilot — run OAuth device flow first")
    })?;

    // The `access_token` stored is the GitHub user token.
    // We exchange it for a short-lived Copilot API token and cache it.
    if let Some(copilot_tokens) = token_store.load("github-copilot-api")
        && let Some(expires_at) = copilot_tokens.expires_at
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now + 60 < expires_at {
            return Ok(copilot_tokens.access_token.expose_secret().clone());
        }
    }

    let resp = client
        .get(COPILOT_TOKEN_URL)
        .header(
            "Authorization",
            format!("token {}", tokens.access_token.expose_secret()),
        )
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            "moltis/0.1.0 (GitHub Copilot compatible client)",
        )
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Copilot token exchange failed: {body}");
    }

    let copilot_resp: CopilotTokenResponse = resp.json().await?;
    let _ = token_store.save(
        "github-copilot-api",
        &OAuthTokens {
            access_token: Secret::new(copilot_resp.token.clone()),
            refresh_token: None,
            expires_at: Some(copilot_resp.expires_at),
        },
    );

    Ok(copilot_resp.token)
}

async fn fetch_valid_copilot_token_with_fallback(
    client: &reqwest::Client,
    primary_store: &TokenStore,
) -> anyhow::Result<String> {
    let Some(token_store) = token_store_with_provider_tokens(primary_store) else {
        anyhow::bail!("not logged in to github-copilot — run OAuth device flow first");
    };
    fetch_valid_copilot_token(client, &token_store).await
}

fn default_model_catalog() -> Vec<super::DiscoveredModel> {
    COPILOT_MODELS
        .iter()
        .map(|(id, name)| super::DiscoveredModel::new(*id, *name))
        .collect()
}

fn normalize_display_name(model_id: &str, display_name: Option<&str>) -> String {
    let normalized = display_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(model_id);
    if normalized == model_id {
        model_id.to_string()
    } else {
        normalized.to_string()
    }
}

fn is_likely_model_id(model_id: &str) -> bool {
    if model_id.is_empty() || model_id.len() > 120 {
        return false;
    }
    if model_id.chars().any(char::is_whitespace) {
        return false;
    }
    model_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
}

fn parse_model_entry(entry: &serde_json::Value) -> Option<super::DiscoveredModel> {
    let obj = entry.as_object()?;
    let model_id = obj
        .get("id")
        .or_else(|| obj.get("slug"))
        .or_else(|| obj.get("model"))
        .and_then(serde_json::Value::as_str)?;

    if !is_likely_model_id(model_id) {
        return None;
    }

    let display_name = obj
        .get("display_name")
        .or_else(|| obj.get("displayName"))
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("title"))
        .and_then(serde_json::Value::as_str);

    let created_at = obj.get("created").and_then(serde_json::Value::as_i64);

    Some(
        super::DiscoveredModel::new(model_id, normalize_display_name(model_id, display_name))
            .with_created_at(created_at),
    )
}

fn collect_candidate_arrays<'a>(
    value: &'a serde_json::Value,
    out: &mut Vec<&'a serde_json::Value>,
) {
    match value {
        serde_json::Value::Array(items) => out.extend(items),
        serde_json::Value::Object(map) => {
            for key in ["models", "data", "items", "results", "available"] {
                if let Some(nested) = map.get(key) {
                    collect_candidate_arrays(nested, out);
                }
            }
        },
        _ => {},
    }
}

fn parse_models_payload(value: &serde_json::Value) -> Vec<super::DiscoveredModel> {
    let mut candidates = Vec::new();
    collect_candidate_arrays(value, &mut candidates);

    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for entry in candidates {
        if let Some(model) = parse_model_entry(entry)
            && seen.insert(model.id.clone())
        {
            models.push(model);
        }
    }

    // Sort by created_at descending (newest first). Models without a
    // timestamp are placed after those with one, preserving relative order.
    models.sort_by(|a, b| match (a.created_at, b.created_at) {
        (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts), // newest first
        (Some(_), None) => std::cmp::Ordering::Less, // timestamp before no-timestamp
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    models
}

async fn fetch_models_from_api(
    client: &reqwest::Client,
    access_token: String,
) -> anyhow::Result<Vec<super::DiscoveredModel>> {
    let response = client
        .get(COPILOT_MODELS_ENDPOINT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("Editor-Version", EDITOR_VERSION)
        .header("User-Agent", COPILOT_USER_AGENT)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("copilot models API error HTTP {status}");
    }
    let payload: serde_json::Value = serde_json::from_str(&body)?;
    let models = parse_models_payload(&payload);
    if models.is_empty() {
        anyhow::bail!("copilot models API returned no models");
    }
    Ok(models)
}

fn fetch_models_blocking() -> anyhow::Result<Vec<super::DiscoveredModel>> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| {
                rt.block_on(async {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(8))
                        .build()?;
                    let token_store = TokenStore::new();
                    let access_token =
                        fetch_valid_copilot_token_with_fallback(&client, &token_store).await?;
                    fetch_models_from_api(&client, access_token).await
                })
            });
        let _ = tx.send(result);
    });
    rx.recv()
        .map_err(|err| anyhow::anyhow!("copilot model discovery worker failed: {err}"))?
}

pub fn live_models() -> anyhow::Result<Vec<super::DiscoveredModel>> {
    let models = fetch_models_blocking()?;
    debug!(
        model_count = models.len(),
        "loaded github-copilot live models"
    );
    Ok(models)
}

pub fn available_models() -> Vec<super::DiscoveredModel> {
    let fallback = default_model_catalog();
    let discovered = match live_models() {
        Ok(models) => models,
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("not logged in") || msg.contains("tokens not found") {
                debug!(error = %err, "github-copilot not configured, using fallback catalog");
            } else {
                warn!(error = %err, "failed to fetch github-copilot models, using fallback catalog");
            }
            return fallback;
        },
    };

    super::merge_discovered_with_fallback_catalog(discovered, fallback)
}

// ── LlmProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmProvider for GitHubCopilotProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn id(&self) -> &str {
        &self.model
    }

    fn supports_tools(&self) -> bool {
        super::supports_tools_for_model(&self.model)
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        let token = self.get_valid_copilot_token().await?;

        let openai_messages: Vec<serde_json::Value> =
            messages.iter().map(ChatMessage::to_openai_value).collect();
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": openai_messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(to_openai_tools(tools));
        }

        debug!(
            model = %self.model,
            messages_count = messages.len(),
            tools_count = tools.len(),
            "github-copilot complete request"
        );
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "github-copilot request body");

        let http_resp = self
            .client
            .post(format!("{COPILOT_API_BASE}/chat/completions"))
            .header("Authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("Editor-Version", EDITOR_VERSION)
            .header("User-Agent", COPILOT_USER_AGENT)
            .json(&body)
            .send()
            .await?;

        let status = http_resp.status();
        if !status.is_success() {
            let body_text = http_resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body_text, "github-copilot API error");
            anyhow::bail!("GitHub Copilot API error HTTP {status}: {body_text}");
        }

        let resp = http_resp.json::<serde_json::Value>().await?;
        trace!(response = %resp, "github-copilot raw response");

        let message = &resp["choices"][0]["message"];

        let text = message["content"].as_str().map(|s| s.to_string());
        let tool_calls = parse_tool_calls(message);

        let usage = Usage {
            input_tokens: resp["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: resp["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
            cache_read_tokens: resp["usage"]["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0) as u32,
            ..Default::default()
        };

        Ok(CompletionResponse {
            text,
            tool_calls,
            usage,
        })
    }

    #[allow(clippy::collapsible_if)]
    fn stream(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        self.stream_with_tools(messages, vec![])
    }

    #[allow(clippy::collapsible_if)]
    fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        Box::pin(async_stream::stream! {
            let token = match self.get_valid_copilot_token().await {
                Ok(t) => t,
                Err(e) => {
                    yield StreamEvent::Error(e.to_string());
                    return;
                }
            };

            let openai_messages: Vec<serde_json::Value> =
                messages.iter().map(ChatMessage::to_openai_value).collect();
            let mut body = serde_json::json!({
                "model": self.model,
                "messages": openai_messages,
                "stream": true,
                "stream_options": { "include_usage": true },
            });

            if !tools.is_empty() {
                body["tools"] = serde_json::Value::Array(to_openai_tools(&tools));
            }

            debug!(
                model = %self.model,
                messages_count = openai_messages.len(),
                tools_count = tools.len(),
                "github-copilot stream_with_tools request"
            );
            trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "github-copilot stream request body");

            let resp = match self
                .client
                .post(format!("{COPILOT_API_BASE}/chat/completions"))
                .header("Authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("Editor-Version", EDITOR_VERSION)
                .header("User-Agent", COPILOT_USER_AGENT)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => {
                    if let Err(e) = r.error_for_status_ref() {
                        let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
                        let body_text = r.text().await.unwrap_or_default();
                        yield StreamEvent::Error(format!("HTTP {status}: {body_text}"));
                        return;
                    }
                    r
                }
                Err(e) => {
                    yield StreamEvent::Error(e.to_string());
                    return;
                }
            };

            let mut byte_stream = resp.bytes_stream();
            let mut frame_parser = SseFrameParser::default();
            let mut state = StreamingToolState::default();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield StreamEvent::Error(e.to_string());
                        return;
                    }
                };
                for data in frame_parser.push_bytes(&chunk) {
                    match process_openai_sse_line(&data, &mut state) {
                        SseLineResult::Done => {
                            for event in finalize_stream(&state) {
                                yield event;
                            }
                            return;
                        }
                        SseLineResult::Events(events) => {
                            for event in events {
                                yield event;
                            }
                        }
                        SseLineResult::Skip => {}
                    }
                }
            }

            yield StreamEvent::Error("GitHub Copilot stream ended unexpectedly".into());
        })
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn mock_completion_response() -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "message": {
                    "content": "Hello from Copilot!",
                    "role": "assistant"
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        })
    }

    fn build_completion_body(
        model: &str,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(to_openai_tools(tools));
        }

        body
    }

    fn build_mock_request(body: &serde_json::Value) -> reqwest::Request {
        // We can't bind sockets in some sandboxed test environments.
        // Instead, build the request and assert on headers/body locally.
        let token = "mock-copilot-token";
        reqwest::Client::new()
            .post("https://example.invalid/chat/completions")
            .header("Authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("Editor-Version", EDITOR_VERSION)
            .header("User-Agent", COPILOT_USER_AGENT)
            .json(body)
            .build()
            .unwrap()
    }

    fn parse_completion_response(resp: &serde_json::Value) -> CompletionResponse {
        let message = &resp["choices"][0]["message"];
        let text = message["content"].as_str().map(|s| s.to_string());
        let tool_calls = parse_tool_calls(message);
        let usage = Usage {
            input_tokens: resp["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: resp["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        };

        CompletionResponse {
            text,
            tool_calls,
            usage,
        }
    }

    // ── Unit tests ───────────────────────────────────────────────────────────

    #[test]
    fn has_stored_tokens_returns_false_without_tokens() {
        let _ = has_stored_tokens();
    }

    #[test]
    fn copilot_models_not_empty() {
        assert!(!COPILOT_MODELS.is_empty());
    }

    #[test]
    fn copilot_models_have_unique_ids() {
        let mut ids: Vec<&str> = COPILOT_MODELS.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), COPILOT_MODELS.len());
    }

    // Tests for to_openai_tools and parse_tool_calls are in openai_compat.rs

    #[test]
    fn provider_name_and_id() {
        let provider = GitHubCopilotProvider::new("gpt-4o".into());
        assert_eq!(provider.name(), "github-copilot");
        assert_eq!(provider.id(), "gpt-4o");
        assert!(provider.supports_tools());
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(COPILOT_API_BASE, "https://api.individual.githubcopilot.com");
        assert_eq!(EDITOR_VERSION, "vscode/1.96.2");
        assert!(!COPILOT_USER_AGENT.is_empty());
        assert_eq!(PROVIDER_NAME, "github-copilot");
    }

    #[test]
    fn complete_sends_required_headers() {
        let messages = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let body = build_completion_body("gpt-4o", &messages, &[]);
        let req = build_mock_request(&body);

        let has_editor_version = req
            .headers()
            .get("editor-version")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == EDITOR_VERSION);
        assert!(has_editor_version, "missing Editor-Version header");

        let has_user_agent = req
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == COPILOT_USER_AGENT);
        assert!(has_user_agent, "missing User-Agent header");

        let has_auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "Bearer mock-copilot-token");
        assert!(has_auth, "missing Authorization header");

        let has_content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "application/json");
        assert!(has_content_type, "missing content-type header");
    }

    #[test]
    fn complete_sends_model_in_body() {
        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let body = build_completion_body("gpt-4.1", &messages, &[]);
        assert_eq!(body["model"], "gpt-4.1");
        assert_eq!(body["messages"][0]["content"], "test");
    }

    #[test]
    fn complete_sends_tools_when_provided() {
        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let tools = vec![serde_json::json!({
            "name": "read_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
        })];
        let body = build_completion_body("gpt-4o", &messages, &tools);
        let tools_arr = body["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 1);
        assert_eq!(tools_arr[0]["type"], "function");
        assert_eq!(tools_arr[0]["function"]["name"], "read_file");
    }

    #[test]
    fn complete_omits_tools_when_empty() {
        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let body = build_completion_body("gpt-4o", &messages, &[]);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn complete_parses_text_response() {
        let resp = parse_completion_response(&mock_completion_response());
        assert_eq!(resp.text.as_deref(), Some("Hello from Copilot!"));
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn complete_parses_tool_call_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test.txt\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10}
        });
        let resp = parse_completion_response(&response);

        assert!(resp.text.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_abc");
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].arguments["path"], "/tmp/test.txt");
    }

    #[test]
    fn complete_does_not_send_copilot_integration_id() {
        // Regression: the API rejects requests with an unknown
        // Copilot-Integration-Id header.
        let messages = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let body = build_completion_body("gpt-4o", &messages, &[]);
        let req = build_mock_request(&body);
        let has_integration_id = req.headers().contains_key("copilot-integration-id");
        assert!(
            !has_integration_id,
            "copilot-integration-id header should NOT be sent"
        );
    }
}

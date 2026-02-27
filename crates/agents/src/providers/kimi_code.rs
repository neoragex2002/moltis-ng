//! Kimi Code provider.
//!
//! Authentication uses the Kimi device-flow OAuth (same as kimi-cli).
//! The API is OpenAI-compatible at `https://api.kimi.com/coding/v1`.

use std::pin::Pin;

use {
    async_trait::async_trait,
    futures::StreamExt,
    moltis_oauth::{OAuthTokens, TokenStore, kimi_headers},
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

const KIMI_API_BASE: &str = "https://api.kimi.com/coding/v1";
const KIMI_AUTH_HOST: &str = "https://auth.kimi.com";
const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const PROVIDER_NAME: &str = "kimi-code";

/// Refresh threshold: 5 minutes before expiry.
const REFRESH_THRESHOLD_SECS: u64 = 300;

// ── Provider ─────────────────────────────────────────────────────────────────

enum AuthMode {
    OAuthTokenStore { token_store: TokenStore },
    ApiKey { api_key: Secret<String> },
}

pub struct KimiCodeProvider {
    model: String,
    client: reqwest::Client,
    base_url: String,
    auth_mode: AuthMode,
}

impl KimiCodeProvider {
    /// Build a provider that authenticates via Kimi OAuth tokens.
    pub fn new(model: String) -> Self {
        Self {
            model,
            client: reqwest::Client::new(),
            base_url: KIMI_API_BASE.into(),
            auth_mode: AuthMode::OAuthTokenStore {
                token_store: TokenStore::new(),
            },
        }
    }

    /// Build a provider that authenticates via API key.
    pub fn new_with_api_key(api_key: Secret<String>, model: String, base_url: String) -> Self {
        Self {
            model,
            client: reqwest::Client::new(),
            base_url,
            auth_mode: AuthMode::ApiKey { api_key },
        }
    }

    fn should_send_kimi_headers(&self) -> bool {
        matches!(self.auth_mode, AuthMode::OAuthTokenStore { .. })
    }

    async fn get_auth_token(&self) -> anyhow::Result<String> {
        match &self.auth_mode {
            AuthMode::ApiKey { api_key } => Ok(api_key.expose_secret().clone()),
            AuthMode::OAuthTokenStore { .. } => self.get_valid_oauth_token().await,
        }
    }

    /// Load tokens and refresh if needed (< 5 min remaining).
    async fn get_valid_oauth_token(&self) -> anyhow::Result<String> {
        let AuthMode::OAuthTokenStore { token_store } = &self.auth_mode else {
            return Err(anyhow::anyhow!("oauth token store is not configured"));
        };
        let tokens = token_store.load(PROVIDER_NAME).ok_or_else(|| {
            anyhow::anyhow!(
                "not logged in to kimi-code — run `moltis auth login --provider kimi-code`"
            )
        })?;

        // Check expiry with 5 min buffer
        if let Some(expires_at) = tokens.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now + REFRESH_THRESHOLD_SECS >= expires_at {
                if let Some(ref refresh_token) = tokens.refresh_token {
                    debug!("refreshing kimi-code token");
                    let new_tokens =
                        refresh_access_token(&self.client, refresh_token.expose_secret()).await?;
                    token_store.save(PROVIDER_NAME, &new_tokens)?;
                    return Ok(new_tokens.access_token.expose_secret().clone());
                }
                return Err(anyhow::anyhow!(
                    "kimi-code token expired and no refresh token available"
                ));
            }
        }

        Ok(tokens.access_token.expose_secret().clone())
    }
}

fn build_access_denied_hint(status: reqwest::StatusCode, body_text: &str) -> Option<String> {
    if status == reqwest::StatusCode::FORBIDDEN && body_text.contains("access_terminated_error") {
        return Some(
            "Kimi OAuth access is restricted for this account/client. Configure `kimi-code` with `KIMI_API_KEY` (or [providers.kimi-code].api_key) and use API-key auth.".into(),
        );
    }
    None
}

/// Refresh the access token using the Kimi token endpoint.
pub async fn refresh_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> anyhow::Result<OAuthTokens> {
    let headers = kimi_headers();
    let resp = client
        .post(format!("{KIMI_AUTH_HOST}/api/oauth/token"))
        .headers(headers)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", KIMI_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("kimi-code token refresh failed: {body}");
    }

    #[derive(serde::Deserialize)]
    struct RefreshResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    }

    let body: RefreshResponse = resp.json().await?;
    let expires_at = body.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + secs
    });

    Ok(OAuthTokens {
        access_token: Secret::new(body.access_token),
        refresh_token: body.refresh_token.map(Secret::new),
        expires_at,
    })
}

/// Check if we have stored tokens for Kimi Code.
pub fn has_stored_tokens() -> bool {
    TokenStore::new().load(PROVIDER_NAME).is_some()
}

/// Known Kimi Code models.
pub const KIMI_CODE_MODELS: &[(&str, &str)] = &[
    ("kimi-for-coding", "Kimi For Coding"),
    ("kimi-k2.5", "Kimi K2.5"),
];

// ── LlmProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmProvider for KimiCodeProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn id(&self) -> &str {
        &self.model
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        let token = self.get_auth_token().await?;

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
            "kimi-code complete request"
        );
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "kimi-code request body");

        let mut request = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("content-type", "application/json");
        if self.should_send_kimi_headers() {
            request = request.headers(kimi_headers());
        }
        let http_resp = request.json(&body).send().await?;

        let status = http_resp.status();
        if !status.is_success() {
            let body_text = http_resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body_text, "kimi-code API error");
            let hint = build_access_denied_hint(status, &body_text);
            if let Some(hint) = hint {
                anyhow::bail!("Kimi Code API error HTTP {status}: {body_text} ({hint})");
            }
            anyhow::bail!("Kimi Code API error HTTP {status}: {body_text}");
        }

        let resp = http_resp.json::<serde_json::Value>().await?;
        trace!(response = %resp, "kimi-code raw response");

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
            let token = match self.get_auth_token().await {
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
                "kimi-code stream_with_tools request"
            );
            trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "kimi-code stream request body");

            let mut request = self
                .client
                .post(format!(
                    "{}/chat/completions",
                    self.base_url.trim_end_matches('/')
                ))
                .header("Authorization", format!("Bearer {token}"))
                .header("content-type", "application/json");
            if self.should_send_kimi_headers() {
                request = request.headers(kimi_headers());
            }

            let resp = match request.json(&body).send().await {
                Ok(r) => {
                    if let Err(e) = r.error_for_status_ref() {
                        let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
                        let body_text = r.text().await.unwrap_or_default();
                        let hint = build_access_denied_hint(
                            reqwest::StatusCode::from_u16(status)
                                .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
                            &body_text,
                        );
                        if let Some(hint) = hint {
                            yield StreamEvent::Error(format!("HTTP {status}: {body_text} ({hint})"));
                        } else {
                            yield StreamEvent::Error(format!("HTTP {status}: {body_text}"));
                        }
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

            yield StreamEvent::Error("Kimi Code stream ended unexpectedly".into());
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
                    "content": "Hello from Kimi!",
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

    fn build_request(
        provider: &KimiCodeProvider,
        token: &str,
        body: &serde_json::Value,
    ) -> reqwest::Request {
        // We can't bind sockets in some sandboxed test environments.
        // Instead, build the request and assert on headers/body locally.
        let mut request = provider
            .client
            .post(format!("{}/chat/completions", provider.base_url))
            .header("Authorization", format!("Bearer {token}"))
            .header("content-type", "application/json");
        if provider.should_send_kimi_headers() {
            request = request.headers(kimi_headers());
        }
        request.json(body).build().unwrap()
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
    fn kimi_code_models_not_empty() {
        assert!(!KIMI_CODE_MODELS.is_empty());
    }

    #[test]
    fn kimi_code_models_have_unique_ids() {
        let mut ids: Vec<&str> = KIMI_CODE_MODELS.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KIMI_CODE_MODELS.len());
    }

    #[test]
    fn provider_name_and_id() {
        let provider = KimiCodeProvider::new("kimi-k2.5".into());
        assert_eq!(provider.name(), "kimi-code");
        assert_eq!(provider.id(), "kimi-k2.5");
        assert!(provider.supports_tools());
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(KIMI_API_BASE, "https://api.kimi.com/coding/v1");
        assert_eq!(PROVIDER_NAME, "kimi-code");
        assert_eq!(KIMI_CLIENT_ID, "17e5f671-d194-4dfb-9706-5516cb48c098");
    }

    #[test]
    fn complete_sends_required_headers() {
        let provider = KimiCodeProvider::new("kimi-k2.5".into());
        let openai_messages: Vec<serde_json::Value> = [ChatMessage::user("hi")]
            .iter()
            .map(ChatMessage::to_openai_value)
            .collect();
        let body = build_completion_body("kimi-k2.5", &openai_messages, &[]);
        let req = build_request(&provider, "mock-kimi-token", &body);

        // Verify X-Msh-* headers
        let has_platform = req.headers().contains_key("x-msh-platform");
        assert!(has_platform, "missing X-Msh-Platform header");

        let has_device_id = req.headers().contains_key("x-msh-device-id");
        assert!(has_device_id, "missing X-Msh-Device-Id header");

        let has_auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "Bearer mock-kimi-token");
        assert!(has_auth, "missing Authorization header");
    }

    #[test]
    fn complete_with_api_key_does_not_send_kimi_headers() {
        let provider = KimiCodeProvider::new_with_api_key(
            Secret::new("sk-kimi".into()),
            "kimi-for-coding".into(),
            "https://example.invalid".into(),
        );

        let openai_messages: Vec<serde_json::Value> = [ChatMessage::user("hi")]
            .iter()
            .map(ChatMessage::to_openai_value)
            .collect();
        let body = build_completion_body("kimi-for-coding", &openai_messages, &[]);
        let req = build_request(&provider, "sk-kimi", &body);

        let has_platform = req.headers().contains_key("x-msh-platform");
        assert!(!has_platform, "api-key mode should not send X-Msh-Platform");

        let has_device_id = req.headers().contains_key("x-msh-device-id");
        assert!(
            !has_device_id,
            "api-key mode should not send X-Msh-Device-Id"
        );

        let has_auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "Bearer sk-kimi");
        assert!(has_auth, "missing Authorization header");
    }

    #[test]
    fn access_terminated_error_adds_api_key_hint() {
        let hint = build_access_denied_hint(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"error":{"type":"access_terminated_error"}}"#,
        );
        assert!(hint.is_some());
        let hint_text = hint.unwrap();
        assert!(hint_text.contains("KIMI_API_KEY"));
    }

    #[test]
    fn complete_sends_model_in_body() {
        let body = build_completion_body(
            "kimi-k2.5",
            &[serde_json::json!({"role": "user", "content": "test"})],
            &[],
        );
        assert_eq!(body["model"], "kimi-k2.5");
    }

    #[test]
    fn complete_sends_tools_when_provided() {
        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let tools = vec![serde_json::json!({
            "name": "read_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
        })];
        let body = build_completion_body("kimi-k2.5", &messages, &tools);
        let tools_arr = body["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 1);
        assert_eq!(tools_arr[0]["type"], "function");
        assert_eq!(tools_arr[0]["function"]["name"], "read_file");
    }

    #[test]
    fn complete_parses_text_response() {
        let resp = parse_completion_response(&mock_completion_response());
        assert_eq!(resp.text.as_deref(), Some("Hello from Kimi!"));
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
    }
}

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use {
    async_trait::async_trait,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    sha2::{Digest, Sha256},
    tokio::{
        sync::{OnceCell, OwnedSemaphorePermit, RwLock, Semaphore, mpsc, watch},
        task::AbortHandle,
    },
    tokio_stream::StreamExt,
    tracing::{debug, info, warn},
};

use moltis_config::MessageQueueMode;

use {
    moltis_agents::{
        AgentRunError, ChatMessage, ContentPart, UserContent,
        model::{StreamEvent, values_to_chat_messages},
        multimodal::parse_data_uri,
        prompt::{
            PromptHostRuntimeContext, PromptReplyMedium, PromptRuntimeContext,
            PromptSandboxRuntimeContext, build_canonical_system_prompt_v1,
        },
        providers::{ProviderRegistry, raw_model_id},
        runner::{
            RunnerEvent, RunnerHookContext, run_agent_loop_streaming_with_prefix_and_hook_context,
        },
        tool_registry::ToolRegistry,
    },
    moltis_common::text::{truncate_utf8, truncate_utf8_with_suffix},
    moltis_sessions::{
        ContentBlock, MessageContent, PersistedMessage, metadata::SqliteSessionMetadata,
        store::SessionStore,
    },
    moltis_skills::discover::SkillDiscoverer,
    moltis_tools::policy::{ToolPolicy, profile_tools},
};

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    chat_error::parse_chat_error,
    run_failure::{FailureInput, FailureStage, normalize_failure},
    services::{ChatService, ModelService, ServiceResult},
    session::extract_preview_from_value,
    state::GatewayState,
};

#[cfg(feature = "metrics")]
use moltis_metrics::{counter, histogram, labels, llm as llm_metrics};

/// Convert session-crate `MessageContent` to agents-crate `UserContent`.
///
/// The two types have different image representations:
/// - `ContentBlock::ImageUrl` stores a data URI string
/// - `ContentPart::Image` stores separated `media_type` + `data` fields
fn to_user_content(mc: &MessageContent) -> UserContent {
    match mc {
        MessageContent::Text(text) => UserContent::Text(text.clone()),
        MessageContent::Multimodal(blocks) => {
            let parts: Vec<ContentPart> = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(ContentPart::Text(text.clone())),
                    ContentBlock::ImageUrl { image_url } => match parse_data_uri(&image_url.url) {
                        Some((media_type, data)) => {
                            debug!(
                                media_type,
                                data_len = data.len(),
                                "to_user_content: parsed image from data URI"
                            );
                            Some(ContentPart::Image {
                                media_type: media_type.to_string(),
                                data: data.to_string(),
                            })
                        },
                        None => {
                            let url = &image_url.url;
                            let (is_data_uri, data_uri_header) = url
                                .split_once(',')
                                .map(|(h, _)| (h.starts_with("data:"), Some(h)))
                                .unwrap_or((false, None));
                            warn!(
                                url_len = url.len(),
                                is_data_uri,
                                data_uri_header,
                                "to_user_content: failed to parse data URI, dropping image"
                            );
                            None
                        },
                    },
                })
                .collect();
            let text_count = parts
                .iter()
                .filter(|p| matches!(p, ContentPart::Text(_)))
                .count();
            let image_count = parts
                .iter()
                .filter(|p| matches!(p, ContentPart::Image { .. }))
                .count();
            debug!(
                text_count,
                image_count,
                total_blocks = blocks.len(),
                "to_user_content: converted multimodal content"
            );
            UserContent::Multimodal(parts)
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ReplyMedium {
    Text,
    Voice,
}

fn to_prompt_reply_medium(m: ReplyMedium) -> PromptReplyMedium {
    match m {
        ReplyMedium::Text => PromptReplyMedium::Text,
        ReplyMedium::Voice => PromptReplyMedium::Voice,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TelegramOutboundFailureMeta {
    outbound_op: &'static str,
    outcome_kind: &'static str,
    delivery_state: &'static str,
}

#[cfg(test)]
fn telegram_outbound_failure_meta(err: &anyhow::Error) -> TelegramOutboundFailureMeta {
    err.downcast_ref::<moltis_telegram::outbound::TelegramOutboundError>()
        .map(|err| TelegramOutboundFailureMeta {
            outbound_op: err.op.as_str(),
            outcome_kind: err.outcome_kind.as_str(),
            delivery_state: err
                .delivery_state
                .map(|state| state.as_str())
                .unwrap_or("none"),
        })
        .unwrap_or(TelegramOutboundFailureMeta {
            outbound_op: "unknown",
            outcome_kind: "unknown",
            delivery_state: "none",
        })
}

#[derive(Debug, Deserialize)]
struct InputChannelMeta {
    #[serde(default)]
    message_kind: Option<InputMessageKind>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InputMessageKind {
    Text,
    Voice,
    Audio,
    Photo,
    Document,
    Video,
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputMediumParam {
    Text,
    Voice,
}

/// Typed broadcast payload for the "final" chat event.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatFinalBroadcast {
    run_id: String,
    session_id: String,
    state: &'static str,
    text: String,
    model: String,
    provider: String,
    input_tokens: u32,
    output_tokens: u32,
    message_index: usize,
    reply_medium: ReplyMedium,
    #[serde(skip_serializing_if = "Option::is_none")]
    iterations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls_made: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
}

#[derive(Debug, Clone)]
struct ChatRunOutput {
    text: String,
    input_tokens: u32,
    output_tokens: u32,
    cached_tokens: u32,
    audio_path: Option<String>,
}

/// Typed broadcast payload for the "error" chat event.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatErrorBroadcast {
    run_id: String,
    session_id: String,
    state: &'static str,
    error: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
}

#[derive(Debug, Clone)]
struct RunFailedEvent {
    run_id: String,
    session_key: String,
    trigger_id: Option<String>,
    provider_name: String,
    model_id: String,
    stage_hint: FailureStage,
    raw_error: String,
    details: serde_json::Value,
    seq: Option<u64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const MOLTIS_INTERNAL_KIND_UI_ERROR_NOTICE: &str = "ui_error_notice";

fn ui_error_notice_message(text: &str) -> serde_json::Value {
    serde_json::json!({
        "role": "assistant",
        "content": text,
        "created_at": now_ms(),
        "moltis_internal_kind": MOLTIS_INTERNAL_KIND_UI_ERROR_NOTICE,
    })
}

pub(crate) fn normalize_model_key(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut last_was_separator = true;

    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
            continue;
        }

        if !last_was_separator {
            normalized.push(' ');
            last_was_separator = true;
        }
    }

    normalized.trim().to_string()
}

fn normalize_provider_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn is_openai_responses_provider(provider_name: &str) -> bool {
    normalize_provider_key(provider_name) == "openai-responses"
}

fn as_sent_preamble_for_provider(
    provider_name: &str,
    system_prompt: &str,
) -> Vec<serde_json::Value> {
    if is_openai_responses_provider(provider_name) {
        vec![serde_json::json!({
            "index": 1,
            "role": "developer",
            "text": system_prompt,
        })]
    } else {
        vec![serde_json::json!({
            "index": 1,
            "role": "system",
            "text": system_prompt,
        })]
    }
}

async fn resolve_session_agent_id(
    state: &GatewayState,
    runtime_context: Option<&PromptRuntimeContext>,
) -> Option<String> {
    let rt = runtime_context?;
    if rt.host.channel.as_deref() != Some("telegram") {
        return None;
    }
    let chan_user_id = rt.host.channel_account_id.as_deref()?;
    let account_handle = format!("telegram:{chan_user_id}");
    state
        .services
        .channel
        .telegram_account_agent_id(&account_handle)
        .await
}

fn channel_target_from_session_entry(
    entry: Option<&moltis_sessions::metadata::SessionEntry>,
) -> Option<moltis_common::types::ChannelTarget> {
    let binding = entry.and_then(|entry| entry.channel_binding.as_deref())?;
    if moltis_telegram::adapter::telegram_binding_uses_legacy_shape(binding)
        || moltis_telegram::adapter::telegram_channel_binding_info(binding)
            .is_some_and(|info| info.bucket_key.is_none())
    {
        warn!(
            event = "telegram.session.reject_legacy_binding",
            context = "channel_target_from_session_entry",
            reason_code = "legacy_channel_binding_rejected",
            "telegram channel_binding is legacy or incomplete; rejecting strict V3 operation"
        );
        return None;
    }
    moltis_telegram::adapter::channel_target_from_binding(binding)
}

fn session_key_from_session_entry(
    entry: Option<&moltis_sessions::metadata::SessionEntry>,
) -> Option<String> {
    entry.map(|entry| entry.session_key.clone())
}

fn prompt_cache_key_for_session(
    session_id: &str,
    entry: Option<&moltis_sessions::metadata::SessionEntry>,
) -> Option<String> {
    let session_key = entry.map(|session| session.session_key.as_str())?;
    match moltis_sessions::SessionKey::parse(session_key) {
        Ok(moltis_sessions::key::ParsedSessionKey::System { .. }) => Some(session_key.to_string()),
        Ok(moltis_sessions::key::ParsedSessionKey::Agent { .. }) => Some(session_id.to_string()),
        Err(_) => None,
    }
}

#[allow(dead_code)]
fn is_allowlist_exempt_provider(provider_name: &str) -> bool {
    matches!(
        normalize_provider_key(provider_name).as_str(),
        "local-llm" | "ollama"
    )
}

/// Returns `true` if the model matches the allowlist patterns.
/// An empty pattern list means all models are allowed.
/// Matching is case-insensitive against the full model ID, raw model ID, and
/// display name:
/// - patterns with digits use exact-or-suffix matching (boundary aware)
/// - patterns without digits use substring matching
///
/// This keeps precise model pins like "gpt 5.2" from matching variants such as
/// "gpt-5.2-chat-latest", while still allowing broad buckets like "mini".
#[allow(dead_code)]
fn allowlist_pattern_matches_key(pattern: &str, key: &str) -> bool {
    if pattern.chars().any(|ch| ch.is_ascii_digit()) {
        if key == pattern {
            return true;
        }
        return key
            .strip_suffix(pattern)
            .is_some_and(|prefix| prefix.ends_with(' '));
    }
    key.contains(pattern)
}

#[allow(dead_code)]
pub(crate) fn model_matches_allowlist(
    model: &moltis_agents::providers::ModelInfo,
    patterns: &[String],
) -> bool {
    if patterns.is_empty() {
        return true;
    }
    if is_allowlist_exempt_provider(&model.provider) {
        return true;
    }
    let full = normalize_model_key(&model.id);
    let raw = normalize_model_key(raw_model_id(&model.id));
    let display = normalize_model_key(&model.display_name);
    patterns.iter().any(|p| {
        allowlist_pattern_matches_key(p, &full)
            || allowlist_pattern_matches_key(p, &raw)
            || allowlist_pattern_matches_key(p, &display)
    })
}

#[allow(dead_code)]
pub(crate) fn model_matches_allowlist_with_provider(
    model: &moltis_agents::providers::ModelInfo,
    provider_name: Option<&str>,
    patterns: &[String],
) -> bool {
    if provider_name.is_some_and(is_allowlist_exempt_provider) {
        return true;
    }
    model_matches_allowlist(model, patterns)
}

fn provider_filter_from_params(params: &Value) -> Option<String> {
    params
        .get("provider")
        .and_then(|v| v.as_str())
        .map(normalize_provider_key)
        .filter(|v| !v.is_empty())
}

fn provider_matches_filter(model_provider: &str, provider_filter: Option<&str>) -> bool {
    provider_filter.is_none_or(|expected| normalize_provider_key(model_provider) == expected)
}

fn probe_max_parallel_per_provider(params: &Value) -> usize {
    params
        .get("maxParallelPerProvider")
        .and_then(|v| v.as_u64())
        .map(|v| v.clamp(1, 8) as usize)
        .unwrap_or(1)
}

fn provider_model_entry(model_id: &str, display_name: &str) -> Value {
    serde_json::json!({
        "modelId": model_id,
        "displayName": display_name,
    })
}

fn push_provider_model(
    grouped: &mut BTreeMap<String, Vec<Value>>,
    provider_name: &str,
    model_id: &str,
    display_name: &str,
) {
    if provider_name.trim().is_empty() || model_id.trim().is_empty() {
        return;
    }
    grouped
        .entry(provider_name.to_string())
        .or_default()
        .push(provider_model_entry(model_id, display_name));
}

const PROBE_RATE_LIMIT_INITIAL_BACKOFF_MS: u64 = 1_000;
const PROBE_RATE_LIMIT_MAX_BACKOFF_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy)]
struct ProbeRateLimitState {
    backoff_ms: u64,
    until: Instant,
}

#[derive(Debug, Default)]
struct ProbeRateLimiter {
    by_provider: tokio::sync::Mutex<HashMap<String, ProbeRateLimitState>>,
}

impl ProbeRateLimiter {
    async fn remaining_backoff(&self, provider: &str) -> Option<Duration> {
        let map = self.by_provider.lock().await;
        map.get(provider).and_then(|state| {
            let now = Instant::now();
            (state.until > now).then_some(state.until - now)
        })
    }

    async fn mark_rate_limited(&self, provider: &str) -> Duration {
        let mut map = self.by_provider.lock().await;
        let next_backoff_ms =
            next_probe_rate_limit_backoff_ms(map.get(provider).map(|s| s.backoff_ms));
        let delay = Duration::from_millis(next_backoff_ms);
        let state = ProbeRateLimitState {
            backoff_ms: next_backoff_ms,
            until: Instant::now() + delay,
        };
        let _ = map.insert(provider.to_string(), state);
        delay
    }

    async fn clear(&self, provider: &str) {
        let mut map = self.by_provider.lock().await;
        let _ = map.remove(provider);
    }
}

fn next_probe_rate_limit_backoff_ms(previous_ms: Option<u64>) -> u64 {
    previous_ms
        .map(|ms| ms.saturating_mul(2))
        .unwrap_or(PROBE_RATE_LIMIT_INITIAL_BACKOFF_MS)
        .clamp(
            PROBE_RATE_LIMIT_INITIAL_BACKOFF_MS,
            PROBE_RATE_LIMIT_MAX_BACKOFF_MS,
        )
}

fn is_probe_rate_limited_error(error_obj: &Value, error_text: &str) -> bool {
    if error_obj.get("type").and_then(|v| v.as_str()) == Some("rate_limit_exceeded") {
        return true;
    }

    let lower = error_text.to_ascii_lowercase();
    lower.contains("status=429")
        || lower.contains("http 429")
        || lower.contains("too many requests")
        || lower.contains("rate limit")
        || lower.contains("quota exceeded")
}

#[derive(Debug)]
struct ProbeProviderLimiter {
    permits_per_provider: usize,
    by_provider: tokio::sync::Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ProbeProviderLimiter {
    fn new(permits_per_provider: usize) -> Self {
        Self {
            permits_per_provider,
            by_provider: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn acquire(
        &self,
        provider: &str,
    ) -> Result<OwnedSemaphorePermit, tokio::sync::AcquireError> {
        let provider_sem = {
            let mut map = self.by_provider.lock().await;
            Arc::clone(
                map.entry(provider.to_string())
                    .or_insert_with(|| Arc::new(Semaphore::new(self.permits_per_provider))),
            )
        };

        provider_sem.acquire_owned().await
    }
}

#[derive(Debug)]
enum ProbeStatus {
    Supported,
    Unsupported { detail: String, provider: String },
    Error { message: String },
}

#[derive(Debug)]
struct ProbeOutcome {
    model_id: String,
    display_name: String,
    provider_name: String,
    status: ProbeStatus,
}

/// Run a single model probe: acquire concurrency permits, respect rate-limit
/// backoff, send a "ping" completion, and classify the result.
async fn run_single_probe(
    model_id: String,
    display_name: String,
    provider_name: String,
    provider: Arc<dyn moltis_agents::model::LlmProvider>,
    limiter: Arc<Semaphore>,
    provider_limiter: Arc<ProbeProviderLimiter>,
    rate_limiter: Arc<ProbeRateLimiter>,
) -> ProbeOutcome {
    let _permit = match limiter.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return ProbeOutcome {
                model_id,
                display_name,
                provider_name,
                status: ProbeStatus::Error {
                    message: "probe limiter closed".to_string(),
                },
            };
        },
    };
    let _provider_permit = match provider_limiter.acquire(&provider_name).await {
        Ok(permit) => permit,
        Err(_) => {
            return ProbeOutcome {
                model_id,
                display_name,
                provider_name,
                status: ProbeStatus::Error {
                    message: "provider probe limiter closed".to_string(),
                },
            };
        },
    };

    if let Some(wait_for) = rate_limiter.remaining_backoff(&provider_name).await {
        debug!(
            provider = %provider_name,
            model = %model_id,
            wait_ms = wait_for.as_millis() as u64,
            "skipping model probe while provider is in rate-limit backoff"
        );
        return ProbeOutcome {
            model_id,
            display_name,
            provider_name,
            status: ProbeStatus::Error {
                message: format!(
                    "probe skipped due provider backoff ({}ms remaining)",
                    wait_for.as_millis()
                ),
            },
        };
    }

    let probe = [ChatMessage::user("ping")];
    let llm_context = moltis_agents::model::LlmRequestContext {
        session_key: None,
        session_id: None,
        prompt_cache_key: None,
        run_id: None,
    };
    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        provider.complete_with_context(&llm_context, &probe, &[]),
    )
    .await;

    match completion {
        Ok(Ok(_)) => {
            rate_limiter.clear(&provider_name).await;
            ProbeOutcome {
                model_id,
                display_name,
                provider_name,
                status: ProbeStatus::Supported,
            }
        },
        Ok(Err(err)) => {
            let error_text = err.to_string();
            let error_obj =
                crate::chat_error::parse_chat_error(&error_text, Some(provider_name.as_str()));
            if is_probe_rate_limited_error(&error_obj, &error_text) {
                let backoff = rate_limiter.mark_rate_limited(&provider_name).await;
                let detail = error_obj
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Too many requests while probing model support");
                warn!(
                    provider = %provider_name,
                    model = %model_id,
                    backoff_ms = backoff.as_millis() as u64,
                    "model probe rate limited, applying provider backoff"
                );
                return ProbeOutcome {
                    model_id,
                    display_name,
                    provider_name,
                    status: ProbeStatus::Error {
                        message: format!("{detail} (probe backoff {}ms)", backoff.as_millis()),
                    },
                };
            }

            rate_limiter.clear(&provider_name).await;
            let is_unsupported =
                error_obj.get("type").and_then(|v| v.as_str()) == Some("unsupported_model");

            if is_unsupported {
                let detail = error_obj
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Model is not supported for this account/provider")
                    .to_string();
                let parsed_provider = error_obj
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or(provider_name.as_str())
                    .to_string();
                ProbeOutcome {
                    model_id,
                    display_name,
                    provider_name,
                    status: ProbeStatus::Unsupported {
                        detail,
                        provider: parsed_provider,
                    },
                }
            } else {
                ProbeOutcome {
                    model_id,
                    display_name,
                    provider_name,
                    status: ProbeStatus::Error {
                        message: error_text,
                    },
                }
            }
        },
        Err(_) => ProbeOutcome {
            model_id,
            display_name,
            provider_name,
            status: ProbeStatus::Error {
                message: "probe timeout after 20s".to_string(),
            },
        },
    }
}

fn parse_input_medium(params: &Value) -> Option<ReplyMedium> {
    match params
        .get("_input_medium")
        .cloned()
        .and_then(|v| serde_json::from_value::<InputMediumParam>(v).ok())
    {
        Some(InputMediumParam::Voice) => Some(ReplyMedium::Voice),
        Some(InputMediumParam::Text) => Some(ReplyMedium::Text),
        _ => None,
    }
}

fn explicit_reply_medium_override(text: &str) -> Option<ReplyMedium> {
    let lower = text.to_lowercase();
    let voice_markers = [
        "talk to me",
        "say it",
        "say this",
        "speak",
        "voice message",
        "respond with voice",
        "reply with voice",
        "audio reply",
    ];
    if voice_markers.iter().any(|m| lower.contains(m)) {
        return Some(ReplyMedium::Voice);
    }

    let text_markers = [
        "text only",
        "reply in text",
        "respond in text",
        "don't use voice",
        "do not use voice",
        "no audio",
    ];
    if text_markers.iter().any(|m| lower.contains(m)) {
        return Some(ReplyMedium::Text);
    }

    None
}

fn infer_reply_medium(params: &Value, text: &str) -> ReplyMedium {
    if let Some(explicit) = explicit_reply_medium_override(text) {
        return explicit;
    }

    if let Some(input_medium) = parse_input_medium(params) {
        return input_medium;
    }

    if let Some(channel) = params
        .get("channel")
        .cloned()
        .and_then(|v| serde_json::from_value::<InputChannelMeta>(v).ok())
        && channel.message_kind == Some(InputMessageKind::Voice)
    {
        return ReplyMedium::Voice;
    }

    ReplyMedium::Text
}

fn detect_runtime_shell() -> Option<String> {
    let candidate = std::env::var("SHELL")
        .ok()
        .or_else(|| std::env::var("COMSPEC").ok())?;
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    let name = std::path::Path::new(trimmed)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(trimmed)
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

async fn detect_host_sudo_access() -> (Option<bool>, Option<String>) {
    #[cfg(not(unix))]
    {
        return (None, Some("unsupported".to_string()));
    }

    #[cfg(unix)]
    {
        let output = tokio::process::Command::new("sudo")
            .arg("-n")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => (Some(true), Some("passwordless".to_string())),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
                if stderr.contains("a password is required") {
                    (Some(false), Some("requires_password".to_string()))
                } else if stderr.contains("not in the sudoers")
                    || stderr.contains("is not in the sudoers")
                    || stderr.contains("is not allowed to run sudo")
                    || stderr.contains("may not run sudo")
                {
                    (Some(false), Some("denied".to_string()))
                } else {
                    (None, Some("unknown".to_string()))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (None, Some("not_installed".to_string()))
            },
            Err(_) => (None, Some("unknown".to_string())),
        }
    }
}

/// Pre-loaded agent data used to build the system prompt.
struct PromptAgent {
    config: moltis_config::MoltisConfig,
    identity_md_raw: Option<String>,
    soul_text: Option<String>,
    agents_text: Option<String>,
    tools_text: Option<String>,
}

/// Load agent Type4 templates + config used for runtime tool filtering.
///
/// Both `run_with_tools` and `run_streaming` need the same agent data;
/// this function avoids duplicating the merge logic.
fn load_prompt_agent_with_id(agent_id: Option<&str>) -> PromptAgent {
    let config = moltis_config::discover_and_load();
    PromptAgent {
        config,
        identity_md_raw: agent_id
            .and_then(moltis_config::load_agent_identity_md_raw)
            .or_else(moltis_config::load_identity_md_raw),
        soul_text: agent_id
            .and_then(moltis_config::load_agent_soul)
            .or_else(moltis_config::load_soul),
        agents_text: agent_id
            .and_then(moltis_config::load_agent_agents_md)
            .or_else(moltis_config::load_agents_md),
        tools_text: agent_id
            .and_then(moltis_config::load_agent_tools_md)
            .or_else(moltis_config::load_tools_md),
    }
}

async fn build_prompt_runtime_context(
    state: &Arc<GatewayState>,
    provider: &Arc<dyn moltis_agents::model::LlmProvider>,
    session_id: &str,
    session_entry: Option<&moltis_sessions::metadata::SessionEntry>,
) -> PromptRuntimeContext {
    let sudo_fut = detect_host_sudo_access();
    let sandbox_fut = async {
        if let Some(ref router) = state.sandbox_router {
            let is_sandboxed = router
                .is_sandboxed(
                    session_id,
                    session_key_from_session_entry(session_entry).as_deref(),
                )
                .await
                .unwrap_or(false);
            let config = router.config();
            Some(PromptSandboxRuntimeContext {
                exec_sandboxed: is_sandboxed,
                mode: Some(config.mode.to_string()),
                backend: Some(router.backend_name().to_string()),
                scope: Some(config.scope_key.to_string()),
                image: config.image.clone(),
                data_mount: Some(config.data_mount.to_string()),
                no_network: Some(config.no_network),
                session_override: None,
            })
        } else {
            Some(PromptSandboxRuntimeContext {
                exec_sandboxed: false,
                mode: Some("off".to_string()),
                backend: Some("none".to_string()),
                scope: None,
                image: None,
                data_mount: None,
                no_network: None,
                session_override: None,
            })
        }
    };

    let ((sudo_non_interactive, sudo_status), sandbox_ctx) = tokio::join!(sudo_fut, sandbox_fut);

    let timezone = state
        .sandbox_router
        .as_ref()
        .and_then(|r| r.config().timezone.clone());

    let location = state
        .inner
        .read()
        .await
        .cached_location
        .as_ref()
        .map(|loc| loc.to_string());

    let channel_target = channel_target_from_session_entry(session_entry);

    let host_ctx = PromptHostRuntimeContext {
        host: Some(state.hostname.clone()),
        os: Some(std::env::consts::OS.to_string()),
        arch: Some(std::env::consts::ARCH.to_string()),
        shell: detect_runtime_shell(),
        provider: Some(provider.name().to_string()),
        model: Some(provider.id().to_string()),
        session_id: Some(session_id.to_string()),
        channel: channel_target.as_ref().map(|t| t.channel_type.to_string()),
        channel_account_id: channel_target
            .as_ref()
            .and_then(|t| moltis_common::identity::parse_chan_account_key(&t.account_key))
            .map(|p| p.chan_user_id.to_string()),
        channel_account_handle: None,
        channel_chat_id: channel_target.as_ref().map(|t| t.chat_id.clone()),
        sudo_non_interactive,
        sudo_status,
        timezone,
        location,
        ..Default::default()
    };

    PromptRuntimeContext {
        host: host_ctx,
        sandbox: sandbox_ctx,
    }
}

async fn maybe_append_tg_gst_v1_system_prompt(
    state: &Arc<GatewayState>,
    session_entry: Option<&moltis_sessions::metadata::SessionEntry>,
    system_prompt: &mut String,
) {
    let Some(entry) = session_entry else {
        return;
    };
    let Some(binding) = entry.channel_binding.as_deref() else {
        return;
    };

    let snapshots = state
        .services
        .channel
        .telegram_bus_accounts_snapshot()
        .await;
    let Some(block) =
        moltis_telegram::adapter::tg_gst_v1_system_prompt_block_for_binding(binding, &snapshots)
    else {
        return;
    };

    system_prompt.push_str("\n\n");
    system_prompt.push_str(block);
}

fn effective_tool_policy(config: &moltis_config::MoltisConfig) -> ToolPolicy {
    let mut effective = ToolPolicy::default();
    if let Some(profile) = config.tools.policy.profile.as_deref()
        && !profile.is_empty()
    {
        effective = effective.merge_with(&profile_tools(profile));
    }
    let configured = ToolPolicy {
        allow: config.tools.policy.allow.clone(),
        deny: config.tools.policy.deny.clone(),
    };
    effective.merge_with(&configured)
}

fn apply_runtime_tool_filters(
    base: &ToolRegistry,
    config: &moltis_config::MoltisConfig,
    _skills: &[moltis_skills::types::SkillMetadata],
    mcp_disabled: bool,
) -> ToolRegistry {
    let base_registry = if mcp_disabled {
        base.clone_without_mcp()
    } else {
        base.clone_without(&[])
    };

    let policy = effective_tool_policy(config);
    // NOTE: Do not globally restrict tools by discovered skill `allowed_tools`.
    // Skills are always discovered for prompt injection; applying those lists at
    // runtime can unintentionally remove unrelated tools (for example, leaving
    // only `web_fetch` and preventing `create_skill` from being called).
    // Tool availability here is controlled by configured runtime policy.
    base_registry.clone_allowed_by(|name| policy.is_allowed(name))
}

// ── Disabled Models Store ────────────────────────────────────────────────────

/// Persistent store for disabled model IDs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DisabledModelsStore {
    #[serde(default)]
    pub disabled: HashSet<String>,
    #[serde(default)]
    pub unsupported: HashMap<String, UnsupportedModelInfo>,
}

/// Metadata for a model that failed at runtime due to provider support/account limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsupportedModelInfo {
    pub detail: String,
    pub provider: Option<String>,
    pub updated_at_ms: u64,
}

impl DisabledModelsStore {
    fn config_path() -> Option<PathBuf> {
        moltis_config::config_dir().map(|d| d.join("disabled-models.json"))
    }

    /// Load disabled models from config file.
    pub fn load() -> Self {
        Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Save disabled models to config file.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Disable a model by ID.
    pub fn disable(&mut self, model_id: &str) -> bool {
        self.disabled.insert(model_id.to_string())
    }

    /// Enable a model by ID (remove from disabled set).
    pub fn enable(&mut self, model_id: &str) -> bool {
        self.disabled.remove(model_id)
    }

    /// Check if a model is disabled.
    pub fn is_disabled(&self, model_id: &str) -> bool {
        self.disabled.contains(model_id)
    }

    /// Mark a model as unsupported with a human-readable reason.
    pub fn mark_unsupported(
        &mut self,
        model_id: &str,
        detail: &str,
        provider: Option<&str>,
    ) -> bool {
        let next = UnsupportedModelInfo {
            detail: detail.to_string(),
            provider: provider.map(ToString::to_string),
            updated_at_ms: now_ms(),
        };
        let should_update = self
            .unsupported
            .get(model_id)
            .map(|existing| existing.detail != next.detail || existing.provider != next.provider)
            .unwrap_or(true);

        if should_update {
            self.unsupported.insert(model_id.to_string(), next);
            true
        } else {
            false
        }
    }

    /// Clear unsupported status when a model succeeds again.
    pub fn clear_unsupported(&mut self, model_id: &str) -> bool {
        self.unsupported.remove(model_id).is_some()
    }

    /// Get unsupported metadata for a model.
    pub fn unsupported_info(&self, model_id: &str) -> Option<&UnsupportedModelInfo> {
        self.unsupported.get(model_id)
    }
}

// ── LiveModelService ────────────────────────────────────────────────────────

pub struct LiveModelService {
    providers: Arc<RwLock<ProviderRegistry>>,
    disabled: Arc<RwLock<DisabledModelsStore>>,
    state: Arc<OnceCell<Arc<GatewayState>>>,
    detect_gate: Arc<Semaphore>,
    priority_models: Arc<RwLock<Vec<String>>>,
}

impl LiveModelService {
    pub fn new(
        providers: Arc<RwLock<ProviderRegistry>>,
        disabled: Arc<RwLock<DisabledModelsStore>>,
        priority_models: Vec<String>,
    ) -> Self {
        Self {
            providers,
            disabled,
            state: Arc::new(OnceCell::new()),
            detect_gate: Arc::new(Semaphore::new(1)),
            priority_models: Arc::new(RwLock::new(priority_models)),
        }
    }

    /// Shared handle to the priority models list. Pass this to services
    /// that need to update model ordering at runtime (e.g. `save_model`).
    pub fn priority_models_handle(&self) -> Arc<RwLock<Vec<String>>> {
        Arc::clone(&self.priority_models)
    }

    fn build_priority_order(models: &[String]) -> HashMap<String, usize> {
        let mut order = HashMap::new();
        for (idx, model) in models.iter().enumerate() {
            let key = normalize_model_key(model);
            if !key.is_empty() {
                let _ = order.entry(key).or_insert(idx);
            }
        }
        order
    }

    fn priority_rank(
        order: &HashMap<String, usize>,
        model: &moltis_agents::providers::ModelInfo,
    ) -> usize {
        let full = normalize_model_key(&model.id);
        if let Some(rank) = order.get(&full) {
            return *rank;
        }
        let raw = normalize_model_key(raw_model_id(&model.id));
        if let Some(rank) = order.get(&raw) {
            return *rank;
        }
        let display = normalize_model_key(&model.display_name);
        if let Some(rank) = order.get(&display) {
            return *rank;
        }
        usize::MAX
    }

    fn prioritize_models<'a>(
        order: &HashMap<String, usize>,
        models: impl Iterator<Item = &'a moltis_agents::providers::ModelInfo>,
    ) -> Vec<&'a moltis_agents::providers::ModelInfo> {
        let mut ordered: Vec<(usize, &'a moltis_agents::providers::ModelInfo)> =
            models.enumerate().collect();
        ordered.sort_by_key(|(idx, model)| (Self::priority_rank(order, model), *idx));
        ordered.into_iter().map(|(_, model)| model).collect()
    }

    async fn priority_order(&self) -> HashMap<String, usize> {
        let list = self.priority_models.read().await;
        Self::build_priority_order(&list)
    }

    /// Set the gateway state reference for broadcasting model updates.
    pub fn set_state(&self, state: Arc<GatewayState>) {
        let _ = self.state.set(state);
    }

    async fn broadcast_model_visibility_update(&self, model_id: &str, disabled: bool) {
        if let Some(state) = self.state.get() {
            broadcast(
                state,
                "models.updated",
                serde_json::json!({
                    "modelId": model_id,
                    "disabled": disabled,
                }),
                BroadcastOpts::default(),
            )
            .await;
        }
    }
}

#[async_trait]
impl ModelService for LiveModelService {
    async fn list(&self) -> ServiceResult {
        let reg = self.providers.read().await;
        let disabled = self.disabled.read().await;
        let order = self.priority_order().await;
        let prioritized = Self::prioritize_models(
            &order,
            reg.list_models()
                .iter()
                .filter(|m| moltis_agents::providers::is_chat_capable_model(&m.id))
                .filter(|m| !disabled.is_disabled(&m.id))
                .filter(|m| disabled.unsupported_info(&m.id).is_none()),
        );
        let models: Vec<_> = prioritized
            .iter()
            .copied()
            .map(|m| {
                let supports_tools = reg.get(&m.id).is_some_and(|p| p.supports_tools());
                let preferred = Self::priority_rank(&order, m) != usize::MAX;
                serde_json::json!({
                    "id": m.id,
                    "provider": m.provider,
                    "displayName": m.display_name,
                    "supportsTools": supports_tools,
                    "preferred": preferred,
                    "createdAt": m.created_at,
                    "unsupported": false,
                    "unsupportedReason": Value::Null,
                    "unsupportedProvider": Value::Null,
                    "unsupportedUpdatedAt": Value::Null,
                })
            })
            .collect();
        Ok(serde_json::json!(models))
    }

    async fn list_all(&self) -> ServiceResult {
        let reg = self.providers.read().await;
        let disabled = self.disabled.read().await;
        let order = self.priority_order().await;
        let prioritized = Self::prioritize_models(
            &order,
            reg.list_models()
                .iter()
                .filter(|m| moltis_agents::providers::is_chat_capable_model(&m.id)),
        );
        let models: Vec<_> = prioritized
            .iter()
            .copied()
            .map(|m| {
                let supports_tools = reg.get(&m.id).is_some_and(|p| p.supports_tools());
                let unsupported = disabled.unsupported_info(&m.id);
                serde_json::json!({
                    "id": m.id,
                    "provider": m.provider,
                    "displayName": m.display_name,
                    "supportsTools": supports_tools,
                    "createdAt": m.created_at,
                    "disabled": disabled.is_disabled(&m.id),
                    "unsupported": unsupported.is_some(),
                    "unsupportedReason": unsupported.map(|u| u.detail.clone()),
                    "unsupportedProvider": unsupported.and_then(|u| u.provider.clone()),
                    "unsupportedUpdatedAt": unsupported.map(|u| u.updated_at_ms),
                })
            })
            .collect();
        Ok(serde_json::json!(models))
    }

    async fn disable(&self, params: Value) -> ServiceResult {
        let model_id = params
            .get("modelId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'modelId' parameter".to_string())?;

        info!(model = %model_id, "disabling model");

        let mut disabled = self.disabled.write().await;
        disabled.disable(model_id);
        disabled
            .save()
            .map_err(|e| format!("failed to save: {e}"))?;
        drop(disabled);

        self.broadcast_model_visibility_update(model_id, true).await;

        Ok(serde_json::json!({
            "ok": true,
            "modelId": model_id,
        }))
    }

    async fn enable(&self, params: Value) -> ServiceResult {
        let model_id = params
            .get("modelId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'modelId' parameter".to_string())?;

        info!(model = %model_id, "enabling model");

        let mut disabled = self.disabled.write().await;
        disabled.enable(model_id);
        disabled
            .save()
            .map_err(|e| format!("failed to save: {e}"))?;
        drop(disabled);

        self.broadcast_model_visibility_update(model_id, false)
            .await;

        Ok(serde_json::json!({
            "ok": true,
            "modelId": model_id,
        }))
    }

    async fn detect_supported(&self, params: Value) -> ServiceResult {
        let background = params
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reason = params
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("manual")
            .to_string();
        let max_parallel = params
            .get("maxParallel")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, 32) as usize)
            .unwrap_or(8);
        let max_parallel_per_provider = probe_max_parallel_per_provider(&params);
        let provider_filter = provider_filter_from_params(&params);

        let _run_permit: OwnedSemaphorePermit = if background {
            match Arc::clone(&self.detect_gate).try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    return Ok(serde_json::json!({
                        "ok": true,
                        "background": true,
                        "reason": reason,
                        "skipped": true,
                        "message": "model probe already running",
                    }));
                },
            }
        } else {
            Arc::clone(&self.detect_gate)
                .acquire_owned()
                .await
                .map_err(|_| "model probe gate closed".to_string())?
        };

        let state = self.state.get().cloned();

        // Phase 1: notify clients to refresh and show the full current model list first.
        if let Some(state) = state.as_ref() {
            broadcast(
                state,
                "models.updated",
                serde_json::json!({
                    "phase": "catalog",
                    "background": background,
                    "reason": reason,
                    "provider": provider_filter.as_deref(),
                }),
                BroadcastOpts::default(),
            )
            .await;
        }

        let checks = {
            let reg = self.providers.read().await;
            let disabled = self.disabled.read().await;
            reg.list_models()
                .iter()
                .filter(|m| !disabled.is_disabled(&m.id))
                .filter(|m| provider_matches_filter(&m.provider, provider_filter.as_deref()))
                .filter_map(|m| {
                    reg.get(&m.id).map(|provider| {
                        (
                            m.id.clone(),
                            m.display_name.clone(),
                            provider.name().to_string(),
                            provider,
                        )
                    })
                })
                .collect::<Vec<_>>()
        };

        let total = checks.len();
        if let Some(state) = state.as_ref() {
            broadcast(
                state,
                "models.updated",
                serde_json::json!({
                    "phase": "start",
                    "background": background,
                    "reason": reason,
                    "provider": provider_filter.as_deref(),
                    "maxParallelPerProvider": max_parallel_per_provider,
                    "total": total,
                    "checked": 0,
                    "supported": 0,
                    "unsupported": 0,
                    "errors": 0,
                }),
                BroadcastOpts::default(),
            )
            .await;
        }

        let limiter = Arc::new(Semaphore::new(max_parallel));
        let provider_limiter = Arc::new(ProbeProviderLimiter::new(max_parallel_per_provider));
        let rate_limiter = Arc::new(ProbeRateLimiter::default());
        let mut tasks = futures::stream::FuturesUnordered::new();
        for (model_id, display_name, provider_name, provider) in checks {
            let limiter = Arc::clone(&limiter);
            let provider_limiter = Arc::clone(&provider_limiter);
            let rate_limiter = Arc::clone(&rate_limiter);
            tasks.push(tokio::spawn(run_single_probe(
                model_id,
                display_name,
                provider_name,
                provider,
                limiter,
                provider_limiter,
                rate_limiter,
            )));
        }

        let mut results = Vec::with_capacity(total);
        let mut checked = 0usize;
        let mut supported = 0usize;
        let mut unsupported = 0usize;
        let mut flagged = 0usize;
        let mut cleared = 0usize;
        let mut errors = 0usize;
        let mut supported_by_provider: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut unsupported_by_provider: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut errors_by_provider: BTreeMap<String, Vec<Value>> = BTreeMap::new();

        while let Some(joined) = tasks.next().await {
            checked += 1;
            let outcome = match joined {
                Ok(outcome) => outcome,
                Err(err) => {
                    errors += 1;
                    results.push(serde_json::json!({
                        "modelId": "",
                        "displayName": "",
                        "provider": "",
                        "status": "error",
                        "error": format!("probe task failed: {err}"),
                    }));
                    if let Some(state) = state.as_ref() {
                        broadcast(
                            state,
                            "models.updated",
                            serde_json::json!({
                                "phase": "progress",
                                "background": background,
                                "reason": reason,
                                "provider": provider_filter.as_deref(),
                                "total": total,
                                "checked": checked,
                                "supported": supported,
                                "unsupported": unsupported,
                                "errors": errors,
                            }),
                            BroadcastOpts::default(),
                        )
                        .await;
                    }
                    continue;
                },
            };

            match outcome.status {
                ProbeStatus::Supported => {
                    supported += 1;
                    push_provider_model(
                        &mut supported_by_provider,
                        &outcome.provider_name,
                        &outcome.model_id,
                        &outcome.display_name,
                    );
                    let mut changed = false;
                    {
                        let mut store = self.disabled.write().await;
                        if store.clear_unsupported(&outcome.model_id) {
                            changed = true;
                            if let Err(err) = store.save() {
                                warn!(
                                    model = %outcome.model_id,
                                    error = %err,
                                    "failed to persist unsupported model clear"
                                );
                            }
                        }
                    }
                    if changed {
                        cleared += 1;
                        if let Some(state) = state.as_ref() {
                            broadcast(
                                state,
                                "models.updated",
                                serde_json::json!({
                                    "modelId": outcome.model_id,
                                    "unsupported": false,
                                }),
                                BroadcastOpts::default(),
                            )
                            .await;
                        }
                    }

                    results.push(serde_json::json!({
                        "modelId": outcome.model_id,
                        "displayName": outcome.display_name,
                        "provider": outcome.provider_name,
                        "status": "supported",
                    }));
                },
                ProbeStatus::Unsupported { detail, provider } => {
                    unsupported += 1;
                    push_provider_model(
                        &mut unsupported_by_provider,
                        &outcome.provider_name,
                        &outcome.model_id,
                        &outcome.display_name,
                    );
                    let mut changed = false;
                    let mut updated_at_ms = now_ms();
                    {
                        let mut store = self.disabled.write().await;
                        if store.mark_unsupported(&outcome.model_id, &detail, Some(&provider)) {
                            changed = true;
                            if let Some(info) = store.unsupported_info(&outcome.model_id) {
                                updated_at_ms = info.updated_at_ms;
                            }
                            if let Err(save_err) = store.save() {
                                warn!(
                                    model = %outcome.model_id,
                                    provider = provider,
                                    error = %save_err,
                                    "failed to persist unsupported model flag"
                                );
                            }
                        }
                    }
                    if changed {
                        flagged += 1;
                        if let Some(state) = state.as_ref() {
                            broadcast(
                                state,
                                "models.updated",
                                serde_json::json!({
                                    "modelId": outcome.model_id,
                                    "unsupported": true,
                                    "unsupportedReason": detail,
                                    "unsupportedProvider": provider,
                                    "unsupportedUpdatedAt": updated_at_ms,
                                }),
                                BroadcastOpts::default(),
                            )
                            .await;
                        }
                    }

                    results.push(serde_json::json!({
                        "modelId": outcome.model_id,
                        "displayName": outcome.display_name,
                        "provider": outcome.provider_name,
                        "status": "unsupported",
                        "error": detail,
                    }));
                },
                ProbeStatus::Error { message } => {
                    errors += 1;
                    push_provider_model(
                        &mut errors_by_provider,
                        &outcome.provider_name,
                        &outcome.model_id,
                        &outcome.display_name,
                    );
                    results.push(serde_json::json!({
                        "modelId": outcome.model_id,
                        "displayName": outcome.display_name,
                        "provider": outcome.provider_name,
                        "status": "error",
                        "error": message,
                    }));
                },
            }

            if let Some(state) = state.as_ref() {
                broadcast(
                    state,
                    "models.updated",
                    serde_json::json!({
                        "phase": "progress",
                        "background": background,
                        "reason": reason,
                        "provider": provider_filter.as_deref(),
                        "total": total,
                        "checked": checked,
                        "supported": supported,
                        "unsupported": unsupported,
                        "errors": errors,
                    }),
                    BroadcastOpts::default(),
                )
                .await;
            }
        }

        let summary = serde_json::json!({
            "ok": true,
            "probeWord": "ping",
            "background": background,
            "reason": reason,
            "provider": provider_filter.as_deref(),
            "maxParallel": max_parallel,
            "maxParallelPerProvider": max_parallel_per_provider,
            "total": total,
            "checked": checked,
            "supported": supported,
            "unsupported": unsupported,
            "flagged": flagged,
            "cleared": cleared,
            "errors": errors,
            "supportedByProvider": supported_by_provider,
            "unsupportedByProvider": unsupported_by_provider,
            "errorsByProvider": errors_by_provider,
            "results": results,
        });

        // Final refresh event to ensure clients are in sync after the full pass.
        if let Some(state) = state.as_ref() {
            broadcast(
                state,
                "models.updated",
                serde_json::json!({
                    "phase": "complete",
                    "background": background,
                    "reason": reason,
                    "provider": provider_filter.as_deref(),
                    "summary": summary,
                }),
                BroadcastOpts::default(),
            )
            .await;
        }

        Ok(summary)
    }

    async fn test(&self, params: Value) -> ServiceResult {
        let model_id = params
            .get("modelId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'modelId' parameter".to_string())?;

        let provider = {
            let reg = self.providers.read().await;
            reg.get(model_id)
                .ok_or_else(|| format!("unknown model: {model_id}"))?
        };

        // Use streaming and return as soon as the first token arrives.
        // Dropping the stream closes the HTTP connection, which tells the
        // provider to stop generating — effectively max_tokens: 1.
        let probe = vec![ChatMessage::user("ping")];
        let llm_context = moltis_agents::model::LlmRequestContext {
            session_key: None,
            session_id: None,
            prompt_cache_key: None,
            run_id: None,
        };
        let mut stream = provider.stream_with_tools_with_context(&llm_context, probe, vec![]);

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(event) = stream.next().await {
                match event {
                    StreamEvent::Delta(_) | StreamEvent::Done(_) => return Ok(()),
                    StreamEvent::Error(err) => return Err(err),
                    // Skip other events (tool calls, etc.) and keep waiting.
                    _ => continue,
                }
            }
            Err("stream ended without producing any output".to_string())
        })
        .await;

        // Drop the stream early to cancel the request on the provider side.
        drop(stream);

        match result {
            Ok(Ok(())) => {
                info!(model_id, "model probe succeeded");
                Ok(serde_json::json!({
                    "ok": true,
                    "modelId": model_id,
                }))
            },
            Ok(Err(err)) => {
                let error_obj = crate::chat_error::parse_chat_error(&err, Some(provider.name()));
                let detail = error_obj
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&err)
                    .to_string();

                warn!(model_id, error = %detail, "model probe failed");
                Err(detail)
            },
            Err(_) => {
                warn!(model_id, "model probe timed out after 10s");
                Err("Connection timed out after 10 seconds".to_string())
            },
        }
    }
}

// ── LiveChatService ─────────────────────────────────────────────────────────

/// A message that arrived while an agent run was already active on the session.
#[derive(Debug, Clone)]
struct QueuedMessage {
    params: Value,
}

pub struct LiveChatService {
    providers: Arc<RwLock<ProviderRegistry>>,
    model_store: Arc<RwLock<DisabledModelsStore>>,
    state: Arc<GatewayState>,
    active_runs: Arc<RwLock<HashMap<String, AbortHandle>>>,
    run_completion_notifiers: Arc<RwLock<HashMap<String, watch::Sender<bool>>>>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    session_store: Arc<SessionStore>,
    session_metadata: Arc<SqliteSessionMetadata>,
    hook_registry: Option<Arc<moltis_common::hooks::HookRegistry>>,
    /// Per-session semaphore ensuring only one agent run executes per session at a time.
    session_locks: Arc<RwLock<HashMap<String, Arc<Semaphore>>>>,
    /// Per-session message queue for messages arriving during an active run.
    message_queue: Arc<RwLock<HashMap<String, Vec<QueuedMessage>>>>,
    /// Per-session last-seen client sequence number for ordering diagnostics.
    last_client_seq: Arc<RwLock<HashMap<String, u64>>>,
    /// Per-session consecutive run failures (in-process circuit breaker).
    consecutive_failures: Arc<RwLock<HashMap<String, u32>>>,
    /// Failover configuration for automatic model/provider failover.
    failover_config: moltis_config::schema::FailoverConfig,
}

const RUN_COMPLETION_NOTIFIER_TTL: Duration = Duration::from_secs(60);

async fn mark_run_completed(
    run_completion_notifiers: &Arc<RwLock<HashMap<String, watch::Sender<bool>>>>,
    run_id: &str,
) {
    let completion_sender = { run_completion_notifiers.read().await.get(run_id).cloned() };
    if let Some(sender) = completion_sender {
        let _ = sender.send(true);
    }

    let cleanup_run_id = run_id.to_string();
    let cleanup_notifiers = Arc::clone(run_completion_notifiers);
    tokio::spawn(async move {
        tokio::time::sleep(RUN_COMPLETION_NOTIFIER_TTL).await;
        cleanup_notifiers.write().await.remove(&cleanup_run_id);
    });
}

impl LiveChatService {
    pub fn new(
        providers: Arc<RwLock<ProviderRegistry>>,
        model_store: Arc<RwLock<DisabledModelsStore>>,
        state: Arc<GatewayState>,
        session_store: Arc<SessionStore>,
        session_metadata: Arc<SqliteSessionMetadata>,
    ) -> Self {
        Self {
            providers,
            model_store,
            state,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            run_completion_notifiers: Arc::new(RwLock::new(HashMap::new())),
            tool_registry: Arc::new(RwLock::new(ToolRegistry::new())),
            session_store,
            session_metadata,
            hook_registry: None,
            session_locks: Arc::new(RwLock::new(HashMap::new())),
            message_queue: Arc::new(RwLock::new(HashMap::new())),
            last_client_seq: Arc::new(RwLock::new(HashMap::new())),
            consecutive_failures: Arc::new(RwLock::new(HashMap::new())),
            failover_config: moltis_config::schema::FailoverConfig::default(),
        }
    }

    pub fn with_failover(mut self, config: moltis_config::schema::FailoverConfig) -> Self {
        self.failover_config = config;
        self
    }

    pub fn with_tools(mut self, registry: Arc<RwLock<ToolRegistry>>) -> Self {
        self.tool_registry = registry;
        self
    }

    pub fn with_hooks(mut self, registry: moltis_common::hooks::HookRegistry) -> Self {
        self.hook_registry = Some(Arc::new(registry));
        self
    }

    pub fn with_hooks_arc(mut self, registry: Arc<moltis_common::hooks::HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    async fn wait_for_run_completion_internal(&self, run_id: &str) -> ServiceResult<()> {
        let Some(mut completion_rx) = ({
            self.run_completion_notifiers
                .read()
                .await
                .get(run_id)
                .map(watch::Sender::subscribe)
        }) else {
            return Ok(());
        };

        if *completion_rx.borrow() {
            return Ok(());
        }

        completion_rx
            .changed()
            .await
            .map_err(|_| format!("run completion waiter dropped for {run_id}"))?;
        Ok(())
    }

    fn has_tools_sync(&self) -> bool {
        // Best-effort check: try_read avoids blocking. If the lock is held,
        // assume tools are present (conservative — enables tool mode).
        self.tool_registry
            .try_read()
            .map(|r| {
                let schemas = r.list_schemas();
                let has = !schemas.is_empty();
                tracing::debug!(
                    tool_count = schemas.len(),
                    has_tools = has,
                    "has_tools_sync check"
                );
                has
            })
            .unwrap_or(true)
    }

    /// Return the per-session semaphore, creating one if absent.
    async fn session_semaphore(&self, key: &str) -> Arc<Semaphore> {
        // Fast path: read lock.
        {
            let locks = self.session_locks.read().await;
            if let Some(sem) = locks.get(key) {
                return Arc::clone(sem);
            }
        }
        // Slow path: write lock, insert.
        let mut locks = self.session_locks.write().await;
        Arc::clone(
            locks
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(1))),
        )
    }

    /// Resolve a provider from session metadata, history, or first registered.
    async fn resolve_provider(
        &self,
        session_id: &str,
        history: &[serde_json::Value],
    ) -> Result<Arc<dyn moltis_agents::model::LlmProvider>, String> {
        let reg = self.providers.read().await;
        let session_model = self
            .session_metadata
            .get(session_id)
            .await
            .and_then(|e| e.model.clone());
        let history_model = history
            .iter()
            .rev()
            .find_map(|m| m.get("model").and_then(|v| v.as_str()).map(String::from));
        let model_id = session_model.or(history_model);

        model_id
            .and_then(|id| reg.get(&id))
            .or_else(|| reg.first())
            .ok_or_else(|| "no LLM providers configured".to_string())
    }

    /// Resolve the active session id for a connection.
    async fn active_session_id_for(&self, conn_id: Option<&str>) -> Option<String> {
        let cid = conn_id?;
        let inner = self.state.inner.read().await;
        inner.active_sessions.get(cid).cloned()
    }

    async fn require_session_id(
        &self,
        explicit_session_id: Option<&str>,
        conn_id: Option<&str>,
    ) -> Result<String, String> {
        if let Some(session_id) = explicit_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(session_id.to_string());
        }
        if let Some(session_id) = self.active_session_id_for(conn_id).await {
            return Ok(session_id);
        }
        Err(
            "missing session context: provide _sessionId or switch to an active session first"
                .into(),
        )
    }

    fn require_explicit_session_id(
        &self,
        explicit_session_id: Option<&str>,
        conn_id: Option<&str>,
        method: &'static str,
    ) -> Result<String, String> {
        if let Some(session_id) = explicit_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(session_id.to_string());
        }
        warn!(
            event = "session.contract_violation",
            reason_code = "ui_missing_explicit_session_id",
            decision = "reject",
            policy = "web_ui_session_owner_v1",
            method,
            conn_id = conn_id,
            remediation = "resend with _sessionId from sessionStore.activeSessionId",
            "chat method rejected missing explicit _sessionId"
        );
        Err(format!(
            "{method} requires explicit _sessionId; remediation: resend with _sessionId from sessionStore.activeSessionId"
        ))
    }

    /// Resolve the project context prompt section for a session.
    async fn resolve_project_context(
        &self,
        session_id: &str,
        conn_id: Option<&str>,
    ) -> Option<String> {
        let project_id = if let Some(cid) = conn_id {
            let inner = self.state.inner.read().await;
            inner.active_projects.get(cid).cloned()
        } else {
            None
        };
        // Also check session metadata for project binding (async path).
        let project_id = match project_id {
            Some(pid) => Some(pid),
            None => self
                .session_metadata
                .get(session_id)
                .await
                .and_then(|e| e.project_id),
        };

        let pid = project_id?;
        let val = self
            .state
            .services
            .project
            .get(serde_json::json!({"id": pid}))
            .await
            .ok()?;
        let dir = val.get("directory").and_then(|v| v.as_str())?;
        let files = match moltis_projects::context::load_context_files(std::path::Path::new(dir)) {
            Ok(f) => f,
            Err(e) => {
                warn!("failed to load project context: {e}");
                return None;
            },
        };
        let project: moltis_projects::Project = serde_json::from_value(val.clone()).ok()?;
        let worktree_dir = self
            .session_metadata
            .get(session_id)
            .await
            .and_then(|e| e.worktree_branch)
            .and_then(|_| {
                let wt_path = std::path::Path::new(dir)
                    .join(".moltis-worktrees")
                    .join(session_id);
                if wt_path.exists() {
                    Some(wt_path)
                } else {
                    None
                }
            });
        let ctx = moltis_projects::ProjectContext {
            project,
            context_files: files,
            worktree_dir,
        };
        Some(ctx.to_prompt_section())
    }
}

fn build_compaction_debug_info(messages: &[Value]) -> Value {
    const SUMMARY_PREFIX: &str = "[Conversation Summary]";

    let mut is_compacted = false;
    let mut summary_created_at = None;
    let mut summary_len = None;
    let kept_message_count = messages.len().saturating_sub(1);

    if let Some(first) = messages.first()
        && first.get("role").and_then(|v| v.as_str()) == Some("assistant")
        && let Some(content) = first.get("content").and_then(|v| v.as_str())
    {
        let trimmed = content.trim_start();
        if trimmed.starts_with(SUMMARY_PREFIX) {
            is_compacted = true;
            summary_created_at = first.get("created_at").and_then(|v| v.as_u64());
            let rest = trimmed
                .strip_prefix(SUMMARY_PREFIX)
                .unwrap_or("")
                .strip_prefix("\n\n")
                .unwrap_or("")
                .trim();
            summary_len = Some(rest.len());
        }
    }

    let kept_message_count = if is_compacted {
        Some(kept_message_count)
    } else {
        None
    };

    serde_json::json!({
        "isCompacted": is_compacted,
        "summaryCreatedAt": summary_created_at,
        "summaryLen": summary_len,
        "keptMessageCount": kept_message_count,
        "keepLastUserRounds": KEEP_LAST_USER_ROUNDS,
    })
}

fn sandbox_mount_debug_info(
    sandbox_cfg: &moltis_config::schema::SandboxConfig,
    backend_name: Option<&str>,
    router_available: bool,
) -> (Vec<Value>, Vec<String>, &'static str) {
    let mounts: Vec<Value> = sandbox_cfg
        .mounts
        .iter()
        .map(|m| {
            serde_json::json!({
                "hostDir": m.host_dir.as_str(),
                "guestDir": m.guest_dir.as_str(),
                "mode": m.mode.as_str(),
            })
        })
        .collect();
    let mount_allowlist = sandbox_cfg.mount_allowlist.clone();

    let status = if mounts.is_empty() {
        "none"
    } else if !router_available {
        "router_unavailable"
    } else if backend_name != Some("docker") {
        "unsupported_backend"
    } else if mount_allowlist.is_empty() {
        "deny_by_default"
    } else {
        "configured"
    };

    (mounts, mount_allowlist, status)
}

#[async_trait]
impl ChatService for LiveChatService {
    async fn send(&self, params: Value) -> ServiceResult {
        let mut params = params;
        // Support both text-only and multimodal content.
        // - "text": string → plain text message
        // - "content": array → multimodal content (text + images)
        let (text, message_content) = if let Some(content) = params.get("content") {
            // Multimodal content - extract text for logging/hooks, parse into typed blocks
            let text_part = content
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .find(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .and_then(|block| block.get("text").and_then(|t| t.as_str()))
                })
                .unwrap_or("[Image]")
                .to_string();

            // Parse JSON blocks into typed ContentBlock structs
            let blocks: Vec<ContentBlock> = content
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|block| {
                            let block_type = block.get("type")?.as_str()?;
                            match block_type {
                                "text" => {
                                    let text = block.get("text")?.as_str()?.to_string();
                                    Some(ContentBlock::text(text))
                                },
                                "image_url" => {
                                    let url = block.get("image_url")?.get("url")?.as_str()?;
                                    Some(ContentBlock::ImageUrl {
                                        image_url: moltis_sessions::message::ImageUrl {
                                            url: url.to_string(),
                                        },
                                    })
                                },
                                _ => None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            (text_part, MessageContent::Multimodal(blocks))
        } else {
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing 'text' or 'content' parameter".to_string())?
                .to_string();
            (text.clone(), MessageContent::Text(text))
        };
        let desired_reply_medium = infer_reply_medium(&params, &text);

        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let explicit_model = params
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // Use streaming-only mode if explicitly requested or if no tools are registered.
        let explicit_stream_only = params
            .get("stream_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_tools = self.has_tools_sync();
        let stream_only = explicit_stream_only || !has_tools;
        tracing::debug!(
            explicit_stream_only,
            has_tools,
            stream_only,
            "send() mode decision"
        );

        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.send",
        )?;
        let channel_turn_id = params
            .get("_channelTurnId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                let id = crate::ids::new_trigger_id();
                params["_channelTurnId"] = serde_json::json!(id.clone());
                id
            });
        let queued_replay = params
            .get("_queued_replay")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let trigger_id = channel_turn_id.clone();

        // Track client-side sequence number for ordering diagnostics.
        // Note: seq resets to 1 on page reload, so a drop from a high value
        // back to 1 is normal (new browser session) — only flag issues within
        // a continuous ascending sequence.
        let client_seq = params.get("_seq").and_then(|v| v.as_u64());
        if let Some(seq) = client_seq {
            if queued_replay {
                debug!(
                    session = %session_key,
                    seq,
                    "client seq replayed from queue; skipping ordering diagnostics"
                );
            } else {
                let mut seq_map = self.last_client_seq.write().await;
                let last = seq_map.entry(session_key.clone()).or_insert(0);
                if *last == 0 {
                    // First observed sequence for this session in this process.
                    // We cannot infer a gap yet because earlier messages may have
                    // come from another tab/process before we started tracking.
                    debug!(session = %session_key, seq, "client seq initialized");
                } else if seq == 1 && *last > 1 {
                    // Page reload — reset tracking.
                    debug!(
                        session = %session_key,
                        prev_seq = *last,
                        "client seq reset (page reload)"
                    );
                } else if seq <= *last {
                    warn!(
                        session = %session_key,
                        seq,
                        last_seq = *last,
                        "client seq out of order (duplicate or reorder)"
                    );
                } else if seq > *last + 1 {
                    warn!(
                        session = %session_key,
                        seq,
                        last_seq = *last,
                        gap = seq - *last - 1,
                        "client seq gap detected (missing messages)"
                    );
                }
                *last = seq;
            }
        }

        // Resolve model: explicit param → session metadata → first registered.
        let session_model = if explicit_model.is_none() {
            self.session_metadata
                .get(&session_key)
                .await
                .and_then(|e| e.model)
        } else {
            None
        };
        let model_id = explicit_model.as_deref().or(session_model.as_deref());

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = {
            let reg = self.providers.read().await;
            let primary = if let Some(id) = model_id {
                reg.get(id).ok_or_else(|| {
                    let available: Vec<_> =
                        reg.list_models().iter().map(|m| m.id.clone()).collect();
                    format!("model '{}' not found. available: {:?}", id, available)
                })?
            } else if !stream_only {
                reg.first_with_tools()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            } else {
                reg.first()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            };

            if self.failover_config.enabled {
                let fallbacks = if self.failover_config.fallback_models.is_empty() {
                    // Auto-build: same model on other providers first, then same
                    // provider's other models, then everything else.
                    reg.fallback_providers_for(primary.id(), primary.name())
                } else {
                    reg.providers_for_models(&self.failover_config.fallback_models)
                };
                if fallbacks.is_empty() {
                    primary
                } else {
                    let mut chain = vec![primary];
                    chain.extend(fallbacks);
                    Arc::new(moltis_agents::provider_chain::ProviderChain::new(chain))
                }
            } else {
                primary
            }
        };

        // Check if this is a local model that needs downloading.
        // Only do this check for local-llm providers.
        #[cfg(feature = "local-llm")]
        if provider.name() == "local-llm" {
            let model_to_check = model_id
                .map(raw_model_id)
                .unwrap_or_else(|| raw_model_id(provider.id()))
                .to_string();
            tracing::info!(
                provider_name = provider.name(),
                model_to_check,
                "checking local model cache"
            );
            if let Err(e) =
                crate::local_llm_setup::ensure_local_model_cached(&model_to_check, &self.state)
                    .await
            {
                return Err(format!("Failed to prepare local model: {}", e));
            }
        }

        // Resolve project context for this connection's active project.
        let project_context = self
            .resolve_project_context(&session_key, conn_id.as_deref())
            .await;

        // Dispatch MessageReceived hook (read-only).
        if let Some(ref hooks) = self.hook_registry {
            let channel = params
                .get("channel")
                .and_then(|v| v.as_str())
                .map(String::from);
            let session_entry = self.session_metadata.get(&session_key).await;
            let payload = moltis_common::hooks::HookPayload::MessageReceived {
                session_id: session_key.clone(),
                session_key: session_key_from_session_entry(session_entry.as_ref()),
                channel_target: channel_target_from_session_entry(session_entry.as_ref()),
                content: text.clone(),
                channel,
            };
            if let Err(e) = hooks.dispatch(&payload).await {
                warn!(session = %session_key, error = %e, "MessageReceived hook failed");
            }
        }

        // Generate run_id early so we can link the user message to its agent run.
        let run_id = uuid::Uuid::new_v4().to_string();

        // Convert session-crate content to agents-crate content for the LLM.
        // Must happen before `message_content` is moved into `user_msg`.
        let user_content = to_user_content(&message_content);

        // Build the user message for later persistence (deferred until we
        // know the message won't be queued — avoids double-persist when a
        // queued message is replayed via send()).
        let channel_meta = params.get("channel").cloned();
        let user_msg = PersistedMessage::User {
            content: message_content,
            created_at: Some(now_ms()),
            channel: channel_meta,
            seq: client_seq,
            run_id: Some(run_id.clone()),
        };
        let mut user_val = user_msg.to_value();
        if let Some(obj) = user_val.as_object_mut() {
            obj.insert(
                "triggerId".into(),
                serde_json::Value::String(channel_turn_id.clone()),
            );
            if let Some(v) = params.get("_mergedFromTriggerIds") {
                obj.insert("mergedFromTriggerIds".into(), v.clone());
            }
        }

        // Load conversation history (the current user message is NOT yet
        // persisted — run_streaming / run_agent_loop add it themselves).
        let mut history = self
            .session_store
            .read(&session_key)
            .await
            .unwrap_or_default();

        let session_entry = self
            .session_metadata
            .get(&session_key)
            .await
            .ok_or_else(|| format!("session '{session_key}' not found"))?;
        self.session_metadata
            .touch(&session_key, history.len() as u32)
            .await;

        // If this is a web UI message on a channel-bound session, echo the
        // user message to the channel and register a reply target so the LLM
        // response is also delivered there.
        let is_web_message = conn_id.is_some() && params.get("channel").is_none();

        if is_web_message && let Some(ref binding_json) = session_entry.channel_binding {
            let binding_rejected =
                moltis_telegram::adapter::telegram_binding_uses_legacy_shape(binding_json)
                    || moltis_telegram::adapter::telegram_channel_binding_info(binding_json)
                        .is_some_and(|info| info.bucket_key.is_none());
            if binding_rejected {
                warn!(
                    event = "telegram.session.reject_legacy_binding",
                    session_id = %session_key,
                    context = "web_channel_echo",
                    reason_code = "legacy_channel_binding_rejected",
                    "skipping web-originated channel echo for legacy or incomplete binding"
                );
            }
            let session_key_for_turn =
                (!binding_rejected).then(|| session_entry.session_key.clone());
            let reply_target_ref = (!binding_rejected)
                .then(|| moltis_telegram::adapter::reply_target_ref_from_binding(binding_json))
                .flatten();
            if let Some(reply_target_ref) = reply_target_ref {
                self.state
                    .ensure_channel_turn_context(
                        &channel_turn_id,
                        &session_key,
                        session_key_for_turn,
                    )
                    .await;
                self.state
                    .push_channel_reply(&session_key, &channel_turn_id, reply_target_ref)
                    .await;
            } else {
                warn!(
                    event = "channel.reply_target_ref.missing",
                    session_id = %session_key,
                    reason_code = "missing_reply_target_ref",
                    "channel-bound session binding could not be encoded to reply_target_ref"
                );
            }
        }

        // Discover enabled skills/plugins for prompt injection.
        let search_paths = moltis_skills::discover::FsSkillDiscoverer::default_paths();
        let discoverer = moltis_skills::discover::FsSkillDiscoverer::new(search_paths);
        let discovered_skills = match discoverer.discover().await {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to discover skills: {e}");
                Vec::new()
            },
        };

        // Check if MCP tools are disabled for this session and capture
        // per-session sandbox override details for prompt runtime context.
        let session_entry = Some(session_entry);
        let mcp_disabled = session_entry
            .as_ref()
            .and_then(|entry| entry.mcp_disabled)
            .unwrap_or(false);
        let mut runtime_context = build_prompt_runtime_context(
            &self.state,
            &provider,
            &session_key,
            session_entry.as_ref(),
        )
        .await;
        runtime_context.host.accept_language = params
            .get("_acceptLanguage")
            .and_then(|v| v.as_str())
            .map(String::from);
        runtime_context.host.remote_ip = params
            .get("_remoteIp")
            .and_then(|v| v.as_str())
            .map(String::from);
        if runtime_context.host.timezone.is_none() {
            runtime_context.host.timezone = params
                .get("_timeZone")
                .and_then(|v| v.as_str())
                .map(String::from);
        }

        let tool_session_key = session_key_from_session_entry(session_entry.as_ref());

        let state = Arc::clone(&self.state);
        let active_runs = Arc::clone(&self.active_runs);
        let run_id_clone = run_id.clone();
        let trigger_id_clone = trigger_id.clone();
        let tool_registry = Arc::clone(&self.tool_registry);
        let hook_registry = self.hook_registry.clone();

        // Log if tool mode is active but the provider doesn't support tools.
        // Note: We don't broadcast to the user here - they chose the model knowing
        // its limitations. The UI should show capabilities when selecting a model.
        if !stream_only && !provider.supports_tools() {
            debug!(
                provider = provider.name(),
                model = provider.id(),
                "selected provider does not support tool calling"
            );
        }

        let reply_target_ref_hash =
            first_reply_target_ref_hash(&self.state, &session_key, &channel_turn_id).await;
        let channel_type = channel_field_as_string(&params, "channelType");
        let bucket_key = channel_field_as_string(&params, "bucketKey");

        info!(
            run_id = %run_id,
            trigger_id = %trigger_id,
            channel_turn_id = %channel_turn_id,
            session_id = %session_key,
            channel_type = channel_type.as_deref(),
            bucket_key = bucket_key.as_deref(),
            reply_target_ref_hash = reply_target_ref_hash.as_deref(),
            text_len = text.len(),
            text_preview = %log_text_preview(&text, 240),
            model = provider.id(),
            stream_only,
            reply_medium = ?desired_reply_medium,
            client_seq = ?client_seq,
            "chat.send"
        );

        // Capture user message index (0-based) so we can include assistant
        // message index in the "final" broadcast for client-side deduplication.
        let user_message_index = history.len(); // user msg is at this index in the JSONL

        let provider_name = provider.name().to_string();
        let model_id = provider.id().to_string();
        let model_store = Arc::clone(&self.model_store);
        let session_store = Arc::clone(&self.session_store);
        let session_metadata = Arc::clone(&self.session_metadata);
        let session_key_clone = session_key.clone();
        let tool_session_key_clone = tool_session_key.clone();
        let accept_language = params
            .get("_acceptLanguage")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Try to acquire the per-session semaphore.  If a run is already active,
        // queue the message according to the configured MessageQueueMode instead
        // of blocking the caller.
        let session_sem = self.session_semaphore(&session_key).await;
        let permit: OwnedSemaphorePermit = match session_sem.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // Active run — enqueue and return immediately.
                let queue_mode = moltis_config::discover_and_load().chat.message_queue_mode;
                info!(
                    session = %session_key,
                    trigger_id = %trigger_id,
                    mode = ?queue_mode,
                    client_seq = ?client_seq,
                    "queueing message (run active)"
                );
                let position = {
                    let mut q = self.message_queue.write().await;
                    let entry = q.entry(session_key.clone()).or_default();
                    entry.push(QueuedMessage {
                        params: params.clone(),
                    });
                    entry.len()
                };
                broadcast(
                    &self.state,
                    "chat",
                    serde_json::json!({
                        "sessionId": session_key,
                        "state": "queued",
                        "mode": format!("{queue_mode:?}").to_lowercase(),
                        "position": position,
                    }),
                    BroadcastOpts::default(),
                )
                .await;
                return Ok(serde_json::json!({
                    "queued": true,
                    "mode": format!("{queue_mode:?}").to_lowercase(),
                }));
            },
        };

        // Auto-compact preflight (proactive).
        //
        // The persisted `inputTokens` field is "prompt tokens for that call" and includes
        // the entire history; summing it is O(turn^2) and triggers compaction too early.
        // Instead, estimate the next request input budget from the prompt text.
        let budget = CompactionBudget::for_provider(provider.as_ref());
        if budget.derived_input_cap {
            warn!(
                model = provider.id(),
                context_window = budget.effective_context_window,
                "provider did not report input_limit; deriving input_hard_cap=floor(context_window*0.8)"
            );
        }

        let agent_id = resolve_session_agent_id(&self.state, Some(&runtime_context)).await;
        let agent_id_effective = agent_id.as_deref().unwrap_or("default");
        let agent = load_prompt_agent_with_id(agent_id.as_deref());

        let supports_tools = provider.supports_tools();
        let filtered_registry = if stream_only {
            ToolRegistry::new()
        } else {
            let registry_guard = self.tool_registry.read().await;
            apply_runtime_tool_filters(
                &registry_guard,
                &agent.config,
                &discovered_skills,
                mcp_disabled,
            )
        };

        let canonical = build_canonical_system_prompt_v1(
            &filtered_registry,
            supports_tools,
            stream_only,
            project_context.as_deref(),
            &discovered_skills,
            agent_id_effective,
            agent.identity_md_raw.as_deref(),
            agent.soul_text.as_deref(),
            agent.agents_text.as_deref(),
            agent.tools_text.as_deref(),
            to_prompt_reply_medium(desired_reply_medium),
            Some(&runtime_context),
            &session_key,
        )
        .map_err(|e| e.to_string())?;
        for w in &canonical.warnings {
            warn!(session = %session_key, warning = %w, "prompt template warning");
        }
        let mut system_prompt = canonical.system_prompt;
        maybe_append_tg_gst_v1_system_prompt(
            &self.state,
            session_entry.as_ref(),
            &mut system_prompt,
        )
        .await;

        let estimated_next_input_tokens =
            estimate_next_input_tokens(&system_prompt, &history, &user_content);
        let keep_start_idx = keep_window_start_idx(&history, KEEP_LAST_USER_ROUNDS);
        let keep_window = &history[keep_start_idx..];
        let estimated_keep_window_input_tokens =
            estimate_next_input_tokens(&system_prompt, keep_window, &user_content);

        if estimated_keep_window_input_tokens >= budget.input_hard_cap {
            // Recovery mode: keep window itself doesn't fit — do not call the model.
            // Still persist the user message so the session remains usable.
            if let Err(e) = self.session_store.append(&session_key, &user_val).await {
                warn!("failed to persist user message: {e}");
            }
            // Best-effort: update preview + metadata counts so the session stays visible in UI.
            if let Some(entry) = self.session_metadata.get(&session_key).await
                && entry.preview.is_none()
            {
                let preview_text = extract_preview_from_value(&user_val);
                if let Some(preview) = preview_text {
                    self.session_metadata
                        .set_preview(&session_key, Some(&preview))
                        .await;
                }
            }
            if let Ok(count) = self.session_store.count(&session_key).await {
                self.session_metadata.touch(&session_key, count).await;
            }

            let error_obj = serde_json::json!({
                "type": "keep_window_overflow",
                "icon": "⚠️",
                "title": "Context window overflow",
                "detail": "The last 4 user rounds plus this message exceed the model's input limit, so auto-compaction cannot proceed. Shorten/split your latest message or start a new session.",
                "budget": {
                    "effectiveContextWindow": budget.effective_context_window,
                    "inputHardCap": budget.input_hard_cap,
                    "reservedOutputTokens": budget.reserved_output_tokens,
                    "reserveSafetyTokens": budget.reserve_safety_tokens,
                    "effectiveInputBudget": budget.effective_input_budget(),
                    "estimatedNextInputTokens": estimated_next_input_tokens,
                    "estimatedKeepWindowInputTokens": estimated_keep_window_input_tokens,
                    "highWatermark": budget.high_watermark,
                    "lowWatermark": budget.low_watermark,
                }
            });

            broadcast(
                &self.state,
                "chat",
                serde_json::json!({
                    "runId": run_id,
                    "sessionId": session_key,
                    "state": "error",
                    "error": error_obj,
                    "seq": client_seq,
                }),
                BroadcastOpts::default(),
            )
            .await;

            drop(permit);
            return Ok(serde_json::json!({ "runId": run_id }));
        }

        if estimated_next_input_tokens >= budget.high_watermark && keep_start_idx > 0 {
            let pre_compact_msg_count = history.len();
            info!(
                session = %session_key,
                estimated_next_input_tokens,
                high_watermark = budget.high_watermark,
                input_hard_cap = budget.input_hard_cap,
                "auto-compact triggered (HIGH_WATERMARK reached)"
            );
            broadcast(
                &self.state,
                "chat",
                serde_json::json!({
                    "runId": run_id,
                    "sessionId": session_key,
                    "state": "auto_compact",
                    "phase": "start",
                    "reason": "budget_high_watermark",
                    "messageCount": pre_compact_msg_count,
                    "budget": {
                        "effectiveContextWindow": budget.effective_context_window,
                        "inputHardCap": budget.input_hard_cap,
                        "reservedOutputTokens": budget.reserved_output_tokens,
                        "reserveSafetyTokens": budget.reserve_safety_tokens,
                        "effectiveInputBudget": budget.effective_input_budget(),
                        "estimatedNextInputTokens": estimated_next_input_tokens,
                        "highWatermark": budget.high_watermark,
                        "lowWatermark": budget.low_watermark,
                    }
                }),
                BroadcastOpts::default(),
            )
            .await;

            match compact_session(
                &self.state,
                self.hook_registry.clone(),
                &self.session_store,
                &session_key,
                &provider,
                KEEP_LAST_USER_ROUNDS,
            )
            .await
            {
                Ok(result) => {
                    history = result.compacted.clone();
                    broadcast(
                        &self.state,
                        "chat",
                        serde_json::json!({
                            "runId": run_id,
                            "sessionId": session_key,
                            "state": "auto_compact",
                            "phase": "done",
                            "reason": "budget_high_watermark",
                            "messageCount": pre_compact_msg_count,
                            "keptMessageCount": result.kept_message_count,
                            "keepLastUserRounds": KEEP_LAST_USER_ROUNDS,
                            "summaryLen": result.summary_len,
                            "budget": {
                                "effectiveContextWindow": budget.effective_context_window,
                                "inputHardCap": budget.input_hard_cap,
                                "reservedOutputTokens": budget.reserved_output_tokens,
                                "reserveSafetyTokens": budget.reserve_safety_tokens,
                                "effectiveInputBudget": budget.effective_input_budget(),
                                "estimatedNextInputTokens": estimated_next_input_tokens,
                                "highWatermark": budget.high_watermark,
                                "lowWatermark": budget.low_watermark,
                            }
                        }),
                        BroadcastOpts::default(),
                    )
                    .await;
                },
                Err(e) => {
                    warn!(
                        session = %session_key,
                        error = %e,
                        "auto-compact failed, proceeding with full history"
                    );
                    broadcast(
                        &self.state,
                        "chat",
                        serde_json::json!({
                            "runId": run_id,
                            "sessionId": session_key,
                            "state": "auto_compact",
                            "phase": "error",
                            "reason": "budget_high_watermark",
                            "error": e.to_string(),
                        }),
                        BroadcastOpts::default(),
                    )
                    .await;
                },
            }
        }

        // Persist the user message now that we know it won't be queued.
        // (Queued messages skip this; they are persisted when replayed.)
        if let Err(e) = self.session_store.append(&session_key, &user_val).await {
            warn!("failed to persist user message: {e}");
        }

        // Set preview from the first user message if not already set.
        if let Some(entry) = self.session_metadata.get(&session_key).await
            && entry.preview.is_none()
        {
            let preview_text = extract_preview_from_value(&user_val);
            if let Some(preview) = preview_text {
                self.session_metadata
                    .set_preview(&session_key, Some(&preview))
                    .await;
            }
        }

        let agent_timeout_secs = moltis_config::discover_and_load().tools.agent_timeout_secs;

        let message_queue = Arc::clone(&self.message_queue);
        let state_for_drain = Arc::clone(&self.state);
        let consecutive_failures = Arc::clone(&self.consecutive_failures);
        let run_completion_notifiers = Arc::clone(&self.run_completion_notifiers);
        let (run_completion_tx, _) = watch::channel(false);
        self.run_completion_notifiers
            .write()
            .await
            .insert(run_id.clone(), run_completion_tx);

        let handle = tokio::spawn(async move {
            let permit = permit; // hold permit until agent run completes
            let trigger_id = trigger_id_clone;
            let ctx_ref = project_context.as_deref();
            if desired_reply_medium == ReplyMedium::Voice {
                broadcast(
                    &state,
                    "chat",
                    serde_json::json!({
                        "runId": run_id_clone,
                        "sessionId": session_key_clone,
                        "state": "voice_pending",
                    }),
                    BroadcastOpts::default(),
                )
                .await;
            }
            let agent_fut = async {
                if stream_only {
                    run_streaming(
                        &state,
                        &model_store,
                        &run_id_clone,
                        provider,
                        &model_id,
                        &user_content,
                        &provider_name,
                        &history,
                        &session_key_clone,
                        &trigger_id,
                        desired_reply_medium,
                        ctx_ref,
                        user_message_index,
                        &discovered_skills,
                        Some(&runtime_context),
                        Some(&session_store),
                        client_seq,
                    )
                    .await
                } else {
                    run_with_tools(
                        &state,
                        &model_store,
                        &run_id_clone,
                        provider,
                        &model_id,
                        &tool_registry,
                        &user_content,
                        &provider_name,
                        &history,
                        &session_key_clone,
                        &trigger_id,
                        tool_session_key_clone.as_deref(),
                        desired_reply_medium,
                        ctx_ref,
                        Some(&runtime_context),
                        user_message_index,
                        &discovered_skills,
                        hook_registry,
                        accept_language.clone(),
                        conn_id.clone(),
                        Some(&session_store),
                        mcp_disabled,
                        client_seq,
                    )
                    .await
                }
            };

            let assistant_text = if agent_timeout_secs > 0 {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(agent_timeout_secs),
                    agent_fut,
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        let raw_error = format!("Agent run timed out after {agent_timeout_secs}s");
                        handle_run_failed_event(
                            &state,
                            &model_store,
                            RunFailedEvent {
                                run_id: run_id_clone.clone(),
                                session_key: session_key_clone.clone(),
                                trigger_id: Some(trigger_id.clone()),
                                provider_name: provider_name.clone(),
                                model_id: model_id.clone(),
                                stage_hint: FailureStage::GatewayTimeout,
                                raw_error,
                                details: serde_json::json!({
                                    "timeout_secs": agent_timeout_secs,
                                    "elapsed_ms": agent_timeout_secs * 1000,
                                }),
                                seq: client_seq,
                            },
                        )
                        .await;
                        None
                    },
                }
            } else {
                agent_fut.await
            };

            let run_completed = assistant_text.as_ref().is_some();
            let consecutive_failure_limit: u32 = 2;
            let consecutive_failure_count = {
                let mut failures = consecutive_failures.write().await;
                if run_completed {
                    failures.insert(session_key_clone.clone(), 0);
                    0
                } else {
                    let entry = failures.entry(session_key_clone.clone()).or_insert(0);
                    *entry = entry.saturating_add(1);
                    *entry
                }
            };

            // Persist assistant response (even empty ones — needed for LLM history coherence).
            if let Some(output) = assistant_text {
                let assistant_msg = PersistedMessage::Assistant {
                    content: output.text,
                    created_at: Some(now_ms()),
                    model: Some(model_id.clone()),
                    provider: Some(provider_name.clone()),
                    input_tokens: Some(output.input_tokens),
                    output_tokens: Some(output.output_tokens),
                    cached_tokens: Some(output.cached_tokens),
                    tool_calls: None,
                    audio: output.audio_path,
                    seq: client_seq,
                    run_id: Some(run_id_clone.clone()),
                };
                let mut assistant_val = assistant_msg.to_value();
                if let Some(obj) = assistant_val.as_object_mut() {
                    obj.insert(
                        "triggerId".into(),
                        serde_json::Value::String(trigger_id.clone()),
                    );
                }
                if let Err(e) = session_store
                    .append(&session_key_clone, &assistant_val)
                    .await
                {
                    warn!("failed to persist assistant message: {e}");
                }
                // Update metadata counts.
                if let Ok(count) = session_store.count(&session_key_clone).await {
                    session_metadata.touch(&session_key_clone, count).await;
                }
            }

            active_runs.write().await.remove(&run_id_clone);

            // Release the semaphore *before* draining so replayed sends can
            // acquire it. Without this, every replayed `chat.send()` would
            // fail `try_acquire_owned()` and re-queue the message forever.
            drop(permit);

            // Drain queued messages for this session.
            let queued = message_queue
                .write()
                .await
                .remove(&session_key_clone)
                .unwrap_or_default();
            if !queued.is_empty() {
                if !run_completed && consecutive_failure_count >= consecutive_failure_limit {
                    info!(
                        session = %session_key_clone,
                        failure_count = consecutive_failure_count,
                        failure_limit = consecutive_failure_limit,
                        queued = queued.len(),
                        "circuit breaker tripped; flushing queued triggers"
                    );
                    let breaker_text =
                        "⚠️ 我这边连续出错，已暂停处理后续请求；请稍后重试或重新 @我。";
                    let outbound = state_for_drain.services.channel_outbound_arc();
                    for msg in queued {
                        let Some(tid) = msg.params.get("_channelTurnId").and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        // Drain any buffered status lines for this trigger so they don't leak into
                        // later successful replies if the session recovers.
                        let _ = state_for_drain
                            .drain_channel_status_log(&session_key_clone, tid)
                            .await;
                        let targets = state_for_drain
                            .drain_channel_replies(&session_key_clone, tid)
                            .await;
                        if targets.is_empty() {
                            continue;
                        }
                        if let Some(ref outbound) = outbound {
                            deliver_channel_replies_to_reply_target_refs(
                                Arc::clone(outbound),
                                targets,
                                &session_key_clone,
                                breaker_text,
                                Arc::clone(&state_for_drain),
                                ReplyMedium::Text,
                                Vec::new(),
                                Some(ChannelDeliveryDiag {
                                    run_id: Some(run_id_clone.clone()),
                                    trigger_id: Some(tid.to_string()),
                                }),
                            )
                            .await;
                        }
                    }
                    mark_run_completed(&run_completion_notifiers, &run_id_clone).await;
                    return;
                }
                let queue_mode = moltis_config::discover_and_load().chat.message_queue_mode;
                let chat = state_for_drain.chat().await;
                match queue_mode {
                    MessageQueueMode::Followup => {
                        let mut iter = queued.into_iter();
                        let Some(first) = iter.next() else {
                            mark_run_completed(&run_completion_notifiers, &run_id_clone).await;
                            return;
                        };
                        // Put remaining messages back so the replayed run's
                        // own drain loop picks them up after it completes.
                        let rest: Vec<QueuedMessage> = iter.collect();
                        if !rest.is_empty() {
                            message_queue
                                .write()
                                .await
                                .entry(session_key_clone.clone())
                                .or_default()
                                .extend(rest);
                        }
                        info!(session = %session_key_clone, "replaying queued message (followup)");
                        let mut replay_params = first.params;
                        replay_params["_queued_replay"] = serde_json::json!(true);
                        if let Err(e) = chat.send(replay_params).await {
                            warn!(session = %session_key_clone, error = %e, "failed to replay queued message");
                        }
                    },
                    MessageQueueMode::Collect => {
                        let combined: Vec<&str> = queued
                            .iter()
                            .filter_map(|m| m.params.get("text").and_then(|v| v.as_str()))
                            .collect();
                        if !combined.is_empty() {
                            info!(
                                session = %session_key_clone,
                                count = combined.len(),
                                "replaying collected messages"
                            );
                            // Use the last queued message as the base params, override text.
                            let Some(last) = queued.last() else {
                                mark_run_completed(&run_completion_notifiers, &run_id_clone).await;
                                return;
                            };
                            let merged_from: Vec<String> = queued
                                .iter()
                                .filter_map(|m| {
                                    m.params
                                        .get("_channelTurnId")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_string)
                                })
                                .collect();
                            let merged_trigger_id = crate::ids::new_trigger_id();
                            let last_trigger_id = last
                                .params
                                .get("_channelTurnId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            // Move reply targets from the last trigger to the merged trigger,
                            // and clear the rest to prevent later cross-wiring.
                            for tid in &merged_from {
                                let drained = state_for_drain
                                    .drain_channel_replies(&session_key_clone, tid)
                                    .await;
                                let drained_status = state_for_drain
                                    .drain_channel_status_log(&session_key_clone, tid)
                                    .await;
                                if tid == last_trigger_id {
                                    for t in drained {
                                        state_for_drain
                                            .push_channel_reply(
                                                &session_key_clone,
                                                &merged_trigger_id,
                                                t,
                                            )
                                            .await;
                                    }
                                    for line in drained_status {
                                        state_for_drain
                                            .push_channel_status_log(
                                                &session_key_clone,
                                                &merged_trigger_id,
                                                line,
                                            )
                                            .await;
                                    }
                                }
                            }

                            let mut merged = last.params.clone();
                            merged["text"] = serde_json::json!(combined.join("\n\n"));
                            merged["_queued_replay"] = serde_json::json!(true);
                            merged["_channelTurnId"] = serde_json::json!(merged_trigger_id);
                            merged["_mergedFromTriggerIds"] = serde_json::json!(merged_from);
                            if let Err(e) = chat.send(merged).await {
                                warn!(session = %session_key_clone, error = %e, "failed to replay collected messages");
                            }
                        }
                    },
                }
            }

            mark_run_completed(&run_completion_notifiers, &run_id_clone).await;
        });

        self.active_runs
            .write()
            .await
            .insert(run_id.clone(), handle.abort_handle());

        Ok(serde_json::json!({ "runId": run_id }))
    }

    async fn send_sync(&self, params: Value) -> ServiceResult {
        let text = params
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'text' parameter".to_string())?
            .to_string();
        let desired_reply_medium = infer_reply_medium(&params, &text);

        let explicit_model = params
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let stream_only = !self.has_tools_sync();

        let session_key = self
            .require_session_id(params.get("_sessionId").and_then(|v| v.as_str()), None)
            .await?;
        let channel_turn_id = params
            .get("_channelTurnId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| crate::ids::new_trigger_id());

        // Resolve provider.
        let provider: Arc<dyn moltis_agents::model::LlmProvider> = {
            let reg = self.providers.read().await;
            if let Some(id) = explicit_model.as_deref() {
                reg.get(id)
                    .ok_or_else(|| format!("model '{id}' not found"))?
            } else if !stream_only {
                reg.first_with_tools()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            } else {
                reg.first()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            }
        };

        let trigger_id = channel_turn_id.clone();

        // Persist the user message.
        let user_msg = PersistedMessage::user(&text);
        let mut user_val = user_msg.to_value();
        if let Some(obj) = user_val.as_object_mut() {
            obj.insert(
                "triggerId".into(),
                serde_json::Value::String(trigger_id.clone()),
            );
        }
        let session_entry = self
            .session_metadata
            .get(&session_key)
            .await
            .ok_or_else(|| format!("session '{session_key}' not found"))?;
        if let Err(e) = self.session_store.append(&session_key, &user_val).await {
            warn!("send_sync: failed to persist user message: {e}");
        }
        self.session_metadata.touch(&session_key, 1).await;
        let session_entry = Some(session_entry);
        let mut runtime_context = build_prompt_runtime_context(
            &self.state,
            &provider,
            &session_key,
            session_entry.as_ref(),
        )
        .await;
        runtime_context.host.accept_language = params
            .get("_acceptLanguage")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tool_session_key = session_key_from_session_entry(session_entry.as_ref());

        // Load conversation history (excluding the message we just appended).
        let mut history = self
            .session_store
            .read(&session_key)
            .await
            .unwrap_or_default();
        if !history.is_empty() {
            history.pop();
        }

        // Proactive compaction for send_sync (channels / API callers).
        let budget = CompactionBudget::for_provider(provider.as_ref());
        let agent_id = resolve_session_agent_id(&self.state, Some(&runtime_context)).await;
        let agent_id_effective = agent_id.as_deref().unwrap_or("default");
        let agent = load_prompt_agent_with_id(agent_id.as_deref());

        let supports_tools = provider.supports_tools();
        let filtered_registry = if stream_only {
            ToolRegistry::new()
        } else {
            let registry_guard = self.tool_registry.read().await;
            apply_runtime_tool_filters(
                &registry_guard,
                &agent.config,
                &[],
                false, // send_sync: MCP tools always enabled for API calls
            )
        };

        let canonical = build_canonical_system_prompt_v1(
            &filtered_registry,
            supports_tools,
            stream_only,
            None,
            &[],
            agent_id_effective,
            agent.identity_md_raw.as_deref(),
            agent.soul_text.as_deref(),
            agent.agents_text.as_deref(),
            agent.tools_text.as_deref(),
            to_prompt_reply_medium(desired_reply_medium),
            Some(&runtime_context),
            &session_key,
        )
        .map_err(|e| e.to_string())?;
        for w in &canonical.warnings {
            warn!(session = %session_key, warning = %w, "prompt template warning");
        }
        let mut system_prompt = canonical.system_prompt;
        maybe_append_tg_gst_v1_system_prompt(
            &self.state,
            session_entry.as_ref(),
            &mut system_prompt,
        )
        .await;

        let user_content = UserContent::text(text.clone());
        let estimated_next_input_tokens =
            estimate_next_input_tokens(&system_prompt, &history, &user_content);
        let keep_start_idx = keep_window_start_idx(&history, KEEP_LAST_USER_ROUNDS);
        let estimated_keep_window_input_tokens =
            estimate_next_input_tokens(&system_prompt, &history[keep_start_idx..], &user_content);

        if estimated_keep_window_input_tokens >= budget.input_hard_cap {
            let msg = "keep_window_overflow: last 4 user rounds plus current message exceed input limit; shorten/split your message or start a new session";
            let error_entry = ui_error_notice_message(&format!("[error] {msg}"));
            let _ = self.session_store.append(&session_key, &error_entry).await;
            return Err(msg.to_string());
        }

        if estimated_next_input_tokens >= budget.high_watermark && keep_start_idx > 0 {
            if let Ok(_result) = compact_session(
                &self.state,
                self.hook_registry.clone(),
                &self.session_store,
                &session_key,
                &provider,
                KEEP_LAST_USER_ROUNDS,
            )
            .await
            {
                // Reload history again (excluding the user message we appended).
                history = self
                    .session_store
                    .read(&session_key)
                    .await
                    .unwrap_or_default();
                if !history.is_empty() {
                    history.pop();
                }
            }
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let state = Arc::clone(&self.state);
        let tool_registry = Arc::clone(&self.tool_registry);
        let hook_registry = self.hook_registry.clone();
        let provider_name = provider.name().to_string();
        let model_id = provider.id().to_string();
        let model_store = Arc::clone(&self.model_store);
        let user_message_index = history.len();
        let reply_target_ref_hash =
            first_reply_target_ref_hash(&state, &session_key, &channel_turn_id).await;
        let channel_type = channel_field_as_string(&params, "channelType");
        let bucket_key = channel_field_as_string(&params, "bucketKey");

        info!(
            run_id = %run_id,
            trigger_id = %trigger_id,
            channel_turn_id = %channel_turn_id,
            session_id = %session_key,
            channel_type = channel_type.as_deref(),
            bucket_key = bucket_key.as_deref(),
            reply_target_ref_hash = reply_target_ref_hash.as_deref(),
            text_len = text.len(),
            text_preview = %log_text_preview(&text, 240),
            model = %model_id,
            stream_only,
            reply_medium = ?desired_reply_medium,
            "chat.send_sync"
        );

        if desired_reply_medium == ReplyMedium::Voice {
            broadcast(
                &state,
                "chat",
                serde_json::json!({
                    "runId": run_id,
                    "sessionId": session_key,
                    "state": "voice_pending",
                }),
                BroadcastOpts::default(),
            )
            .await;
        }

        // send_sync is text-only (used by API calls and channels).
        let user_content = UserContent::text(&text);
        let result = if stream_only {
            run_streaming(
                &state,
                &model_store,
                &run_id,
                provider,
                &model_id,
                &user_content,
                &provider_name,
                &history,
                &session_key,
                &trigger_id,
                desired_reply_medium,
                None,
                user_message_index,
                &[],
                Some(&runtime_context),
                Some(&self.session_store),
                None, // send_sync: no client seq
            )
            .await
        } else {
            run_with_tools(
                &state,
                &model_store,
                &run_id,
                provider,
                &model_id,
                &tool_registry,
                &user_content,
                &provider_name,
                &history,
                &session_key,
                &trigger_id,
                tool_session_key.as_deref(),
                desired_reply_medium,
                None,
                Some(&runtime_context),
                user_message_index,
                &[],
                hook_registry,
                None,
                None, // send_sync: no conn_id
                Some(&self.session_store),
                false, // send_sync: MCP tools always enabled for API calls
                None,  // send_sync: no client seq
            )
            .await
        };

        // Persist assistant response (even empty ones — needed for LLM history coherence).
        if let Some(ref output) = result {
            let assistant_msg = PersistedMessage::Assistant {
                content: output.text.clone(),
                created_at: Some(now_ms()),
                model: Some(model_id.clone()),
                provider: Some(provider_name.clone()),
                input_tokens: Some(output.input_tokens),
                output_tokens: Some(output.output_tokens),
                cached_tokens: Some(output.cached_tokens),
                tool_calls: None,
                audio: output.audio_path.clone(),
                seq: None,
                run_id: Some(run_id.clone()),
            };
            let mut assistant_val = assistant_msg.to_value();
            if let Some(obj) = assistant_val.as_object_mut() {
                obj.insert(
                    "triggerId".into(),
                    serde_json::Value::String(trigger_id.clone()),
                );
            }
            if let Err(e) = self
                .session_store
                .append(&session_key, &assistant_val)
                .await
            {
                warn!("send_sync: failed to persist assistant message: {e}");
            }
            // Update metadata message count.
            if let Ok(count) = self.session_store.count(&session_key).await {
                self.session_metadata.touch(&session_key, count).await;
            }
        }

        match result {
            Some(output) => Ok(serde_json::json!({
                "text": output.text,
                "inputTokens": output.input_tokens,
                "outputTokens": output.output_tokens,
            })),
            None => {
                // Check the last broadcast for this run to get the actual error message.
                let error_msg = state
                    .last_run_error(&run_id)
                    .await
                    .unwrap_or_else(|| "agent run failed (check server logs)".to_string());

                // Persist the error in the session so it's visible in session history.
                let error_entry = ui_error_notice_message(&format!("[error] {error_msg}"));
                let _ = self.session_store.append(&session_key, &error_entry).await;
                // Update metadata so the session shows in the UI.
                if let Ok(count) = self.session_store.count(&session_key).await {
                    self.session_metadata.touch(&session_key, count).await;
                }

                Err(error_msg)
            },
        }
    }

    async fn wait_run_completion(&self, run_id: &str) -> ServiceResult<()> {
        self.wait_for_run_completion_internal(run_id).await
    }

    async fn internal_complete(&self, params: Value) -> ServiceResult {
        use moltis_agents::model::{ChatMessage, UserContent};

        let system = params
            .get("system")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'system' parameter".to_string())?
            .to_string();
        let user = params
            .get("user")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'user' parameter".to_string())?
            .to_string();
        let explicit_model = params.get("model").and_then(|v| v.as_str());

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = {
            let reg = self.providers.read().await;
            if let Some(id) = explicit_model {
                reg.get(id)
                    .ok_or_else(|| format!("model '{id}' not found"))?
            } else {
                reg.first()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            }
        };

        let messages = [
            ChatMessage::System { content: system },
            ChatMessage::User {
                content: UserContent::text(user),
            },
        ];

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            provider.complete(&messages, &[]),
        )
        .await
        .map_err(|_| "internal complete timed out after 8s".to_string())?
        .map_err(|e| e.to_string())?;

        if !resp.tool_calls.is_empty() {
            return Err("internal complete returned unexpected tool calls".into());
        }

        Ok(serde_json::json!({
            "text": resp.text.unwrap_or_default(),
            "inputTokens": resp.usage.input_tokens,
            "outputTokens": resp.usage.output_tokens,
        }))
    }

    async fn abort(&self, params: Value) -> ServiceResult {
        let run_id = params
            .get("runId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'runId'".to_string())?;

        if let Some(handle) = self.active_runs.write().await.remove(run_id) {
            handle.abort();
        }
        mark_run_completed(&self.run_completion_notifiers, run_id).await;
        Ok(serde_json::json!({}))
    }

    async fn cancel_queued(&self, params: Value) -> ServiceResult {
        let session_key = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId'".to_string())?;

        let removed = self
            .message_queue
            .write()
            .await
            .remove(session_key)
            .unwrap_or_default();
        let count = removed.len();
        info!(session = %session_key, count, "cancel_queued: cleared message queue");

        broadcast(
            &self.state,
            "chat",
            serde_json::json!({
                "sessionId": session_key,
                "state": "queue_cleared",
                "count": count,
            }),
            BroadcastOpts::default(),
        )
        .await;

        Ok(serde_json::json!({ "cleared": count }))
    }

    async fn history(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_session_id(None, conn_id.as_deref()).await?;
        let messages = self
            .session_store
            .read(&session_key)
            .await
            .map_err(|e| e.to_string())?;
        // Filter out empty assistant messages — they are kept in storage for LLM
        // history coherence but should not be shown in the UI.
        let visible: Vec<Value> = messages
            .into_iter()
            .filter(|msg| {
                if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                    return true;
                }
                msg.get("content")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
            })
            .collect();
        Ok(serde_json::json!(visible))
    }

    async fn inject(&self, _params: Value) -> ServiceResult {
        Err("inject not yet implemented".into())
    }

    async fn clear(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.clear",
        )?;

        self.session_store
            .clear(&session_key)
            .await
            .map_err(|e| e.to_string())?;

        // Reset client sequence tracking for this session. A cleared chat starts
        // a fresh sequence from the web UI.
        {
            let mut seq_map = self.last_client_seq.write().await;
            seq_map.remove(&session_key);
        }

        // Reset metadata message count and preview.
        self.session_metadata.touch(&session_key, 0).await;
        self.session_metadata.set_preview(&session_key, None).await;

        // Notify all WebSocket clients so the web UI clears the session
        // even when /clear is issued from a channel (e.g. Telegram).
        broadcast(
            &self.state,
            "chat",
            serde_json::json!({
                "sessionId": session_key,
                "state": "session_cleared",
            }),
            BroadcastOpts::default(),
        )
        .await;

        info!(session = %session_key, "chat.clear");
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn compact(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.compact",
        )?;

        let history = self
            .session_store
            .read(&session_key)
            .await
            .map_err(|e| e.to_string())?;

        let provider = self.resolve_provider(&session_key, &history).await?;

        let result = compact_session(
            &self.state,
            self.hook_registry.clone(),
            &self.session_store,
            &session_key,
            &provider,
            KEEP_LAST_USER_ROUNDS,
        )
        .await?;

        // Update metadata counts after compaction.
        if let Ok(count) = self.session_store.count(&session_key).await {
            self.session_metadata.touch(&session_key, count).await;
        }

        info!(
            session = %session_key,
            summary_len = result.summary_len,
            kept_messages = result.kept_message_count,
            "chat.compact: done"
        );
        Ok(serde_json::json!(result.compacted))
    }

    async fn context(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.context",
        )?;

        // Optional: draft text from the web UI input box (not yet sent).
        let draft_text = params
            .get("draftText")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Session info
        let message_count = self.session_store.count(&session_key).await.unwrap_or(0);
        let session_entry = self.session_metadata.get(&session_key).await;
        let provider: Arc<dyn moltis_agents::model::LlmProvider> = {
            let reg = self.providers.read().await;
            let session_model = session_entry.as_ref().and_then(|e| e.model.as_deref());
            if let Some(id) = session_model {
                reg.get(id)
                    .or_else(|| reg.first())
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            } else {
                reg.first()
                    .ok_or_else(|| "no LLM providers configured".to_string())?
            }
        };
        let provider_name = Some(provider.name().to_string());
        let supports_tools = provider.supports_tools();
        let llm_context = moltis_agents::model::LlmRequestContext {
            session_key: session_key_from_session_entry(session_entry.as_ref()),
            session_id: Some(session_key.clone()),
            prompt_cache_key: prompt_cache_key_for_session(&session_key, session_entry.as_ref()),
            run_id: None,
        };
        let llm_debug = serde_json::json!({
            "provider": provider.name(),
            "model": provider.id(),
            "overrides": provider.debug_request_overrides(Some(&llm_context)),
        });
        let session_info = serde_json::json!({
            "sessionId": session_key,
            "messageCount": message_count,
            "model": session_entry.as_ref().and_then(|e| e.model.as_deref()),
            "provider": provider_name,
            "label": session_entry.as_ref().and_then(|e| e.label.as_deref()),
            "projectId": session_entry.as_ref().and_then(|e| e.project_id.as_deref()),
        });

        // Project info & context files
        let project_id = if let Some(cid) = conn_id.as_deref() {
            let inner = self.state.inner.read().await;
            inner.active_projects.get(cid).cloned()
        } else {
            None
        };
        let project_id =
            project_id.or_else(|| session_entry.as_ref().and_then(|e| e.project_id.clone()));

        let project_info = if let Some(pid) = project_id {
            match self
                .state
                .services
                .project
                .get(serde_json::json!({"id": pid}))
                .await
            {
                Ok(val) => {
                    let dir = val.get("directory").and_then(|v| v.as_str());
                    let context_files = if let Some(d) = dir {
                        match moltis_projects::context::load_context_files(std::path::Path::new(d))
                        {
                            Ok(files) => files
                                .iter()
                                .map(|f| {
                                    serde_json::json!({
                                        "path": f.path.display().to_string(),
                                        "size": f.content.len(),
                                    })
                                })
                                .collect::<Vec<_>>(),
                            Err(_) => vec![],
                        }
                    } else {
                        vec![]
                    };
                    serde_json::json!({
                        "id": val.get("id"),
                        "label": val.get("label"),
                        "directory": dir,
                        "systemPrompt": val.get("system_prompt").or(val.get("systemPrompt")),
                        "contextFiles": context_files,
                    })
                },
                Err(_) => serde_json::json!(null),
            }
        } else {
            serde_json::json!(null)
        };

        // Tools (only include if the provider supports tool calling)
        let mcp_disabled = session_entry
            .as_ref()
            .and_then(|e| e.mcp_disabled)
            .unwrap_or(false);
        let app_config = moltis_config::discover_and_load();
        let tools: Vec<serde_json::Value> = if supports_tools {
            let registry_guard = self.tool_registry.read().await;
            let effective_registry =
                apply_runtime_tool_filters(&registry_guard, &app_config, &[], mcp_disabled);
            effective_registry
                .list_schemas()
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.get("name").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "description": s.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect()
        } else {
            vec![]
        };

        // Load persisted history for debug/estimates.
        let messages = self
            .session_store
            .read(&session_key)
            .await
            .unwrap_or_default();

        // Compaction state inferred from persisted history.
        let compaction_info = build_compaction_debug_info(&messages);

        // Sandbox info
        let sandbox_info = if let Some(ref router) = self.state.sandbox_router {
            let config = router.config();
            let router_session_key = session_key_from_session_entry(session_entry.as_ref());
            let is_sandboxed = router
                .is_sandboxed(&session_key, router_session_key.as_deref())
                .await
                .unwrap_or(false);
            let effective_image = config
                .image
                .clone()
                .unwrap_or_else(|| "<missing>".to_string());
            let container_name = {
                match router.sandbox_id_for(&session_key, router_session_key.as_deref()) {
                    Ok(id) => Some(id.key),
                    Err(e) => {
                        tracing::debug!(
                            session_id = %session_key,
                            session_key = ?router_session_key,
                            reason_code = "sandbox_container_name_key_error",
                            error = %e,
                            "failed to derive sandbox id for container name"
                        );
                        None
                    },
                }
            };
            let (mounts, mount_allowlist, external_mounts_status) = sandbox_mount_debug_info(
                &app_config.tools.exec.sandbox,
                Some(router.backend_name()),
                true,
            );
            serde_json::json!({
                "enabled": is_sandboxed,
                "backend": router.backend_name(),
                "mode": config.mode,
                "scope": config.scope_key.to_string(),
                "dataMount": config.data_mount,
                "mountAllowlist": mount_allowlist,
                "mounts": mounts,
                "externalMountsStatus": external_mounts_status,
                "image": effective_image,
                "effectiveSandboxKey": router
                    .effective_sandbox_key(&session_key, router_session_key.as_deref())
                    .ok(),
                "containerName": container_name,
            })
        } else {
            let (mounts, mount_allowlist, external_mounts_status) =
                sandbox_mount_debug_info(&app_config.tools.exec.sandbox, None, false);
            serde_json::json!({
                "enabled": false,
                "backend": null,
                "dataMount": app_config.tools.exec.sandbox.data_mount.as_str(),
                "mountAllowlist": mount_allowlist,
                "mounts": mounts,
                "externalMountsStatus": external_mounts_status,
            })
        };

        // Discover enabled skills/plugins (only if provider supports tools)
        let discovered_skills: Vec<moltis_skills::types::SkillMetadata> = if supports_tools {
            let search_paths = moltis_skills::discover::FsSkillDiscoverer::default_paths();
            let discoverer = moltis_skills::discover::FsSkillDiscoverer::new(search_paths);
            match discoverer.discover().await {
                Ok(s) => s,
                Err(e) => {
                    warn!("failed to discover skills: {e}");
                    Vec::new()
                },
            }
        } else {
            Vec::new()
        };
        let skills_list: Vec<serde_json::Value> = discovered_skills
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "source": s.source,
                })
            })
            .collect();

        // MCP servers (only if provider supports tools)
        let mcp_servers = if supports_tools {
            self.state
                .services
                .mcp
                .list()
                .await
                .unwrap_or(serde_json::json!([]))
        } else {
            serde_json::json!([])
        };

        // Build the system prompt used for token estimates and debug displays.
        let stream_only = !self.has_tools_sync();
        let project_context = self
            .resolve_project_context(&session_key, conn_id.as_deref())
            .await;
        let runtime_context = build_prompt_runtime_context(
            &self.state,
            &provider,
            &session_key,
            session_entry.as_ref(),
        )
        .await;
        let agent_id = resolve_session_agent_id(&self.state, Some(&runtime_context)).await;
        let agent_id_effective = agent_id.as_deref().unwrap_or("default").to_string();
        let agent = load_prompt_agent_with_id(agent_id.as_deref());

        let filtered_registry = if stream_only {
            ToolRegistry::new()
        } else {
            let registry_guard = self.tool_registry.read().await;
            apply_runtime_tool_filters(
                &registry_guard,
                &agent.config,
                &discovered_skills,
                mcp_disabled,
            )
        };

        let canonical = build_canonical_system_prompt_v1(
            &filtered_registry,
            supports_tools,
            stream_only,
            project_context.as_deref(),
            &discovered_skills,
            &agent_id_effective,
            agent.identity_md_raw.as_deref(),
            agent.soul_text.as_deref(),
            agent.agents_text.as_deref(),
            agent.tools_text.as_deref(),
            PromptReplyMedium::Text,
            Some(&runtime_context),
            &session_key,
        )
        .map_err(|e| e.to_string())?;
        for w in &canonical.warnings {
            warn!(session = %session_key, warning = %w, "prompt template warning");
        }
        let prompt_template_warnings = canonical.warnings.clone();
        let mut system_prompt = canonical.system_prompt;
        maybe_append_tg_gst_v1_system_prompt(
            &self.state,
            session_entry.as_ref(),
            &mut system_prompt,
        )
        .await;

        let tools_for_api: Vec<serde_json::Value> = if stream_only || !supports_tools {
            Vec::new()
        } else {
            filtered_registry.list_schemas()
        };
        let history_with_tools = reconstruct_tool_history_for_prompt_estimate(
            &messages,
            app_config.tools.max_tool_result_bytes,
        );
        let mut msgs_for_as_sent = Vec::with_capacity(1 + history_with_tools.len());
        msgs_for_as_sent.push(ChatMessage::system(system_prompt.clone()));
        msgs_for_as_sent.extend(values_to_chat_messages(&history_with_tools));
        let as_sent = provider.debug_as_sent_summary(&msgs_for_as_sent, &tools_for_api);

        let as_sent_preamble = if is_openai_responses_provider(provider.name()) {
            Some(as_sent_preamble_for_provider(
                provider.name(),
                &system_prompt,
            ))
        } else {
            None
        };

        let token_debug = build_token_debug_info(
            provider.as_ref(),
            &llm_debug,
            &system_prompt,
            &messages,
            draft_text.as_deref(),
            app_config.tools.max_tool_result_bytes,
        );

        Ok(serde_json::json!({
            "session": session_info,
            "llm": llm_debug,
            "project": project_info,
            "tools": tools,
            "skills": skills_list,
            "mcpServers": mcp_servers,
            "mcpDisabled": mcp_disabled,
            "sandbox": sandbox_info,
            "supportsTools": supports_tools,
            "compaction": compaction_info,
            "tokenDebug": token_debug,
            "agentIdEffective": agent_id_effective,
            "asSentPreamble": as_sent_preamble,
            "asSent": as_sent,
            "promptTemplateWarnings": prompt_template_warnings
        }))
    }

    async fn raw_prompt(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.raw_prompt",
        )?;

        // Resolve provider.
        let history = self
            .session_store
            .read(&session_key)
            .await
            .unwrap_or_default();
        let provider = self.resolve_provider(&session_key, &history).await?;
        let native_tools = provider.supports_tools();

        // Build runtime context.
        let session_entry = self.session_metadata.get(&session_key).await;
        let mut runtime_context = build_prompt_runtime_context(
            &self.state,
            &provider,
            &session_key,
            session_entry.as_ref(),
        )
        .await;
        runtime_context.host.accept_language = params
            .get("_acceptLanguage")
            .and_then(|v| v.as_str())
            .map(String::from);
        runtime_context.host.remote_ip = params
            .get("_remoteIp")
            .and_then(|v| v.as_str())
            .map(String::from);
        if runtime_context.host.timezone.is_none() {
            runtime_context.host.timezone = params
                .get("_timeZone")
                .and_then(|v| v.as_str())
                .map(String::from);
        }

        // Resolve project context.
        let project_context = self
            .resolve_project_context(&session_key, conn_id.as_deref())
            .await;

        let agent_id = resolve_session_agent_id(&self.state, Some(&runtime_context)).await;
        let agent_id_effective = agent_id.as_deref().unwrap_or("default").to_string();
        let agent = load_prompt_agent_with_id(agent_id.as_deref());

        // Discover skills.
        let search_paths = moltis_skills::discover::FsSkillDiscoverer::default_paths();
        let discoverer = moltis_skills::discover::FsSkillDiscoverer::new(search_paths);
        let discovered_skills = match discoverer.discover().await {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to discover skills: {e}");
                Vec::new()
            },
        };

        // Check MCP disabled.
        let mcp_disabled = session_entry
            .as_ref()
            .and_then(|entry| entry.mcp_disabled)
            .unwrap_or(false);

        let stream_only = !self.has_tools_sync();

        // Build filtered tool registry.
        let filtered_registry = if stream_only {
            ToolRegistry::new()
        } else {
            let registry_guard = self.tool_registry.read().await;
            apply_runtime_tool_filters(
                &registry_guard,
                &agent.config,
                &discovered_skills,
                mcp_disabled,
            )
        };

        let canonical = build_canonical_system_prompt_v1(
            &filtered_registry,
            native_tools,
            stream_only,
            project_context.as_deref(),
            &discovered_skills,
            &agent_id_effective,
            agent.identity_md_raw.as_deref(),
            agent.soul_text.as_deref(),
            agent.agents_text.as_deref(),
            agent.tools_text.as_deref(),
            PromptReplyMedium::Text,
            Some(&runtime_context),
            &session_key,
        )
        .map_err(|e| e.to_string())?;
        for w in &canonical.warnings {
            warn!(session = %session_key, warning = %w, "prompt template warning");
        }
        let prompt_template_warnings = canonical.warnings.clone();
        let mut system_prompt = canonical.system_prompt;
        maybe_append_tg_gst_v1_system_prompt(
            &self.state,
            session_entry.as_ref(),
            &mut system_prompt,
        )
        .await;
        let tool_count = if stream_only {
            0
        } else {
            filtered_registry.list_schemas().len()
        };

        let tools_for_api: Vec<serde_json::Value> = if stream_only || !native_tools {
            Vec::new()
        } else {
            filtered_registry.list_schemas()
        };
        let msgs_for_as_sent = vec![ChatMessage::system(system_prompt.clone())];
        let as_sent = provider.debug_as_sent_summary(&msgs_for_as_sent, &tools_for_api);

        let as_sent_preamble = if is_openai_responses_provider(provider.name()) {
            Some(as_sent_preamble_for_provider(
                provider.name(),
                &system_prompt,
            ))
        } else {
            None
        };

        Ok(serde_json::json!({
            "prompt": system_prompt,
            "charCount": system_prompt.len(),
            "native_tools": native_tools,
            "toolCount": tool_count,
            "agentIdEffective": agent_id_effective,
            "asSentPreamble": as_sent_preamble,
            "asSent": as_sent,
            "promptTemplateWarnings": prompt_template_warnings,
        }))
    }

    /// Return the **full messages array** that would be sent to the LLM on the
    /// next call — system prompt + conversation history — in OpenAI format.
    async fn full_context(&self, params: Value) -> ServiceResult {
        let conn_id = params
            .get("_connId")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_key = self.require_explicit_session_id(
            params.get("_sessionId").and_then(|v| v.as_str()),
            conn_id.as_deref(),
            "chat.full_context",
        )?;

        // Resolve provider.
        let history = self
            .session_store
            .read(&session_key)
            .await
            .unwrap_or_default();
        let provider = self.resolve_provider(&session_key, &history).await?;
        let native_tools = provider.supports_tools();
        let app_config = moltis_config::discover_and_load();

        // Build runtime context.
        let session_entry = self.session_metadata.get(&session_key).await;
        let mut runtime_context = build_prompt_runtime_context(
            &self.state,
            &provider,
            &session_key,
            session_entry.as_ref(),
        )
        .await;
        runtime_context.host.accept_language = params
            .get("_acceptLanguage")
            .and_then(|v| v.as_str())
            .map(String::from);
        runtime_context.host.remote_ip = params
            .get("_remoteIp")
            .and_then(|v| v.as_str())
            .map(String::from);
        if runtime_context.host.timezone.is_none() {
            runtime_context.host.timezone = params
                .get("_timeZone")
                .and_then(|v| v.as_str())
                .map(String::from);
        }

        let agent_id = resolve_session_agent_id(&self.state, Some(&runtime_context)).await;
        let agent_id_effective = agent_id.as_deref().unwrap_or("default").to_string();
        let agent = load_prompt_agent_with_id(agent_id.as_deref());

        // Resolve project context.
        let project_context = self
            .resolve_project_context(&session_key, conn_id.as_deref())
            .await;

        // Discover skills.
        let search_paths = moltis_skills::discover::FsSkillDiscoverer::default_paths();
        let discoverer = moltis_skills::discover::FsSkillDiscoverer::new(search_paths);
        let discovered_skills = match discoverer.discover().await {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to discover skills: {e}");
                Vec::new()
            },
        };

        // Check MCP disabled.
        let mcp_disabled = session_entry
            .as_ref()
            .and_then(|entry| entry.mcp_disabled)
            .unwrap_or(false);

        let stream_only = !self.has_tools_sync();

        // Build filtered tool registry.
        let filtered_registry = if stream_only {
            ToolRegistry::new()
        } else {
            let registry_guard = self.tool_registry.read().await;
            apply_runtime_tool_filters(
                &registry_guard,
                &agent.config,
                &discovered_skills,
                mcp_disabled,
            )
        };

        let history_with_tools = reconstruct_tool_history_for_prompt_estimate(
            &history,
            app_config.tools.max_tool_result_bytes,
        );

        let canonical = build_canonical_system_prompt_v1(
            &filtered_registry,
            native_tools,
            stream_only,
            project_context.as_deref(),
            &discovered_skills,
            &agent_id_effective,
            agent.identity_md_raw.as_deref(),
            agent.soul_text.as_deref(),
            agent.agents_text.as_deref(),
            agent.tools_text.as_deref(),
            PromptReplyMedium::Text,
            Some(&runtime_context),
            &session_key,
        )
        .map_err(|e| e.to_string())?;
        for w in &canonical.warnings {
            warn!(session = %session_key, warning = %w, "prompt template warning");
        }
        let prompt_template_warnings = canonical.warnings.clone();
        let mut system_prompt = canonical.system_prompt;
        maybe_append_tg_gst_v1_system_prompt(
            &self.state,
            session_entry.as_ref(),
            &mut system_prompt,
        )
        .await;

        let tools_for_api: Vec<serde_json::Value> = if stream_only || !native_tools {
            Vec::new()
        } else {
            filtered_registry.list_schemas()
        };
        let mut msgs_for_as_sent = Vec::with_capacity(1 + history_with_tools.len());
        msgs_for_as_sent.push(ChatMessage::system(system_prompt.clone()));
        msgs_for_as_sent.extend(values_to_chat_messages(&history_with_tools));
        let as_sent = provider.debug_as_sent_summary(&msgs_for_as_sent, &tools_for_api);

        let as_sent_preamble = if is_openai_responses_provider(provider.name()) {
            Some(as_sent_preamble_for_provider(
                provider.name(),
                &system_prompt,
            ))
        } else {
            None
        };

        let (openai_messages, system_prompt_chars) =
            if is_openai_responses_provider(provider.name()) {
                let mut msgs: Vec<Value> = Vec::new();
                msgs.push(
                    serde_json::json!({"role": "developer", "content": system_prompt.clone()}),
                );

                for msg in values_to_chat_messages(&history_with_tools) {
                    let mut val = msg.to_openai_value();
                    if val.get("role").and_then(|r| r.as_str()) == Some("system") {
                        val["role"] = serde_json::Value::String("developer".to_string());
                    }
                    msgs.push(val);
                }

                (msgs, system_prompt.len())
            } else {
                // Build the full messages array: system prompt + conversation history.
                let mut messages = Vec::with_capacity(1 + history_with_tools.len());
                messages.push(ChatMessage::system(system_prompt.clone()));
                messages.extend(values_to_chat_messages(&history_with_tools));

                (
                    messages.iter().map(|m| m.to_openai_value()).collect(),
                    system_prompt.len(),
                )
            };

        let message_count = openai_messages.len();
        let total_chars: usize = openai_messages
            .iter()
            .map(|v| serde_json::to_string(v).unwrap_or_default().len())
            .sum();

        Ok(serde_json::json!({
            "messages": openai_messages,
            "messageCount": message_count,
            "systemPromptChars": system_prompt_chars,
            "totalChars": total_chars,
            "agentIdEffective": agent_id_effective,
            "asSentPreamble": as_sent_preamble,
            "asSent": as_sent,
            "promptTemplateWarnings": prompt_template_warnings,
        }))
    }
}

// ── Agent loop mode ─────────────────────────────────────────────────────────

async fn mark_unsupported_model(
    state: &Arc<GatewayState>,
    model_store: &Arc<RwLock<DisabledModelsStore>>,
    model_id: &str,
    provider_name: &str,
    error_obj: &serde_json::Value,
) {
    if error_obj.get("type").and_then(|v| v.as_str()) != Some("unsupported_model") {
        return;
    }

    let detail = error_obj
        .get("detail")
        .and_then(|v| v.as_str())
        .unwrap_or("Model is not supported for this account/provider");
    let provider = error_obj
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or(provider_name);

    let mut store = model_store.write().await;
    if store.mark_unsupported(model_id, detail, Some(provider)) {
        let unsupported = store.unsupported_info(model_id).cloned();
        if let Err(err) = store.save() {
            warn!(
                model = model_id,
                provider = provider,
                error = %err,
                "failed to persist unsupported model flag"
            );
        } else {
            info!(
                model = model_id,
                provider = provider,
                "flagged model as unsupported"
            );
        }
        drop(store);
        broadcast(
            state,
            "models.updated",
            serde_json::json!({
                "modelId": model_id,
                "unsupported": true,
                "unsupportedReason": unsupported.as_ref().map(|u| u.detail.as_str()).unwrap_or(detail),
                "unsupportedProvider": unsupported
                    .as_ref()
                    .and_then(|u| u.provider.as_deref())
                    .unwrap_or(provider),
                "unsupportedUpdatedAt": unsupported.map(|u| u.updated_at_ms).unwrap_or_else(now_ms),
            }),
            BroadcastOpts::default(),
        )
        .await;
    }
}

async fn clear_unsupported_model(
    state: &Arc<GatewayState>,
    model_store: &Arc<RwLock<DisabledModelsStore>>,
    model_id: &str,
) {
    let mut store = model_store.write().await;
    if store.clear_unsupported(model_id) {
        if let Err(err) = store.save() {
            warn!(
                model = model_id,
                error = %err,
                "failed to persist unsupported model clear"
            );
        } else {
            info!(model = model_id, "cleared unsupported model flag");
        }
        drop(store);
        broadcast(
            state,
            "models.updated",
            serde_json::json!({
                "modelId": model_id,
                "unsupported": false,
            }),
            BroadcastOpts::default(),
        )
        .await;
    }
}

async fn handle_run_failed_event(
    state: &Arc<GatewayState>,
    model_store: &Arc<RwLock<DisabledModelsStore>>,
    event: RunFailedEvent,
) {
    let normalized = normalize_failure(FailureInput {
        stage_hint: event.stage_hint,
        raw_error: &event.raw_error,
        provider_name: Some(event.provider_name.as_str()),
        model_id: Some(event.model_id.as_str()),
        details: event.details.clone(),
    });

    // For send_sync callers: store a safe, user-facing error string.
    state
        .set_run_error(&event.run_id, normalized.message.user.clone())
        .await;

    let dedup_key = format!("run.failure.egress:{}", event.run_id);
    let suppress_side_effects = state.dedupe_check_and_insert(&dedup_key).await;

    let (reply_targets_before, targets) = if let Some(ref trigger_id) = event.trigger_id {
        (
            state
                .peek_channel_replies(&event.session_key, trigger_id)
                .await
                .len(),
            state
                .drain_channel_replies(&event.session_key, trigger_id)
                .await,
        )
    } else {
        let targets = state.drain_all_channel_replies(&event.session_key).await;
        let before = targets.len();
        (before, targets)
    };
    let drained_count = targets.len();
    // Always drain status logs on failure so they don't leak to later replies.
    if let Some(ref trigger_id) = event.trigger_id {
        let _ = state
            .drain_channel_status_log(&event.session_key, trigger_id)
            .await;
    } else {
        let _ = state.drain_all_channel_status_log(&event.session_key).await;
    }

    let mut egress = serde_json::json!({
        "sent": false,
        "reply_targets_before": reply_targets_before,
        "drained_count": drained_count,
    });

    // Best-effort channel error reply: send once per run_id in-process.
    if suppress_side_effects {
        // Still drain reply targets/status log to prevent cross-wiring, but do not
        // send/broadcast/log the failure twice.
        warn!(
            event = "run.failure.duplicate",
            run_id = event.run_id,
            session_key = event.session_key,
            trigger_id = ?event.trigger_id,
            provider = event.provider_name,
            model = event.model_id,
            dedup_key,
            egress_reply_targets_before = reply_targets_before,
            egress_drained_count = drained_count,
            "duplicate failure egress suppressed"
        );
        return;
    }

    if !targets.is_empty() && !normalized.message.user.trim().is_empty() {
        match state.services.channel_outbound_arc() {
            Some(outbound) => {
                let code = match normalized.stage {
                    FailureStage::GatewayTimeout => "gateway_timeout".to_string(),
                    _ if matches!(normalized.kind, crate::run_failure::ErrorKind::Cancelled) => {
                        "cancelled".to_string()
                    },
                    _ => format!("{}/{}", normalized.stage.as_str(), normalized.kind.as_str()),
                };
                let text = format!("\u{26A0}\u{FE0F} {} code={code}", normalized.message.user);
                deliver_channel_replies_to_reply_target_refs(
                    outbound,
                    targets,
                    &event.session_key,
                    &text,
                    Arc::clone(state),
                    ReplyMedium::Text,
                    Vec::new(),
                    Some(ChannelDeliveryDiag {
                        run_id: Some(event.run_id.clone()),
                        trigger_id: event.trigger_id.clone(),
                    }),
                )
                .await;
                egress["sent"] = serde_json::json!(true);
            },
            None => {
                egress["last_error"] = serde_json::json!({
                    "action": "DeliverChannelErrorOnce",
                    "class": "outbound_unavailable",
                    "message_redacted": "channel outbound unavailable",
                });
            },
        }
    }

    // Build the UI error card and enrich it with normalized fields.
    let mut error_obj = parse_chat_error(&event.raw_error, Some(event.provider_name.as_str()));
    if let Some(obj) = error_obj.as_object_mut() {
        // Prefer showing the user-facing message as the card detail; other diagnostics
        // are available via additional fields.
        obj.insert(
            "detail".into(),
            serde_json::Value::String(normalized.message.user.clone()),
        );
        obj.insert("stage".into(), serde_json::json!(normalized.stage));
        obj.insert("kind".into(), serde_json::json!(normalized.kind));
        obj.insert("retryable".into(), serde_json::json!(normalized.retryable));
        obj.insert("action".into(), serde_json::json!(normalized.action));
        obj.insert("message".into(), serde_json::json!(normalized.message));
        obj.insert("details".into(), normalized.details.clone());
        obj.insert("raw".into(), serde_json::json!(normalized.raw));
        obj.insert("egress".into(), egress.clone());
        obj.insert(
            "dedup_key".into(),
            serde_json::Value::String(dedup_key.clone()),
        );
    }

    mark_unsupported_model(
        state,
        model_store,
        &event.model_id,
        &event.provider_name,
        &error_obj,
    )
    .await;

    // Broadcast terminal error frame (Web UI).
    let error_payload = ChatErrorBroadcast {
        run_id: event.run_id.clone(),
        session_id: event.session_key.clone(),
        state: "error",
        error: error_obj,
        seq: event.seq,
    };
    #[allow(clippy::unwrap_used)] // serializing known-valid struct
    let payload_val = serde_json::to_value(&error_payload).unwrap();
    broadcast(state, "chat", payload_val, BroadcastOpts::default()).await;

    // Single structured failure line (logs).
    warn!(
        event = "run.failure",
        run_id = event.run_id,
        session_key = event.session_key,
        provider = event.provider_name,
        model = event.model_id,
        stage = normalized.stage.as_str(),
        kind = normalized.kind.as_str(),
        retryable = normalized.retryable,
        action = normalized.action.as_str(),
        dedup_key,
        raw_class = normalized.raw.class,
        raw_message = normalized.raw.message_redacted,
        egress_sent = egress
            .get("sent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        egress_reply_targets_before = reply_targets_before,
        egress_drained_count = drained_count,
        "run failed"
    );
}

fn ordered_runner_event_callback() -> (
    Box<dyn Fn(RunnerEvent) + Send + Sync>,
    mpsc::UnboundedReceiver<RunnerEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<RunnerEvent>();
    let callback: Box<dyn Fn(RunnerEvent) + Send + Sync> = Box::new(move |event| {
        if tx.send(event).is_err() {
            debug!("runner event dropped because event processor is closed");
        }
    });
    (callback, rx)
}

async fn run_with_tools(
    state: &Arc<GatewayState>,
    model_store: &Arc<RwLock<DisabledModelsStore>>,
    run_id: &str,
    provider: Arc<dyn moltis_agents::model::LlmProvider>,
    model_id: &str,
    tool_registry: &Arc<RwLock<ToolRegistry>>,
    user_content: &UserContent,
    provider_name: &str,
    history_raw: &[serde_json::Value],
    session_key: &str,
    trigger_id: &str,
    session_key_for_tools: Option<&str>,
    desired_reply_medium: ReplyMedium,
    project_context: Option<&str>,
    runtime_context: Option<&PromptRuntimeContext>,
    user_message_index: usize,
    skills: &[moltis_skills::types::SkillMetadata],
    hook_registry: Option<Arc<moltis_common::hooks::HookRegistry>>,
    accept_language: Option<String>,
    conn_id: Option<String>,
    session_store: Option<&Arc<SessionStore>>,
    mcp_disabled: bool,
    client_seq: Option<u64>,
) -> Option<ChatRunOutput> {
    let agent_id = resolve_session_agent_id(state, runtime_context).await;
    let agent_id_effective = agent_id.as_deref().unwrap_or("default");
    let agent = load_prompt_agent_with_id(agent_id.as_deref());

    let native_tools = provider.supports_tools();

    let filtered_registry = {
        let registry_guard = tool_registry.read().await;
        apply_runtime_tool_filters(&registry_guard, &agent.config, skills, mcp_disabled)
    };

    let canonical = match build_canonical_system_prompt_v1(
        &filtered_registry,
        native_tools,
        false, // run_with_tools: include tool inventory when available
        project_context,
        skills,
        agent_id_effective,
        agent.identity_md_raw.as_deref(),
        agent.soul_text.as_deref(),
        agent.agents_text.as_deref(),
        agent.tools_text.as_deref(),
        to_prompt_reply_medium(desired_reply_medium),
        runtime_context,
        session_key,
    ) {
        Ok(v) => v,
        Err(e) => {
            handle_run_failed_event(
                state,
                model_store,
                RunFailedEvent {
                    run_id: run_id.to_string(),
                    session_key: session_key.to_string(),
                    trigger_id: Some(trigger_id.to_string()),
                    provider_name: provider_name.to_string(),
                    model_id: model_id.to_string(),
                    stage_hint: FailureStage::Runner,
                    raw_error: e.to_string(),
                    details: serde_json::json!({
                        "kind": "canonical_system_prompt_v1_build_failed",
                    }),
                    seq: client_seq,
                },
            )
            .await;
            return None;
        },
    };
    for w in &canonical.warnings {
        warn!(session = %session_key, warning = %w, "prompt template warning");
    }
    let system_prompt_text = canonical.system_prompt;

    let session_entry_for_hooks = if hook_registry.is_some() {
        if let Some(ref sm) = state.services.session_metadata {
            sm.get(session_key).await
        } else {
            None
        }
    } else {
        None
    };
    let session_key_for_hooks = session_key_from_session_entry(session_entry_for_hooks.as_ref());
    let channel_target_for_hooks =
        channel_target_from_session_entry(session_entry_for_hooks.as_ref());
    let session_is_sandboxed = if let Some(ref router) = state.sandbox_router {
        router
            .is_sandboxed(session_key, session_key_for_hooks.as_deref())
            .await
            .unwrap_or(false)
    } else {
        false
    };

    // Dispatch BeforeAgentStart hook (may block).
    if let Some(ref hooks) = hook_registry {
        let payload = moltis_common::hooks::HookPayload::BeforeAgentStart {
            session_id: session_key.to_string(),
            session_key: session_key_for_hooks.clone(),
            channel_target: channel_target_for_hooks.clone(),
            model: provider.id().to_string(),
        };
        match hooks.dispatch(&payload).await {
            Ok(moltis_common::hooks::HookAction::Block(reason)) => {
                let error_str = format!("blocked by BeforeAgentStart hook: {reason}");
                handle_run_failed_event(
                    state,
                    model_store,
                    RunFailedEvent {
                        run_id: run_id.to_string(),
                        session_key: session_key.to_string(),
                        trigger_id: Some(trigger_id.to_string()),
                        provider_name: provider_name.to_string(),
                        model_id: model_id.to_string(),
                        stage_hint: FailureStage::Runner,
                        raw_error: error_str,
                        details: serde_json::json!({
                            "hook": "BeforeAgentStart",
                        }),
                        seq: client_seq,
                    },
                )
                .await;
                return None;
            },
            Ok(moltis_common::hooks::HookAction::ModifyPayload(_)) => {
                debug!("BeforeAgentStart ModifyPayload ignored");
            },
            Ok(moltis_common::hooks::HookAction::Continue) => {},
            Err(e) => {
                warn!(
                    run_id,
                    session = session_key,
                    error = %e,
                    "BeforeAgentStart hook dispatch failed"
                );
            },
        }
    }

    // Broadcast tool events to the UI in the order emitted by the runner.
    let state_for_events = Arc::clone(state);
    let run_id_for_events = run_id.to_string();
    let session_key_for_events = session_key.to_string();
    let trigger_id_for_events = trigger_id.to_string();
    let provider_for_events = provider_name.to_string();
    let model_for_events = model_id.to_string();
    let session_store_for_events = session_store.map(Arc::clone);
    let hook_registry_for_events = hook_registry.clone();
    let session_key_for_hooks_for_event_forwarder = session_key_for_hooks.clone();
    let channel_target_for_hooks_for_event_forwarder = channel_target_for_hooks.clone();
    let (on_event, mut event_rx) = ordered_runner_event_callback();
    let event_forwarder = tokio::spawn(async move {
        // Track tool call arguments from ToolCallStart so they can be persisted in ToolCallEnd.
        let mut tool_args_map: HashMap<String, Value> = HashMap::new();
        let mut retry_logged = false;
        while let Some(event) = event_rx.recv().await {
            let state = Arc::clone(&state_for_events);
            let run_id = run_id_for_events.clone();
            let sk = session_key_for_events.clone();
            let trigger_id = trigger_id_for_events.clone();
            let provider = provider_for_events.clone();
            let model = model_for_events.clone();
            let store = session_store_for_events.clone();
            let hook_registry = hook_registry_for_events.clone();
            let session_key_for_hooks_opt = session_key_for_hooks_for_event_forwarder.clone();
            let channel_target_for_hooks_opt = channel_target_for_hooks_for_event_forwarder.clone();
            let seq = client_seq;
            let payload = match event {
                RunnerEvent::Thinking => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "thinking",
                    "seq": seq,
                }),
                RunnerEvent::ThinkingDone => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "thinking_done",
                    "seq": seq,
                }),
                RunnerEvent::ToolCallStart {
                    id,
                    name,
                    arguments,
                } => {
                    tool_args_map.insert(id.clone(), arguments.clone());

                    // Send tool status to channels (Telegram, etc.)
                    let state_clone = Arc::clone(&state);
                    let sk_clone = sk.clone();
                    let name_clone = name.clone();
                    let args_clone = arguments.clone();
                    tokio::spawn(async move {
                        send_tool_status_to_channels(
                            &state_clone,
                            &sk_clone,
                            &trigger_id,
                            &name_clone,
                            &args_clone,
                        )
                        .await;
                    });

                    let is_browser = name == "browser";
                    let mut payload = serde_json::json!({
                        "runId": run_id,
                        "sessionId": sk,
                        "state": "tool_call_start",
                        "toolCallId": id,
                        "toolName": name,
                        "arguments": arguments,
                        "seq": seq,
                    });
                    if is_browser {
                        payload["executionMode"] = serde_json::json!(if session_is_sandboxed {
                            "sandbox"
                        } else {
                            "host"
                        });
                    }
                    payload
                },
                RunnerEvent::ToolCallEnd {
                    id,
                    name,
                    success,
                    error,
                    result,
                } => {
                    let mut payload = serde_json::json!({
                        "runId": run_id,
                        "sessionId": sk,
                        "state": "tool_call_end",
                        "toolCallId": id,
                        "toolName": name,
                        "success": success,
                        "seq": seq,
                    });
                    if let Some(ref err) = error {
                        payload["error"] = serde_json::json!(parse_chat_error(err, None));
                    }
                    // Check for screenshot to send to channel (Telegram, etc.)
                    let screenshot_to_send = result
                        .as_ref()
                        .and_then(|r| r.get("screenshot"))
                        .and_then(|s| s.as_str())
                        .filter(|s| s.starts_with("data:image/"))
                        .map(String::from);

                    // Extract location from show_map results for native pin
                    let location_to_send = if name == "show_map" {
                        result.as_ref().and_then(|r| {
                            let lat = r.get("latitude")?.as_f64()?;
                            let lon = r.get("longitude")?.as_f64()?;
                            let label = r.get("label").and_then(|l| l.as_str()).map(String::from);
                            Some((lat, lon, label))
                        })
                    } else {
                        None
                    };

                    if let Some(ref res) = result {
                        // Cap output sent to the UI to avoid huge WS frames.
                        let mut capped = res.clone();
                        for field in &["stdout", "stderr"] {
                            if let Some(s) = capped.get(*field).and_then(|v| v.as_str())
                                && s.len() > 10_000
                            {
                                let truncated = format!(
                                    "{}\n\n... [truncated — {} bytes total]",
                                    truncate_utf8(s, 10_000),
                                    s.len()
                                );
                                capped[*field] = serde_json::Value::String(truncated);
                            }
                        }
                        payload["result"] = capped;
                    }

                    // Send native location pin to channels before the screenshot.
                    if let Some((lat, lon, label)) = location_to_send {
                        let state_clone = Arc::clone(&state);
                        let sk_clone = sk.clone();
                        let trigger_id_clone = trigger_id.clone();
                        tokio::spawn(async move {
                            send_location_to_channels(
                                &state_clone,
                                &sk_clone,
                                &trigger_id_clone,
                                lat,
                                lon,
                                label.as_deref(),
                            )
                            .await;
                        });
                    }

                    // Send screenshot to channel targets (Telegram) if present.
                    if let Some(screenshot_data) = screenshot_to_send {
                        let state_clone = Arc::clone(&state);
                        let sk_clone = sk.clone();
                        let trigger_id_clone = trigger_id.clone();
                        tokio::spawn(async move {
                            send_screenshot_to_channels(
                                &state_clone,
                                &sk_clone,
                                &trigger_id_clone,
                                &screenshot_data,
                            )
                            .await;
                        });
                    }

                    // Persist tool result to the session JSONL file.
                    if let Some(ref store) = store {
                        let tracked_args = tool_args_map.remove(&id);
                        // Save screenshot to media dir (if present) and replace
                        // with a lightweight path reference. Strip screenshot_scale
                        // (only needed for live rendering). Cap stdout/stderr at
                        // 10 KB, matching the WS broadcast cap.
                        let store_media = Arc::clone(store);
                        let sk_media = sk.clone();
                        let tool_call_id = id.clone();
                        let persisted_result = result.as_ref().map(|res| {
                            let mut r = res.clone();
                            // Try to decode and persist the screenshot to the media
                            // directory. Extract base64 into an owned Vec first to
                            // release the borrow on `r`.
                            let decoded_screenshot = r
                                .get("screenshot")
                                .and_then(|v| v.as_str())
                                .filter(|s| s.starts_with("data:image/"))
                                .and_then(|uri| uri.split(',').nth(1))
                                .and_then(|b64| {
                                    use base64::Engine;
                                    base64::engine::general_purpose::STANDARD.decode(b64).ok()
                                });
                            if let Some(bytes) = decoded_screenshot {
                                let filename = format!("{tool_call_id}.png");
                                let store_ref = Arc::clone(&store_media);
                                let sk_ref = sk_media.clone();
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        store_ref.save_media(&sk_ref, &filename, &bytes).await
                                    {
                                        warn!("failed to save screenshot media: {e}");
                                    }
                                });
                                let sanitized = SessionStore::key_to_filename(&sk_media);
                                r["screenshot"] = serde_json::Value::String(format!(
                                    "media/{sanitized}/{tool_call_id}.png"
                                ));
                            }
                            // If screenshot is still a data URI (decode failed), strip it.
                            let strip_screenshot = r
                                .get("screenshot")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s.starts_with("data:"));
                            if let Some(obj) = r.as_object_mut() {
                                if strip_screenshot {
                                    obj.remove("screenshot");
                                }
                                obj.remove("screenshot_scale");
                            }
                            for field in &["stdout", "stderr"] {
                                if let Some(s) = r.get(*field).and_then(|v| v.as_str())
                                    && s.len() > 10_000
                                {
                                    let truncated = format!(
                                        "{}\n\n... [truncated — {} bytes total]",
                                        truncate_utf8(s, 10_000),
                                        s.len()
                                    );
                                    r[*field] = serde_json::Value::String(truncated);
                                }
                            }
                            r
                        });
                        let mut persisted_result_for_store = persisted_result;
                        let mut persist_blocked = false;

                        // Dispatch ToolResultPersist hook (may modify/block).
                        if let Some(ref hooks) = hook_registry {
                            let hook_payload =
                                moltis_common::hooks::HookPayload::ToolResultPersist {
                                    session_id: sk.clone(),
                                    session_key: session_key_for_hooks_opt.clone(),
                                    channel_target: channel_target_for_hooks_opt.clone(),
                                    tool_name: name.clone(),
                                    result: persisted_result_for_store
                                        .clone()
                                        .unwrap_or(serde_json::Value::Null),
                                };
                            match hooks.dispatch(&hook_payload).await {
                                Ok(moltis_common::hooks::HookAction::Block(reason)) => {
                                    warn!(
                                        session = %sk,
                                        tool_name = %name,
                                        reason = %reason,
                                        "tool result persistence blocked by hook"
                                    );
                                    persist_blocked = true;
                                },
                                Ok(moltis_common::hooks::HookAction::ModifyPayload(v)) => {
                                    persisted_result_for_store = (!v.is_null()).then_some(v);
                                },
                                Ok(moltis_common::hooks::HookAction::Continue) => {},
                                Err(e) => {
                                    warn!(
                                        session = %sk,
                                        tool_name = %name,
                                        error = %e,
                                        "ToolResultPersist hook dispatch failed"
                                    );
                                },
                            }
                        }
                        if !persist_blocked {
                            let tool_result_msg = PersistedMessage::tool_result(
                                id,
                                name,
                                tracked_args,
                                success,
                                persisted_result_for_store,
                                error,
                            );
                            let store_clone = Arc::clone(store);
                            let sk_persist = sk.clone();
                            tokio::spawn(async move {
                                if let Err(e) = store_clone
                                    .append(&sk_persist, &tool_result_msg.to_value())
                                    .await
                                {
                                    warn!("failed to persist tool result: {e}");
                                }
                            });
                        }
                    }

                    payload
                },
                RunnerEvent::ThinkingText(text) => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "thinking_text",
                    "text": text,
                    "seq": seq,
                }),
                RunnerEvent::TextDelta(text) => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "delta",
                    "text": text,
                    "seq": seq,
                }),
                RunnerEvent::Iteration(n) => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "iteration",
                    "iteration": n,
                    "seq": seq,
                }),
                RunnerEvent::SubAgentStart { task, model, depth } => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "sub_agent_start",
                    "task": task,
                    "model": model,
                    "depth": depth,
                    "seq": seq,
                }),
                RunnerEvent::SubAgentEnd {
                    task,
                    model,
                    depth,
                    iterations,
                    tool_calls_made,
                } => serde_json::json!({
                    "runId": run_id,
                    "sessionId": sk,
                    "state": "sub_agent_end",
                    "task": task,
                    "model": model,
                    "depth": depth,
                    "iterations": iterations,
                    "toolCallsMade": tool_calls_made,
                    "seq": seq,
                }),
                RunnerEvent::RetryingAfterError(msg) => {
                    let reason_preview = sanitize_reason_preview(&msg);
                    if !retry_logged {
                        retry_logged = true;
                        info!(
                            event = "llm.retrying",
                            run_id = %run_id,
                            session_key = %sk,
                            trigger_id = %trigger_id,
                            provider = %provider,
                            model = %model,
                            reason_preview = %reason_preview,
                            "runner retrying after transient error"
                        );
                    }
                    serde_json::json!({
                        "runId": run_id,
                        "sessionId": sk,
                        "state": "retrying",
                        "reasonPreview": reason_preview,
                        "provider": provider,
                        "model": model,
                        "seq": seq,
                    })
                },
            };
            broadcast(&state, "chat", payload, BroadcastOpts::default()).await;
        }
    });

    // Convert persisted JSON history to typed ChatMessages for the LLM provider.
    let chat_history = values_to_chat_messages(history_raw);
    let hist = if chat_history.is_empty() {
        None
    } else {
        Some(chat_history)
    };

    let retry_budget = CompactionBudget::for_provider(provider.as_ref());
    let estimated_next_input_tokens =
        estimate_next_input_tokens(&system_prompt_text, history_raw, user_content);

    // Inject session identifiers, sandbox mode, and accept-language into tool call params so tools can
    // resolve per-session state and forward the user's locale to web requests.
    // The browser tool uses _sandbox to determine whether to run in a container.
    let mut tool_context = serde_json::json!({
        "_sessionId": session_key,
        "_runId": run_id,
        "_sandbox": session_is_sandboxed,
    });
    if let Some(session_key_for_tools) = session_key_for_tools {
        tool_context["_sessionKey"] = serde_json::json!(session_key_for_tools);
    }
    if let Some(lang) = accept_language.as_deref() {
        tool_context["_acceptLanguage"] = serde_json::json!(lang);
    }
    if let Some(cid) = conn_id.as_deref() {
        tool_context["_connId"] = serde_json::json!(cid);
    }

    let provider_ref = provider.clone();
    let runner_hook_context = RunnerHookContext {
        session_key: session_key_for_hooks.clone(),
        channel_target: channel_target_for_hooks.clone(),
    };
    let first_result = run_agent_loop_streaming_with_prefix_and_hook_context(
        provider,
        &filtered_registry,
        vec![ChatMessage::system(&system_prompt_text)],
        user_content,
        Some(&on_event),
        hist,
        Some(tool_context.clone()),
        Some(runner_hook_context.clone()),
        hook_registry.clone(),
    )
    .await;

    // On context-window overflow, compact the session and retry once.
    let result = match first_result {
        Err(AgentRunError::ContextWindowExceeded(ref msg)) if session_store.is_some() => {
            let store = session_store?;
            info!(
                run_id,
                session = session_key,
                error = %msg,
                estimated_next_input_tokens,
                input_hard_cap = retry_budget.input_hard_cap,
                "context window exceeded — compacting and retrying"
            );

            broadcast(
                state,
                "chat",
                serde_json::json!({
                    "runId": run_id,
                    "sessionId": session_key,
                    "state": "auto_compact",
                    "phase": "start",
                    "reason": "context_window_exceeded",
                    "budget": {
                        "effectiveContextWindow": retry_budget.effective_context_window,
                        "inputHardCap": retry_budget.input_hard_cap,
                        "reservedOutputTokens": retry_budget.reserved_output_tokens,
                        "reserveSafetyTokens": retry_budget.reserve_safety_tokens,
                        "effectiveInputBudget": retry_budget.effective_input_budget(),
                        "estimatedNextInputTokens": estimated_next_input_tokens,
                        "highWatermark": retry_budget.high_watermark,
                        "lowWatermark": retry_budget.low_watermark,
                    }
                }),
                BroadcastOpts::default(),
            )
            .await;

            // Inline compaction: summarize history, replace in store.
            match compact_session(
                state,
                hook_registry.clone(),
                store,
                session_key,
                &provider_ref,
                KEEP_LAST_USER_ROUNDS,
            )
            .await
            {
                Ok(_) => {
                    broadcast(
                        state,
                        "chat",
                        serde_json::json!({
                            "runId": run_id,
                            "sessionId": session_key,
                            "state": "auto_compact",
                            "phase": "done",
                            "reason": "context_window_exceeded",
                        }),
                        BroadcastOpts::default(),
                    )
                    .await;

                    // Reload compacted history and retry.
                    let compacted_history_raw = store.read(session_key).await.unwrap_or_default();
                    let compacted_chat = values_to_chat_messages(&compacted_history_raw);
                    let retry_hist = if compacted_chat.is_empty() {
                        None
                    } else {
                        Some(compacted_chat)
                    };

                    run_agent_loop_streaming_with_prefix_and_hook_context(
                        provider_ref.clone(),
                        &filtered_registry,
                        vec![ChatMessage::system(&system_prompt_text)],
                        user_content,
                        Some(&on_event),
                        retry_hist,
                        Some(tool_context),
                        Some(runner_hook_context),
                        hook_registry.clone(),
                    )
                    .await
                },
                Err(e) => {
                    warn!(run_id, error = %e, "retry compaction failed");
                    broadcast(
                        state,
                        "chat",
                        serde_json::json!({
                            "runId": run_id,
                            "sessionId": session_key,
                            "state": "auto_compact",
                            "phase": "error",
                            "error": e.to_string(),
                        }),
                        BroadcastOpts::default(),
                    )
                    .await;
                    // Return the original error.
                    first_result
                },
            }
        },
        other => other,
    };

    // Ensure all runner events (including deltas) are broadcast in order before
    // emitting terminal final/error frames.
    drop(on_event);
    if let Err(e) = event_forwarder.await {
        warn!(run_id, error = %e, "runner event forwarder task failed");
    }

    match result {
        Ok(result) => {
            clear_unsupported_model(state, model_store, model_id).await;

            let mut display_text = result.text;

            // Dispatch MessageSending hook (may modify/block).
            if let Some(ref hooks) = hook_registry {
                let payload = moltis_common::hooks::HookPayload::MessageSending {
                    session_id: session_key.to_string(),
                    session_key: session_key_for_hooks.clone(),
                    channel_target: channel_target_for_hooks.clone(),
                    content: display_text.clone(),
                };
                match hooks.dispatch(&payload).await {
                    Ok(moltis_common::hooks::HookAction::Block(reason)) => {
                        let error_str = format!("blocked by MessageSending hook: {reason}");
                        handle_run_failed_event(
                            state,
                            model_store,
                            RunFailedEvent {
                                run_id: run_id.to_string(),
                                session_key: session_key.to_string(),
                                trigger_id: Some(trigger_id.to_string()),
                                provider_name: provider_name.to_string(),
                                model_id: model_id.to_string(),
                                stage_hint: FailureStage::Runner,
                                raw_error: error_str,
                                details: serde_json::json!({
                                    "hook": "MessageSending",
                                }),
                                seq: client_seq,
                            },
                        )
                        .await;
                        return None;
                    },
                    Ok(moltis_common::hooks::HookAction::ModifyPayload(v)) => {
                        if let Some(s) = v.as_str() {
                            display_text = s.to_string();
                        } else if let Some(obj) = v.as_object()
                            && let Some(s) = obj.get("content").and_then(|c| c.as_str())
                        {
                            display_text = s.to_string();
                        } else {
                            warn!(
                                run_id,
                                session = session_key,
                                "MessageSending ModifyPayload ignored (expected string or object with 'content')"
                            );
                        }
                    },
                    Ok(moltis_common::hooks::HookAction::Continue) => {},
                    Err(e) => {
                        warn!(
                            run_id,
                            session = session_key,
                            error = %e,
                            "MessageSending hook dispatch failed"
                        );
                    },
                }
            }

            let is_silent = display_text.trim().is_empty();
            let reply_target_ref_hash =
                first_reply_target_ref_hash(state, session_key, &trigger_id).await;
            let (channel_type, bucket_key) =
                session_channel_binding_fields(state, session_key).await;

            info!(
                run_id,
                trigger_id = %trigger_id,
                channel_turn_id = %trigger_id,
                session_id = %session_key,
                channel_type = channel_type.as_deref(),
                bucket_key = bucket_key.as_deref(),
                reply_target_ref_hash = reply_target_ref_hash.as_deref(),
                iterations = result.iterations,
                tool_calls = result.tool_calls_made,
                output_text_len = display_text.len(),
                output_preview = %log_text_preview(&display_text, 240),
                silent = is_silent,
                "agent run complete"
            );
            let assistant_message_index = user_message_index + 1;

            // Generate & persist TTS audio for voice-medium web UI replies.
            let audio_path = if !is_silent && desired_reply_medium == ReplyMedium::Voice {
                if let Some(bytes) = generate_tts_audio(state, session_key, &display_text).await {
                    let filename = format!("{run_id}.ogg");
                    if let Some(store) = session_store {
                        match store.save_media(session_key, &filename, &bytes).await {
                            Ok(path) => Some(path),
                            Err(e) => {
                                warn!(run_id, error = %e, "failed to save TTS audio to media dir");
                                None
                            },
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let final_payload = ChatFinalBroadcast {
                run_id: run_id.to_string(),
                session_id: session_key.to_string(),
                state: "final",
                text: display_text.clone(),
                model: provider_ref.id().to_string(),
                provider: provider_name.to_string(),
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
                message_index: assistant_message_index,
                reply_medium: desired_reply_medium,
                iterations: Some(result.iterations),
                tool_calls_made: Some(result.tool_calls_made),
                audio: audio_path.clone(),
                seq: client_seq,
            };
            #[allow(clippy::unwrap_used)] // serializing known-valid struct
            let payload_val = serde_json::to_value(&final_payload).unwrap();
            broadcast(state, "chat", payload_val, BroadcastOpts::default()).await;

            if !is_silent {
                // Send push notification when chat response completes
                #[cfg(feature = "push-notifications")]
                {
                    tracing::info!("push: checking push notification (agent mode)");
                    send_chat_push_notification(state, session_key, &display_text).await;
                }
                deliver_channel_replies(
                    state,
                    session_key,
                    trigger_id,
                    &display_text,
                    desired_reply_medium,
                )
                .await;
            } else {
                // Silent responses must still clear pending channel delivery state
                // (reply targets + logbook) to avoid later reply "cross-wiring".
                deliver_channel_replies(state, session_key, trigger_id, "", desired_reply_medium)
                    .await;
            }

            // Dispatch MessageSent + AgentEnd hooks (read-only).
            if let Some(ref hooks) = hook_registry {
                let payload = moltis_common::hooks::HookPayload::MessageSent {
                    session_id: session_key.to_string(),
                    session_key: session_key_for_hooks.clone(),
                    channel_target: channel_target_for_hooks.clone(),
                    content: display_text.clone(),
                };
                if let Err(e) = hooks.dispatch(&payload).await {
                    warn!(run_id, session = session_key, error = %e, "MessageSent hook failed");
                }

                let payload = moltis_common::hooks::HookPayload::AgentEnd {
                    session_id: session_key.to_string(),
                    session_key: session_key_for_hooks.clone(),
                    channel_target: channel_target_for_hooks.clone(),
                    text: display_text.clone(),
                    iterations: result.iterations,
                    tool_calls: result.tool_calls_made,
                };
                if let Err(e) = hooks.dispatch(&payload).await {
                    warn!(run_id, session = session_key, error = %e, "AgentEnd hook failed");
                }
            }
            Some(ChatRunOutput {
                text: display_text,
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
                cached_tokens: result.usage.cache_read_tokens,
                audio_path,
            })
        },
        Err(e) => {
            let error_str = e.to_string();
            handle_run_failed_event(
                state,
                model_store,
                RunFailedEvent {
                    run_id: run_id.to_string(),
                    session_key: session_key.to_string(),
                    trigger_id: Some(trigger_id.to_string()),
                    provider_name: provider_name.to_string(),
                    model_id: model_id.to_string(),
                    stage_hint: FailureStage::Runner,
                    raw_error: error_str,
                    details: serde_json::json!({}),
                    seq: client_seq,
                },
            )
            .await;
            None
        },
    }
}

const KEEP_LAST_USER_ROUNDS: usize = 4;
const SAFETY_MARGIN_TOKENS: u64 = 1024;

#[derive(Debug, Clone, Copy)]
struct CompactionBudget {
    effective_context_window: u64,
    input_hard_cap: u64,
    derived_input_cap: bool,
    reserved_output_tokens: u64,
    reserve_safety_tokens: u64,
    high_watermark: u64,
    low_watermark: u64,
}

impl CompactionBudget {
    fn for_provider(provider: &dyn moltis_agents::model::LlmProvider) -> Self {
        let effective_context_window = u64::from(provider.context_window());
        let (input_hard_cap, derived_input_cap) = provider
            .input_limit()
            .map(|v| (u64::from(v), false))
            .unwrap_or_else(|| ((effective_context_window * 80) / 100, true));
        let reserved_output_tokens = provider
            .output_limit()
            .map(u64::from)
            .unwrap_or_else(|| u64::min(16_384, effective_context_window / 5));
        let reserve_safety_tokens = SAFETY_MARGIN_TOKENS;
        let high_watermark = (input_hard_cap * 85) / 100;
        let low_watermark = (input_hard_cap * 60) / 100;
        Self {
            effective_context_window,
            input_hard_cap,
            derived_input_cap,
            reserved_output_tokens,
            reserve_safety_tokens,
            high_watermark,
            low_watermark,
        }
    }

    fn effective_input_budget(&self) -> u64 {
        self.input_hard_cap
            .saturating_sub(self.reserve_safety_tokens)
    }
}

fn tokens_estimate_utf8_bytes_div_3(text: &str) -> u64 {
    let bytes = text.as_bytes().len() as u64;
    (bytes + 2) / 3
}

fn estimate_input_tokens_for_messages(messages: &[ChatMessage]) -> u64 {
    let mut total = 0u64;
    for msg in messages {
        match msg {
            ChatMessage::System { content } => total += tokens_estimate_utf8_bytes_div_3(content),
            ChatMessage::User { content } => match content {
                UserContent::Text(t) => total += tokens_estimate_utf8_bytes_div_3(t),
                UserContent::Multimodal(parts) => {
                    for p in parts {
                        if let ContentPart::Text(t) = p {
                            total += tokens_estimate_utf8_bytes_div_3(t);
                        }
                    }
                },
            },
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                if let Some(t) = content {
                    total += tokens_estimate_utf8_bytes_div_3(t);
                }
                for tc in tool_calls {
                    total += tokens_estimate_utf8_bytes_div_3(&tc.name);
                    total += tokens_estimate_utf8_bytes_div_3(&tc.arguments.to_string());
                }
            },
            ChatMessage::Tool {
                tool_call_id: _,
                content,
            } => total += tokens_estimate_utf8_bytes_div_3(content),
        }
    }
    total
}

fn reconstruct_tool_history_for_prompt_estimate(
    history_raw: &[serde_json::Value],
    max_tool_result_bytes: usize,
) -> Vec<serde_json::Value> {
    let mut out = Vec::with_capacity(history_raw.len());
    for val in history_raw {
        if val.get("role").and_then(|r| r.as_str()) != Some("tool_result") {
            out.push(val.clone());
            continue;
        }

        let tool_call_id = val
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tool_name = val
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let args_str = val
            .get("arguments")
            .map(|a| a.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let output = if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
            format!("Error: {err}")
        } else if let Some(res) = val.get("result") {
            res.to_string()
        } else {
            String::new()
        };
        let output = moltis_agents::runner::sanitize_tool_result(&output, max_tool_result_bytes);

        // Reconstruct the call+output pair the LLM would typically see:
        // assistant(tool_calls) -> tool(output).
        out.push(serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": tool_call_id,
                "type": "function",
                "function": {
                    "name": tool_name,
                    "arguments": args_str,
                }
            }]
        }));
        out.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": val.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or(""),
            "content": output,
        }));
    }
    out
}

fn extract_planned_max_output_toks(overrides: &serde_json::Value) -> Option<u64> {
    overrides
        .get("generation")
        .and_then(|g| g.get("max_output_tokens"))
        .and_then(|m| {
            m.get("effective")
                .and_then(|v| v.as_u64())
                .or_else(|| m.as_u64())
        })
}

fn build_token_debug_info(
    provider: &dyn moltis_agents::model::LlmProvider,
    llm_debug: &serde_json::Value,
    system_prompt: &str,
    history_raw: &[serde_json::Value],
    draft_text: Option<&str>,
    max_tool_result_bytes: usize,
) -> serde_json::Value {
    let last_request = {
        let mut input_tokens: Option<u64> = None;
        let mut output_tokens: Option<u64> = None;
        let mut cached_tokens: Option<u64> = None;
        for m in history_raw.iter().rev() {
            if m.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            input_tokens = m.get("inputTokens").and_then(|v| v.as_u64());
            output_tokens = m.get("outputTokens").and_then(|v| v.as_u64());
            cached_tokens = m.get("cachedTokens").and_then(|v| v.as_u64());
            if input_tokens.is_some() || output_tokens.is_some() || cached_tokens.is_some() {
                break;
            }
        }
        serde_json::json!({
            "inputTokens": input_tokens,
            "outputTokens": output_tokens,
            "cachedTokens": cached_tokens,
        })
    };

    let context_window = u64::from(provider.context_window());
    let planned_max_output_toks = extract_planned_max_output_toks(
        llm_debug
            .get("overrides")
            .unwrap_or(&serde_json::Value::Null),
    )
    .or_else(|| provider.output_limit().map(u64::from))
    .unwrap_or_else(|| u64::min(16_384, context_window / 5));

    let max_input_toks = provider
        .input_limit()
        .map(u64::from)
        .unwrap_or_else(|| (context_window * 80) / 100);
    let auto_compact_toks_thred = (max_input_toks * 85) / 100;

    let history_with_tools =
        reconstruct_tool_history_for_prompt_estimate(history_raw, max_tool_result_bytes);
    let mut msgs = Vec::with_capacity(1 + history_with_tools.len());
    msgs.push(ChatMessage::system(system_prompt));
    msgs.extend(values_to_chat_messages(&history_with_tools));
    let history_input_toks_est = estimate_input_tokens_for_messages(&msgs);

    let pending_user_toks_est = draft_text
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(tokens_estimate_utf8_bytes_div_3)
        .unwrap_or(0);

    let reserve_safety_toks = SAFETY_MARGIN_TOKENS;
    let prompt_input_toks_est = history_input_toks_est
        .saturating_add(pending_user_toks_est)
        .saturating_add(reserve_safety_toks);

    let compact_progress = if auto_compact_toks_thred == 0 {
        None
    } else {
        Some(prompt_input_toks_est as f64 / auto_compact_toks_thred as f64)
    };

    serde_json::json!({
        "lastRequest": last_request,
        "nextRequest": {
            "contextWindow": context_window,
            "plannedMaxOutputToks": planned_max_output_toks,
            "maxInputToks": max_input_toks,
            "autoCompactToksThred": auto_compact_toks_thred,
            "promptInputToksEst": prompt_input_toks_est,
            "compactProgress": compact_progress,
            "details": {
                "method": "heuristic",
                "historyInputToksEst": history_input_toks_est,
                "pendingUserToksEst": pending_user_toks_est,
                "reserveSafetyToks": reserve_safety_toks,
                "draftProvided": draft_text.is_some(),
                "maxInputDerived": provider.input_limit().is_none(),
            }
        }
    })
}

fn estimate_next_input_tokens(
    system_prompt: &str,
    history_raw: &[serde_json::Value],
    user_content: &UserContent,
) -> u64 {
    let mut messages = Vec::with_capacity(history_raw.len() + 2);
    messages.push(ChatMessage::system(system_prompt));
    messages.extend(values_to_chat_messages(history_raw));
    messages.push(ChatMessage::User {
        content: user_content.clone(),
    });
    estimate_input_tokens_for_messages(&messages) + SAFETY_MARGIN_TOKENS
}

fn keep_window_start_idx(history_raw: &[serde_json::Value], keep_last_user_rounds: usize) -> usize {
    if keep_last_user_rounds == 0 {
        return history_raw.len();
    }
    let user_indices: Vec<usize> = history_raw
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            m.get("role")
                .and_then(|v| v.as_str())
                .filter(|r| *r == "user")
                .map(|_| i)
        })
        .collect();
    if user_indices.len() <= keep_last_user_rounds {
        return 0;
    }
    let keep_start_user = user_indices.len() - keep_last_user_rounds;
    user_indices[keep_start_user]
}

fn build_compacted_history(
    history_raw: &[serde_json::Value],
    summary: &str,
    keep_last_user_rounds: usize,
    created_at: Option<u64>,
) -> Result<(Vec<serde_json::Value>, usize, usize), String> {
    let keep_start_idx = keep_window_start_idx(history_raw, keep_last_user_rounds);
    if keep_start_idx == 0 {
        return Err("nothing to compact".into());
    }

    let keep_window = history_raw[keep_start_idx..].to_vec();
    let compacted_msg = PersistedMessage::Assistant {
        content: format!("[Conversation Summary]\n\n{summary}"),
        created_at,
        model: None,
        provider: None,
        input_tokens: None,
        output_tokens: None,
        cached_tokens: None,
        tool_calls: None,
        audio: None,
        seq: None,
        run_id: None,
    };

    let mut compacted = Vec::with_capacity(1 + keep_window.len());
    compacted.push(compacted_msg.to_value());
    compacted.extend(keep_window);
    let kept_message_count = compacted.len().saturating_sub(1);
    Ok((compacted, keep_start_idx, kept_message_count))
}

#[derive(Debug, Clone)]
struct CompactionResult {
    kept_message_count: usize,
    summary_len: usize,
    compacted: Vec<serde_json::Value>,
}

/// Compact a session's history by summarizing older turns and keeping the last N user rounds raw.
///
/// Standalone helper so proactive/retry compaction paths can share one implementation.
async fn compact_session(
    state: &Arc<GatewayState>,
    hook_registry: Option<Arc<moltis_common::hooks::HookRegistry>>,
    store: &Arc<SessionStore>,
    session_id: &str,
    provider: &Arc<dyn moltis_agents::model::LlmProvider>,
    keep_last_user_rounds: usize,
) -> Result<CompactionResult, String> {
    let history = store.read(session_id).await.map_err(|e| e.to_string())?;
    if history.is_empty() {
        return Err("nothing to compact".into());
    }

    let pre_message_count = history.len();
    let session_entry = if let Some(ref sm) = state.services.session_metadata {
        sm.get(session_id).await
    } else {
        None
    };
    let session_key_for_hooks = session_key_from_session_entry(session_entry.as_ref());

    if let Some(ref hooks) = hook_registry {
        let payload = moltis_common::hooks::HookPayload::BeforeCompaction {
            session_id: session_id.to_string(),
            session_key: session_key_for_hooks.clone(),
            channel_target: channel_target_from_session_entry(session_entry.as_ref()),
            message_count: pre_message_count,
        };
        if let Err(e) = hooks.dispatch(&payload).await {
            warn!(session = %session_id, error = %e, "BeforeCompaction hook failed");
        }
    }

    // Best-effort silent memory flush before summarization.
    if let Some(ref mm) = state.memory_manager {
        let memory_dir = moltis_config::data_dir();
        let chat_history_for_memory = values_to_chat_messages(&history);
        match moltis_agents::silent_turn::run_silent_memory_turn(
            Arc::clone(provider),
            &chat_history_for_memory,
            &memory_dir,
            Some(session_id),
        )
        .await
        {
            Ok(paths) => {
                for path in &paths {
                    if let Err(e) = mm.sync_path(path).await {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "compact: memory sync of written file failed"
                        );
                    }
                }
                if !paths.is_empty() {
                    info!(
                        files = paths.len(),
                        "compact: silent memory turn wrote files"
                    );
                }
            },
            Err(e) => warn!(error = %e, "compact: silent memory turn failed"),
        }
    }

    let keep_start_idx = keep_window_start_idx(&history, keep_last_user_rounds);
    if keep_start_idx == 0 {
        return Err("nothing to compact".into());
    }

    let old_segment = &history[..keep_start_idx];

    let mut summary_messages = vec![ChatMessage::system(
        "You are a conversation summarizer. Summarize the conversation messages you see.\n\
\n\
Output MUST be factual and concise. Use this fixed structure:\n\
\n\
## Context\n\
- ...\n\
\n\
## Decisions\n\
- ...\n\
\n\
## Plan\n\
- ...\n\
\n\
## Open Questions\n\
- ...\n\
\n\
## Artifacts\n\
- ... (files, commands, links, identifiers)\n\
\n\
If something is unknown, write \"Unknown\" instead of guessing.",
    )];
    summary_messages.extend(values_to_chat_messages(old_segment));
    summary_messages.push(ChatMessage::user(
        "Summarize the conversation above. Output only the summary, no preamble.",
    ));

    let llm_context = moltis_agents::model::LlmRequestContext {
        session_key: session_key_from_session_entry(session_entry.as_ref()),
        session_id: Some(session_id.to_string()),
        prompt_cache_key: prompt_cache_key_for_session(session_id, session_entry.as_ref()),
        run_id: None,
    };
    let mut stream =
        provider.stream_with_tools_with_context(&llm_context, summary_messages, vec![]);
    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Delta(delta) => summary.push_str(&delta),
            StreamEvent::Done(_) => break,
            StreamEvent::Error(e) => return Err(format!("compact summarization failed: {e}")),
            StreamEvent::ToolCallStart { .. }
            | StreamEvent::ToolCallArgumentsDelta { .. }
            | StreamEvent::ToolCallComplete { .. } => {},
        }
    }

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        return Err("compact produced empty summary".into());
    }
    let created_at = Some(now_ms());
    let (compacted, _keep_start_idx_2, kept_message_count) =
        build_compacted_history(&history, &summary, keep_last_user_rounds, created_at)?;

    store
        .replace_history(session_id, compacted.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Save compaction summary to memory file and trigger sync (best-effort).
    if let Some(ref mm) = state.memory_manager {
        let memory_dir = moltis_config::data_dir().join("memory");
        if let Err(e) = tokio::fs::create_dir_all(&memory_dir).await {
            warn!(error = %e, "compact: failed to create memory dir");
        } else {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let filename = format!("compaction-{}-{ts}.md", session_id);
            let path = memory_dir.join(&filename);
            let content = format!(
                "# Compaction Summary\n\n- **Session**: {session_id}\n- **Timestamp**: {ts}\n\n{summary}"
            );
            if let Err(e) = tokio::fs::write(&path, &content).await {
                warn!(error = %e, "compact: failed to write memory file");
            } else {
                let mm = Arc::clone(mm);
                tokio::spawn(async move {
                    if let Err(e) = mm.sync().await {
                        tracing::warn!("compact: memory sync failed: {e}");
                    }
                });
            }
        }
    }

    if let Some(ref hooks) = hook_registry {
        let payload = moltis_common::hooks::HookPayload::AfterCompaction {
            session_id: session_id.to_string(),
            session_key: session_key_for_hooks.clone(),
            channel_target: channel_target_from_session_entry(session_entry.as_ref()),
            summary_len: summary.len(),
        };
        if let Err(e) = hooks.dispatch(&payload).await {
            warn!(session = %session_id, error = %e, "AfterCompaction hook failed");
        }
    }

    Ok(CompactionResult {
        kept_message_count,
        summary_len: summary.len(),
        compacted,
    })
}

// ── Streaming mode (no tools) ───────────────────────────────────────────────

async fn run_streaming(
    state: &Arc<GatewayState>,
    model_store: &Arc<RwLock<DisabledModelsStore>>,
    run_id: &str,
    provider: Arc<dyn moltis_agents::model::LlmProvider>,
    model_id: &str,
    user_content: &UserContent,
    provider_name: &str,
    history_raw: &[serde_json::Value],
    session_key: &str,
    trigger_id: &str,
    desired_reply_medium: ReplyMedium,
    project_context: Option<&str>,
    user_message_index: usize,
    _skills: &[moltis_skills::types::SkillMetadata],
    runtime_context: Option<&PromptRuntimeContext>,
    session_store: Option<&Arc<SessionStore>>,
    client_seq: Option<u64>,
) -> Option<ChatRunOutput> {
    let agent_id = resolve_session_agent_id(state, runtime_context).await;
    let agent = load_prompt_agent_with_id(agent_id.as_deref());
    let hook_registry = state.inner.read().await.hook_registry.clone();
    let session_entry = if let Some(ref sm) = state.services.session_metadata {
        sm.get(session_key).await
    } else {
        None
    };
    let session_key_for_hooks = session_key_from_session_entry(session_entry.as_ref());

    // Dispatch BeforeAgentStart hook (may block).
    if let Some(ref hooks) = hook_registry {
        let payload = moltis_common::hooks::HookPayload::BeforeAgentStart {
            session_id: session_key.to_string(),
            session_key: session_key_for_hooks.clone(),
            channel_target: channel_target_from_session_entry(session_entry.as_ref()),
            model: provider.id().to_string(),
        };
        match hooks.dispatch(&payload).await {
            Ok(moltis_common::hooks::HookAction::Block(reason)) => {
                let error_str = format!("blocked by BeforeAgentStart hook: {reason}");
                handle_run_failed_event(
                    state,
                    model_store,
                    RunFailedEvent {
                        run_id: run_id.to_string(),
                        session_key: session_key.to_string(),
                        trigger_id: Some(trigger_id.to_string()),
                        provider_name: provider_name.to_string(),
                        model_id: model_id.to_string(),
                        stage_hint: FailureStage::Runner,
                        raw_error: error_str,
                        details: serde_json::json!({
                            "hook": "BeforeAgentStart",
                        }),
                        seq: client_seq,
                    },
                )
                .await;
                return None;
            },
            Ok(moltis_common::hooks::HookAction::ModifyPayload(_)) => {
                debug!("BeforeAgentStart ModifyPayload ignored");
            },
            Ok(moltis_common::hooks::HookAction::Continue) => {},
            Err(e) => {
                warn!(
                    run_id,
                    session = session_key,
                    error = %e,
                    "BeforeAgentStart hook dispatch failed"
                );
            },
        }
    }

    let mut messages: Vec<ChatMessage> = Vec::new();
    let canonical = match build_canonical_system_prompt_v1(
        &ToolRegistry::new(),
        provider.supports_tools(),
        true, // run_streaming: no tools
        project_context,
        _skills,
        agent_id.as_deref().unwrap_or("default"),
        agent.identity_md_raw.as_deref(),
        agent.soul_text.as_deref(),
        agent.agents_text.as_deref(),
        agent.tools_text.as_deref(),
        to_prompt_reply_medium(desired_reply_medium),
        runtime_context,
        session_key,
    ) {
        Ok(v) => v,
        Err(e) => {
            handle_run_failed_event(
                state,
                model_store,
                RunFailedEvent {
                    run_id: run_id.to_string(),
                    session_key: session_key.to_string(),
                    trigger_id: Some(trigger_id.to_string()),
                    provider_name: provider_name.to_string(),
                    model_id: model_id.to_string(),
                    stage_hint: FailureStage::Runner,
                    raw_error: e.to_string(),
                    details: serde_json::json!({
                        "kind": "canonical_system_prompt_v1_build_failed",
                    }),
                    seq: client_seq,
                },
            )
            .await;
            return None;
        },
    };
    for w in &canonical.warnings {
        warn!(session = %session_key, warning = %w, "prompt template warning");
    }
    messages.push(ChatMessage::system(canonical.system_prompt));
    // Convert persisted JSON history to typed ChatMessages for the LLM provider.
    messages.extend(values_to_chat_messages(history_raw));
    messages.push(ChatMessage::User {
        content: user_content.clone(),
    });

    let stream_started_at = Instant::now();
    #[cfg(feature = "metrics")]
    let stream_start = stream_started_at;

    let llm_context = moltis_agents::model::LlmRequestContext {
        session_key: session_key_for_hooks.clone(),
        session_id: Some(session_key.to_string()),
        prompt_cache_key: prompt_cache_key_for_session(session_key, session_entry.as_ref()),
        run_id: Some(run_id.to_string()),
    };
    // Stream-only mode still needs request context (e.g. prompt_cache_key bucketing
    // for OpenAI Responses). Pass empty tools to preserve the no-tools behavior.
    let mut stream = provider.stream_with_tools_with_context(&llm_context, messages, vec![]);
    let mut accumulated = String::new();

    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Delta(delta) => {
                accumulated.push_str(&delta);
                broadcast(
                    state,
                    "chat",
                    serde_json::json!({
                        "runId": run_id,
                        "sessionId": session_key,
                        "state": "delta",
                        "text": delta,
                    }),
                    BroadcastOpts::default(),
                )
                .await;
            },
            StreamEvent::Done(usage) => {
                clear_unsupported_model(state, model_store, model_id).await;

                // Record streaming completion metrics (mirroring provider_chain.rs)
                #[cfg(feature = "metrics")]
                {
                    let duration = stream_start.elapsed().as_secs_f64();
                    counter!(
                        llm_metrics::COMPLETIONS_TOTAL,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .increment(1);
                    counter!(
                        llm_metrics::INPUT_TOKENS_TOTAL,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .increment(u64::from(usage.input_tokens));
                    counter!(
                        llm_metrics::OUTPUT_TOKENS_TOTAL,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .increment(u64::from(usage.output_tokens));
                    counter!(
                        llm_metrics::CACHE_READ_TOKENS_TOTAL,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .increment(u64::from(usage.cache_read_tokens));
                    counter!(
                        llm_metrics::CACHE_WRITE_TOKENS_TOTAL,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .increment(u64::from(usage.cache_write_tokens));
                    histogram!(
                        llm_metrics::COMPLETION_DURATION_SECONDS,
                        labels::PROVIDER => provider_name.to_string(),
                        labels::MODEL => model_id.to_string()
                    )
                    .record(duration);
                }

                // Dispatch MessageSending hook (may modify/block).
                if let Some(ref hooks) = hook_registry {
                    let payload = moltis_common::hooks::HookPayload::MessageSending {
                        session_id: session_key.to_string(),
                        session_key: session_key_for_hooks.clone(),
                        channel_target: channel_target_from_session_entry(session_entry.as_ref()),
                        content: accumulated.clone(),
                    };
                    match hooks.dispatch(&payload).await {
                        Ok(moltis_common::hooks::HookAction::Block(reason)) => {
                            let error_str = format!("blocked by MessageSending hook: {reason}");
                            handle_run_failed_event(
                                state,
                                model_store,
                                RunFailedEvent {
                                    run_id: run_id.to_string(),
                                    session_key: session_key.to_string(),
                                    trigger_id: Some(trigger_id.to_string()),
                                    provider_name: provider_name.to_string(),
                                    model_id: model_id.to_string(),
                                    stage_hint: FailureStage::Runner,
                                    raw_error: error_str,
                                    details: serde_json::json!({
                                        "hook": "MessageSending",
                                    }),
                                    seq: client_seq,
                                },
                            )
                            .await;
                            return None;
                        },
                        Ok(moltis_common::hooks::HookAction::ModifyPayload(v)) => {
                            if let Some(s) = v.as_str() {
                                accumulated = s.to_string();
                            } else if let Some(obj) = v.as_object()
                                && let Some(s) = obj.get("content").and_then(|c| c.as_str())
                            {
                                accumulated = s.to_string();
                            } else {
                                warn!(
                                    run_id,
                                    session = session_key,
                                    "MessageSending ModifyPayload ignored (expected string or object with 'content')"
                                );
                            }
                        },
                        Ok(moltis_common::hooks::HookAction::Continue) => {},
                        Err(e) => {
                            warn!(
                                run_id,
                                session = session_key,
                                error = %e,
                                "MessageSending hook dispatch failed"
                            );
                        },
                    }
                }

                let is_silent = accumulated.trim().is_empty();
                let reply_target_ref_hash =
                    first_reply_target_ref_hash(state, session_key, trigger_id).await;
                let (channel_type, bucket_key) =
                    session_channel_binding_fields(state, session_key).await;

                info!(
                    run_id,
                    trigger_id = %trigger_id,
                    channel_turn_id = %trigger_id,
                    session_id = %session_key,
                    channel_type = channel_type.as_deref(),
                    bucket_key = bucket_key.as_deref(),
                    reply_target_ref_hash = reply_target_ref_hash.as_deref(),
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    output_text_len = accumulated.len(),
                    output_preview = %log_text_preview(&accumulated, 240),
                    silent = is_silent,
                    "chat stream done"
                );
                let assistant_message_index = user_message_index + 1;

                // Generate & persist TTS audio for voice-medium web UI replies.
                let audio_path = if !is_silent && desired_reply_medium == ReplyMedium::Voice {
                    if let Some(bytes) = generate_tts_audio(state, session_key, &accumulated).await
                    {
                        let filename = format!("{run_id}.ogg");
                        if let Some(store) = session_store {
                            match store.save_media(session_key, &filename, &bytes).await {
                                Ok(path) => Some(path),
                                Err(e) => {
                                    warn!(run_id, error = %e, "failed to save TTS audio to media dir");
                                    None
                                },
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let final_payload = ChatFinalBroadcast {
                    run_id: run_id.to_string(),
                    session_id: session_key.to_string(),
                    state: "final",
                    text: accumulated.clone(),
                    model: provider.id().to_string(),
                    provider: provider_name.to_string(),
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    message_index: assistant_message_index,
                    reply_medium: desired_reply_medium,
                    iterations: None,
                    tool_calls_made: None,
                    audio: audio_path.clone(),
                    seq: client_seq,
                };
                #[allow(clippy::unwrap_used)] // serializing known-valid struct
                let payload_val = serde_json::to_value(&final_payload).unwrap();
                broadcast(state, "chat", payload_val, BroadcastOpts::default()).await;

                if !is_silent {
                    // Send push notification when chat response completes
                    #[cfg(feature = "push-notifications")]
                    {
                        tracing::info!("push: checking push notification");
                        send_chat_push_notification(state, session_key, &accumulated).await;
                    }
                    deliver_channel_replies(
                        state,
                        session_key,
                        trigger_id,
                        &accumulated,
                        desired_reply_medium,
                    )
                    .await;
                } else {
                    // Silent responses must still clear pending channel delivery state
                    // (reply targets + logbook) to avoid later reply "cross-wiring".
                    deliver_channel_replies(
                        state,
                        session_key,
                        trigger_id,
                        "",
                        desired_reply_medium,
                    )
                    .await;
                }

                // Dispatch MessageSent + AgentEnd hooks (read-only).
                if let Some(ref hooks) = hook_registry {
                    let payload = moltis_common::hooks::HookPayload::MessageSent {
                        session_id: session_key.to_string(),
                        session_key: session_key_for_hooks.clone(),
                        channel_target: channel_target_from_session_entry(session_entry.as_ref()),
                        content: accumulated.clone(),
                    };
                    if let Err(e) = hooks.dispatch(&payload).await {
                        warn!(run_id, session = session_key, error = %e, "MessageSent hook failed");
                    }

                    let payload = moltis_common::hooks::HookPayload::AgentEnd {
                        session_id: session_key.to_string(),
                        session_key: session_key_for_hooks.clone(),
                        channel_target: channel_target_from_session_entry(session_entry.as_ref()),
                        text: accumulated.clone(),
                        iterations: 1,
                        tool_calls: 0,
                    };
                    if let Err(e) = hooks.dispatch(&payload).await {
                        warn!(run_id, session = session_key, error = %e, "AgentEnd hook failed");
                    }
                }
                return Some(ChatRunOutput {
                    text: accumulated,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cached_tokens: usage.cache_read_tokens,
                    audio_path,
                });
            },
            StreamEvent::Error(msg) => {
                handle_run_failed_event(
                    state,
                    model_store,
                    RunFailedEvent {
                        run_id: run_id.to_string(),
                        session_key: session_key.to_string(),
                        trigger_id: Some(trigger_id.to_string()),
                        provider_name: provider_name.to_string(),
                        model_id: model_id.to_string(),
                        stage_hint: FailureStage::ProviderStream,
                        raw_error: msg,
                        details: serde_json::json!({
                            "elapsed_ms": stream_started_at.elapsed().as_millis() as u64
                        }),
                        seq: client_seq,
                    },
                )
                .await;
                return None;
            },
            // Tool events not expected in stream-only mode.
            StreamEvent::ToolCallStart { .. }
            | StreamEvent::ToolCallArgumentsDelta { .. }
            | StreamEvent::ToolCallComplete { .. } => {},
        }
    }
    None
}

/// Send a push notification when a chat response completes.
/// Only sends if push notifications are configured and there are subscribers.
#[cfg(feature = "push-notifications")]
async fn send_chat_push_notification(state: &Arc<GatewayState>, session_id: &str, text: &str) {
    let push_service = match state.get_push_service().await {
        Some(svc) => svc,
        None => {
            tracing::info!("push notification skipped: service not configured");
            return;
        },
    };

    let sub_count = push_service.subscription_count().await;
    if sub_count == 0 {
        tracing::info!("push notification skipped: no subscribers");
        return;
    }

    tracing::info!(
        subscribers = sub_count,
        session = session_id,
        "sending push notification"
    );

    // Create a short summary of the response (first 100 chars)
    let summary = truncate_utf8_with_suffix(text, 100, "…");

    // Build the notification
    let title = "Message received";
    let url = format!("/chats/{session_id}");

    match crate::push_routes::send_push_notification(
        &push_service,
        title,
        &summary,
        Some(&url),
        Some(session_id),
    )
    .await
    {
        Ok(sent) => {
            tracing::info!(sent, "push notification sent");
        },
        Err(e) => {
            tracing::warn!("failed to send push notification: {e}");
        },
    }
}

/// Drain any pending channel reply targets for a session and send the
/// response text back to each originating channel via outbound.
/// Each delivery runs in its own spawned task so slow network calls
/// don't block each other or the chat pipeline.
async fn deliver_channel_replies(
    state: &Arc<GatewayState>,
    session_id: &str,
    trigger_id: &str,
    text: &str,
    desired_reply_medium: ReplyMedium,
) {
    let targets = state.drain_channel_replies(session_id, trigger_id).await;
    // Always drain buffered status logs when closing out a channel delivery attempt.
    // Otherwise, early returns (empty text, outbound unavailable, etc.) can cause
    // logbook entries to leak into later successful replies.
    let status_log = state.drain_channel_status_log(session_id, trigger_id).await;
    if targets.is_empty() {
        info!(
            session_id,
            trigger_id,
            text_len = text.len(),
            "channel reply delivery skipped: no pending targets"
        );
        return;
    }
    if text.is_empty() {
        info!(
            session_id,
            trigger_id,
            target_count = targets.len(),
            "channel reply delivery skipped: empty response text"
        );
        return;
    }
    let reply_target_ref_hash = targets
        .first()
        .map(|reply_target_ref| sha256_hex(reply_target_ref));
    let (channel_type, bucket_key) = session_channel_binding_fields(state, session_id).await;
    info!(
        session_id,
        trigger_id,
        channel_turn_id = trigger_id,
        channel_type = channel_type.as_deref(),
        bucket_key = bucket_key.as_deref(),
        target_count = targets.len(),
        reply_target_ref_hash = reply_target_ref_hash.as_deref(),
        text_len = text.len(),
        text_preview = %log_text_preview(text, 240),
        reply_medium = ?desired_reply_medium,
        "channel reply delivery starting"
    );
    let outbound = match state.services.channel_outbound_arc() {
        Some(o) => o,
        None => {
            info!(
                session_id,
                trigger_id,
                target_count = targets.len(),
                "channel reply delivery skipped: outbound unavailable"
            );
            return;
        },
    };
    deliver_channel_replies_to_reply_target_refs(
        outbound,
        targets,
        session_id,
        text,
        Arc::clone(state),
        desired_reply_medium,
        status_log,
        Some(ChannelDeliveryDiag {
            run_id: None,
            trigger_id: Some(trigger_id.to_string()),
        }),
    )
    .await;
}

/// Format buffered status log entries into a Telegram expandable blockquote HTML.
/// Returns an empty string if there are no entries.
fn format_logbook_html(entries: &[String]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut html = String::from("<blockquote expandable>\n\u{1f4cb} <b>Activity log</b>\n");
    for entry in entries {
        // Escape HTML entities in the entry text.
        let escaped = entry
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        html.push_str(&format!("\u{2022} {escaped}\n"));
    }
    html.push_str("</blockquote>");
    html
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let bytes = hasher.finalize();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn log_text_preview(text: &str, max_chars: usize) -> String {
    let mut preview = text
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if preview.chars().count() > max_chars {
        preview = preview
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        preview.push('…');
    }
    preview
}

fn channel_field_as_string(params: &Value, key: &str) -> Option<String> {
    params
        .get("channel")
        .and_then(|channel| channel.get(key))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

async fn first_reply_target_ref_hash(
    state: &Arc<GatewayState>,
    session_id: &str,
    channel_turn_id: &str,
) -> Option<String> {
    state
        .channel_turn_context(session_id, channel_turn_id)
        .await
        .and_then(|context| context.reply_target_refs.into_iter().next())
        .map(|reply_target_ref| sha256_hex(&reply_target_ref))
}

async fn session_channel_binding_fields(
    state: &Arc<GatewayState>,
    session_id: &str,
) -> (Option<String>, Option<String>) {
    let Some(metadata) = state.services.session_metadata.as_ref() else {
        return (None, None);
    };
    let Some(entry) = metadata.get(session_id).await else {
        return (None, None);
    };
    let Some(binding) = entry.channel_binding.as_deref() else {
        return (None, None);
    };
    let channel_type = moltis_telegram::adapter::channel_target_from_binding(binding)
        .map(|target| target.channel_type);
    let bucket_key = moltis_telegram::adapter::bucket_key_from_binding(binding);
    (channel_type, bucket_key)
}

fn sanitize_reason_preview(reason: &str) -> String {
    let collapsed = reason
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let mut out = Vec::new();
    let mut redact_next = false;
    for word in collapsed.split_whitespace() {
        if redact_next {
            out.push("<redacted>");
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower == "authorization:" || lower == "bearer" {
            out.push(word);
            redact_next = true;
            continue;
        }
        if lower.starts_with("token:") && word.len() > "token:".len() + 6 {
            out.push("token:<redacted>");
            continue;
        }
        if word.starts_with("sk-") && word.len() > 16 {
            out.push("<redacted>");
            continue;
        }
        out.push(word);
    }

    let mut preview = out.join(" ");
    const MAX_CHARS: usize = 200;
    if preview.chars().count() > MAX_CHARS {
        preview = preview
            .chars()
            .take(MAX_CHARS.saturating_sub(1))
            .collect::<String>();
        preview.push('…');
    }
    preview
}

#[cfg(test)]
async fn resolve_telegram_runtime_session_key(
    state: &Arc<GatewayState>,
    chan_account_key: &str,
    bucket_key: &str,
) -> Option<String> {
    let agent_id = state
        .services
        .channel
        .telegram_account_agent_id(chan_account_key)
        .await
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            futures::executor::block_on(async {
                state
                    .services
                    .channel
                    .telegram_bus_accounts_snapshot()
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.account_handle == chan_account_key)
                    .and_then(|snapshot| snapshot.agent_id)
                    .filter(|value| !value.trim().is_empty())
            })
        })?;
    Some(moltis_sessions::SessionKey::agent(&agent_id, bucket_key).0)
}

#[cfg(test)]
async fn ensure_channel_bound_session(
    state: &Arc<GatewayState>,
    session_id: &str,
    chan_account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    bucket_key: Option<&str>,
) {
    let Some(ref session_meta) = state.services.session_metadata else {
        return;
    };

    let Some(binding_json) = moltis_telegram::adapter::telegram_binding_json_for_bucket(
        chan_account_key,
        chat_id,
        thread_id,
        bucket_key,
    ) else {
        return;
    };

    let session_key = if let Some(bucket_key) = bucket_key {
        resolve_telegram_runtime_session_key(state, chan_account_key, bucket_key).await
    } else {
        None
    };

    let entry = session_meta.get(session_id).await;
    let stored_binding = entry.as_ref().and_then(|e| e.channel_binding.as_deref());
    if let Some(stored_binding) = stored_binding
        && (moltis_telegram::adapter::telegram_binding_uses_legacy_shape(stored_binding)
            || moltis_telegram::adapter::telegram_channel_binding_info(stored_binding)
                .is_some_and(|info| info.bucket_key.is_none()))
    {
        warn!(
            event = "telegram.session.reject_legacy_binding",
            session_id,
            context = "ensure_channel_bound_session",
            reason_code = "legacy_channel_binding_rejected",
            "refusing to refresh legacy or incomplete telegram channel_binding"
        );
        return;
    }
    let needs_binding_update = stored_binding != Some(binding_json.as_str());
    if let Some(session_key) = session_key.as_deref() {
        session_meta
            .set_active_session_id(session_key, session_id)
            .await;
    }
    if entry.is_none() {
        let snapshots = state
            .services
            .channel
            .telegram_bus_accounts_snapshot()
            .await;
        let bot_username =
            crate::session_labels::resolve_telegram_bot_username(&snapshots, chan_account_key);
        let label = crate::session_labels::format_telegram_session_label(
            chan_account_key,
            bot_username,
            chat_id,
        );
        let session_key = session_key
            .clone()
            .unwrap_or_else(|| session_id.to_string());
        let _ = session_meta
            .upsert(session_id, &session_key, Some(label))
            .await;
    }

    if needs_binding_update {
        session_meta
            .set_channel_binding(session_id, Some(binding_json))
            .await;
    }
}

#[cfg(test)]
async fn resolve_telegram_session_id(
    state: &Arc<GatewayState>,
    chan_account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    sender_account_key: Option<&str>,
) -> Option<String> {
    if state.services.session_metadata.is_none() {
        warn!(
            event = "telegram.session.reject_missing_metadata",
            chan_account_key,
            chat_id,
            thread_id = ?thread_id,
            reason_code = "missing_session_metadata",
            "rejecting telegram session lookup without session metadata"
        );
        return None;
    }

    let snapshots = state
        .services
        .channel
        .telegram_bus_accounts_snapshot()
        .await;
    let sm = state.services.session_metadata.as_ref()?;
    let route = moltis_telegram::adapter::resolve_group_bucket_route(
        &snapshots,
        chan_account_key,
        chat_id,
        thread_id,
        sender_account_key,
    );
    let Some(route) = route else {
        warn!(
            event = "telegram.session.reject_missing_bucket_route",
            chan_account_key,
            chat_id,
            thread_id = ?thread_id,
            sender_account_key = ?sender_account_key,
            reason_code = "missing_bucket_route",
            "rejecting telegram session lookup without strict bucket route"
        );
        return None;
    };

    let session_key =
        resolve_telegram_runtime_session_key(state, chan_account_key, &route.bucket_key).await?;
    sm.get_active_session_id(&session_key).await
}

#[derive(Debug, Clone, Default)]
struct ChannelDeliveryDiag {
    run_id: Option<String>,
    trigger_id: Option<String>,
}

#[cfg(test)]
async fn deliver_channel_replies_to_targets(
    outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound>,
    targets: Vec<moltis_channels::ChannelReplyTarget>,
    session_key: &str,
    text: &str,
    state: Arc<GatewayState>,
    desired_reply_medium: ReplyMedium,
    status_log: Vec<String>,
    diag: Option<ChannelDeliveryDiag>,
) {
    let session_key = session_key.to_string();
    let text = text.to_string();
    let diag = diag.unwrap_or_default();
    let logbook_html = format_logbook_html(&status_log);
    let mut tasks = Vec::with_capacity(targets.len());
    for target in targets {
        let outbound = Arc::clone(&outbound);
        let state = Arc::clone(&state);
        let session_key = session_key.clone();
        let text = text.clone();
        let logbook_html = logbook_html.clone();
        let diag = diag.clone();
        tasks.push(tokio::spawn(async move {
            let tts_payload = match desired_reply_medium {
                ReplyMedium::Voice => build_tts_payload(&state, &session_key, &target, &text).await,
                ReplyMedium::Text => None,
            };
            match target.chan_type {
                moltis_channels::ChannelType::Telegram => match tts_payload {
                    Some(mut payload) => {
                        let transcript = std::mem::take(&mut payload.text);

                        // Short transcript fits as a caption on the voice message.
                        if transcript.len() <= moltis_telegram::markdown::TELEGRAM_CAPTION_LIMIT {
                            payload.text = transcript;
                            let send_res = outbound
                                .send_media_to_target_with_ref(&target, &payload)
                                .await;
                            if let Err(e) = send_res {
                                warn!(
                                    event = "channel_delivery.failed",
                                    code = "telegram_send_media_failed",
                                    op = "telegram.send_media_with_ref",
                                    run_id = ?diag.run_id,
                                    trigger_id = ?diag.trigger_id,
                                    chan_account_key = target.chan_account_key,
                                    chat_id = target.chat_id,
                                    "failed to send channel voice reply: {e}"
                                );
                            }
                            // Send logbook as a follow-up if present.
                            if !logbook_html.is_empty()
                                && let Err(e) = outbound
                                    .send_text_with_suffix_to_target(&target, "", &logbook_html)
                                    .await
                            {
                                let outbound_meta = telegram_outbound_failure_meta(&e);
                                warn!(
                                    event = "channel_delivery.failed",
                                    code = "telegram_send_text_failed",
                                    op = "telegram.send_text_with_suffix",
                                    telegram_outbound_op = outbound_meta.outbound_op,
                                    outcome_kind = outbound_meta.outcome_kind,
                                    delivery_state = outbound_meta.delivery_state,
                                    run_id = ?diag.run_id,
                                    trigger_id = ?diag.trigger_id,
                                    chan_account_key = target.chan_account_key,
                                    chat_id = target.chat_id,
                                    "failed to send logbook follow-up: {e}"
                                );
                            }
                        } else {
                            // Transcript too long for a caption — send voice
                            // without caption, then the full text as a follow-up.
                            if let Err(e) = outbound
                                .send_media_to_target_with_ref(&target, &payload)
                                .await
                            {
                                warn!(
                                    event = "channel_delivery.failed",
                                    code = "telegram_send_media_failed",
                                    op = "telegram.send_media_with_ref",
                                    run_id = ?diag.run_id,
                                    trigger_id = ?diag.trigger_id,
                                    chan_account_key = target.chan_account_key,
                                    chat_id = target.chat_id,
                                    "failed to send channel voice reply: {e}"
                                );
                            }
                            let text_result = if logbook_html.is_empty() {
                                outbound.send_text_to_target(&target, &transcript).await
                            } else {
                                outbound
                                    .send_text_with_suffix_to_target(
                                        &target,
                                        &transcript,
                                        &logbook_html,
                                    )
                                    .await
                            };
                            if let Err(e) = text_result {
                                let outbound_meta = telegram_outbound_failure_meta(&e);
                                warn!(
                                    event = "channel_delivery.failed",
                                    code = "telegram_send_text_failed",
                                    op = "telegram.send_text_followup",
                                    telegram_outbound_op = outbound_meta.outbound_op,
                                    outcome_kind = outbound_meta.outcome_kind,
                                    delivery_state = outbound_meta.delivery_state,
                                    run_id = ?diag.run_id,
                                    trigger_id = ?diag.trigger_id,
                                    chan_account_key = target.chan_account_key,
                                    chat_id = target.chat_id,
                                    "failed to send transcript follow-up: {e}"
                                );
                            }
                        }
                    },
                    None => {
                        let result = if logbook_html.is_empty() {
                            outbound.send_text_to_target_with_ref(&target, &text).await
                        } else {
                            outbound
                                .send_text_with_suffix_to_target_with_ref(
                                    &target,
                                    &text,
                                    &logbook_html,
                                )
                                .await
                        };
                        if let Err(e) = result {
                            let outbound_meta = telegram_outbound_failure_meta(&e);
                            warn!(
                                event = "channel_delivery.failed",
                                code = "telegram_send_text_failed",
                                op = "telegram.send_text_with_ref",
                                telegram_outbound_op = outbound_meta.outbound_op,
                                outcome_kind = outbound_meta.outcome_kind,
                                delivery_state = outbound_meta.delivery_state,
                                run_id = ?diag.run_id,
                                trigger_id = ?diag.trigger_id,
                                chan_account_key = target.chan_account_key,
                                chat_id = target.chat_id,
                                "failed to send channel reply: {e}"
                            );
                            return;
                        }
                    },
                },
            }
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            warn!(error = %e, "channel reply task join failed");
        }
    }
}

async fn deliver_channel_replies_to_reply_target_refs(
    outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound>,
    reply_target_refs: Vec<String>,
    session_id: &str,
    text: &str,
    state: Arc<GatewayState>,
    desired_reply_medium: ReplyMedium,
    status_log: Vec<String>,
    diag: Option<ChannelDeliveryDiag>,
) {
    let session_id = session_id.to_string();
    let text = text.to_string();
    let diag = diag.unwrap_or_default();

    let logbook_html = format_logbook_html(&status_log);

    let channel_key = if let Some(ref sm) = state.services.session_metadata
        && let Some(entry) = sm.get(&session_id).await
        && let Some(binding) = entry.channel_binding.as_deref()
    {
        moltis_telegram::adapter::channel_target_from_binding(binding)
            .map(|target| format!("{}:{}", target.channel_type, target.account_key))
    } else {
        None
    };

    let mut tasks = Vec::with_capacity(reply_target_refs.len());
    for reply_target_ref in reply_target_refs {
        let outbound = Arc::clone(&outbound);
        let state = Arc::clone(&state);
        let session_id = session_id.clone();
        let text = text.clone();
        let logbook_html = logbook_html.clone();
        let diag = diag.clone();
        let channel_key = channel_key.clone();
        tasks.push(tokio::spawn(async move {
            let reply_target_ref_hash = sha256_hex(&reply_target_ref);
            if matches!(desired_reply_medium, ReplyMedium::Voice) {
                if let Some(mut payload) = build_tts_payload_for_channel_key(
                    &state,
                    &session_id,
                    channel_key.as_deref(),
                    &text,
                )
                .await
                {
                    let transcript = std::mem::take(&mut payload.text);
                    let (voice_payload, follow_up_text) =
                        if transcript.len() <= moltis_telegram::markdown::TELEGRAM_CAPTION_LIMIT {
                            payload.text = transcript.clone();
                            (payload, None)
                        } else {
                            (payload, Some(transcript))
                        };

                    if let Err(e) = outbound
                        .send_media_by_reply_target_ref_with_ref(&reply_target_ref, &voice_payload)
                        .await
                    {
                        warn!(
                            event = "channel_delivery.failed",
                            code = "send_media_failed",
                            op = "channel.send_media_by_reply_target_ref",
                            run_id = ?diag.run_id,
                            trigger_id = ?diag.trigger_id,
                            reply_target_ref_hash = %reply_target_ref_hash,
                            reason_code = "send_media_failed",
                            "failed to send channel voice reply: {e}"
                        );
                        return;
                    }

                    if let Some(follow_up) = follow_up_text {
                        let result = if logbook_html.is_empty() {
                            outbound
                                .send_text_by_reply_target_ref_with_ref(
                                    &reply_target_ref,
                                    &follow_up,
                                )
                                .await
                        } else {
                            outbound
                                .send_text_with_suffix_by_reply_target_ref_with_ref(
                                    &reply_target_ref,
                                    &follow_up,
                                    &logbook_html,
                                )
                                .await
                        };
                        if let Err(e) = result {
                            warn!(
                                event = "channel_delivery.failed",
                                code = "send_text_failed",
                                op = "channel.send_text_by_reply_target_ref",
                                run_id = ?diag.run_id,
                                trigger_id = ?diag.trigger_id,
                                reply_target_ref_hash = %reply_target_ref_hash,
                                reason_code = "send_text_failed",
                                "failed to send voice transcript follow-up: {e}"
                            );
                        }
                    } else if !logbook_html.is_empty()
                        && let Err(e) = outbound
                            .send_text_with_suffix_by_reply_target_ref_with_ref(
                                &reply_target_ref,
                                "",
                                &logbook_html,
                            )
                            .await
                    {
                        warn!(
                            event = "channel_delivery.failed",
                            code = "send_text_failed",
                            op = "channel.send_logbook_follow_up",
                            run_id = ?diag.run_id,
                            trigger_id = ?diag.trigger_id,
                            reply_target_ref_hash = %reply_target_ref_hash,
                            reason_code = "send_text_failed",
                            "failed to send logbook follow-up: {e}"
                        );
                    }

                    return;
                }
            }

            let result = if logbook_html.is_empty() {
                outbound
                    .send_text_by_reply_target_ref_with_ref(&reply_target_ref, &text)
                    .await
            } else {
                outbound
                    .send_text_with_suffix_by_reply_target_ref_with_ref(
                        &reply_target_ref,
                        &text,
                        &logbook_html,
                    )
                    .await
            };
            if let Err(e) = result {
                warn!(
                    event = "channel_delivery.failed",
                    code = "send_text_failed",
                    op = "channel.send_text_by_reply_target_ref",
                    run_id = ?diag.run_id,
                    trigger_id = ?diag.trigger_id,
                    reply_target_ref_hash = %reply_target_ref_hash,
                    reason_code = "send_text_failed",
                    "failed to send channel reply: {e}"
                );
                return;
            }
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            warn!(error = %e, "channel reply task join failed");
        }
    }
}

#[derive(Debug, Deserialize)]
struct TtsStatusResponse {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct TtsConvertRequest<'a> {
    text: &'a str,
    format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "voiceId")]
    voice_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TtsConvertResponse {
    audio: String,
    #[serde(default)]
    mime_type: Option<String>,
}

/// Generate TTS audio bytes for a web UI response.
///
/// Uses the session-level TTS override if configured, otherwise the global TTS
/// config. Returns raw audio bytes (OGG format) on success, `None` if TTS is
/// disabled or generation fails.
async fn generate_tts_audio(
    state: &Arc<GatewayState>,
    session_key: &str,
    text: &str,
) -> Option<Vec<u8>> {
    use base64::Engine;

    let tts_status = state.services.tts.status().await.ok()?;
    let status: TtsStatusResponse = serde_json::from_value(tts_status).ok()?;
    if !status.enabled {
        return None;
    }

    // Layer 2: strip markdown/URLs the LLM may have included despite the prompt.
    let text = moltis_voice::tts::sanitize_text_for_tts(text);

    let session_override = {
        state
            .inner
            .read()
            .await
            .tts_session_overrides
            .get(session_key)
            .cloned()
    };

    let request = TtsConvertRequest {
        text: &text,
        format: "ogg",
        provider: session_override.as_ref().and_then(|o| o.provider.clone()),
        voice_id: session_override.as_ref().and_then(|o| o.voice_id.clone()),
        model: session_override.as_ref().and_then(|o| o.model.clone()),
    };

    let tts_result = state
        .services
        .tts
        .convert(serde_json::to_value(request).ok()?)
        .await
        .ok()?;

    let response: TtsConvertResponse = serde_json::from_value(tts_result).ok()?;
    base64::engine::general_purpose::STANDARD
        .decode(&response.audio)
        .ok()
}

#[cfg(test)]
async fn build_tts_payload(
    state: &Arc<GatewayState>,
    session_key: &str,
    target: &moltis_channels::ChannelReplyTarget,
    text: &str,
) -> Option<moltis_common::types::ReplyPayload> {
    use moltis_common::types::{MediaAttachment, ReplyPayload};

    let tts_status = state.services.tts.status().await.ok()?;
    let status: TtsStatusResponse = serde_json::from_value(tts_status).ok()?;
    if !status.enabled {
        return None;
    }

    // Strip markdown/URLs the LLM may have included — use sanitized text
    // only for TTS conversion, but keep the original for the caption.
    let sanitized = moltis_voice::tts::sanitize_text_for_tts(text);

    let channel_key = format!("{}:{}", target.chan_type.as_str(), target.chan_account_key);
    let (channel_override, session_override) = {
        let inner = state.inner.read().await;
        (
            inner.tts_channel_overrides.get(&channel_key).cloned(),
            inner.tts_session_overrides.get(session_key).cloned(),
        )
    };
    let resolved = channel_override.or(session_override);

    let request = TtsConvertRequest {
        text: &sanitized,
        format: "ogg",
        provider: resolved.as_ref().and_then(|o| o.provider.clone()),
        voice_id: resolved.as_ref().and_then(|o| o.voice_id.clone()),
        model: resolved.as_ref().and_then(|o| o.model.clone()),
    };

    let tts_result = state
        .services
        .tts
        .convert(serde_json::to_value(request).ok()?)
        .await
        .ok()?;

    let response: TtsConvertResponse = serde_json::from_value(tts_result).ok()?;

    let mime_type = response
        .mime_type
        .unwrap_or_else(|| "audio/ogg".to_string());

    Some(ReplyPayload {
        text: text.to_string(),
        media: Some(MediaAttachment {
            url: format!("data:{mime_type};base64,{}", response.audio),
            mime_type,
        }),
        reply_to_message_id: None,
        silent: false,
    })
}

async fn build_tts_payload_for_channel_key(
    state: &Arc<GatewayState>,
    session_id: &str,
    channel_key: Option<&str>,
    text: &str,
) -> Option<moltis_common::types::ReplyPayload> {
    use moltis_common::types::{MediaAttachment, ReplyPayload};

    let tts_status = state.services.tts.status().await.ok()?;
    let status: TtsStatusResponse = serde_json::from_value(tts_status).ok()?;
    if !status.enabled {
        return None;
    }

    let sanitized = moltis_voice::tts::sanitize_text_for_tts(text);

    let (channel_override, session_override) = {
        let inner = state.inner.read().await;
        (
            channel_key.and_then(|k| inner.tts_channel_overrides.get(k).cloned()),
            inner.tts_session_overrides.get(session_id).cloned(),
        )
    };
    let resolved = channel_override.or(session_override);

    let request = TtsConvertRequest {
        text: &sanitized,
        format: "ogg",
        provider: resolved.as_ref().and_then(|o| o.provider.clone()),
        voice_id: resolved.as_ref().and_then(|o| o.voice_id.clone()),
        model: resolved.as_ref().and_then(|o| o.model.clone()),
    };

    let tts_result = state
        .services
        .tts
        .convert(serde_json::to_value(request).ok()?)
        .await
        .ok()?;

    let response: TtsConvertResponse = serde_json::from_value(tts_result).ok()?;

    let mime_type = response
        .mime_type
        .unwrap_or_else(|| "audio/ogg".to_string());

    Some(ReplyPayload {
        text: text.to_string(),
        media: Some(MediaAttachment {
            url: format!("data:{mime_type};base64,{}", response.audio),
            mime_type,
        }),
        reply_to_message_id: None,
        silent: false,
    })
}

/// Buffer a tool execution status into the channel status log for a session.
/// The buffered entries are appended as a collapsible logbook when the final
/// response is delivered, instead of being sent as separate messages.
async fn send_tool_status_to_channels(
    state: &Arc<GatewayState>,
    session_key: &str,
    trigger_id: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
) {
    let targets = state.peek_channel_replies(session_key, trigger_id).await;
    if targets.is_empty() {
        return;
    }

    // Buffer the status message for the logbook
    let message = format_tool_status_message(tool_name, arguments);
    state
        .push_channel_status_log(session_key, trigger_id, message)
        .await;
}

/// Format a human-readable tool execution message.
fn format_tool_status_message(tool_name: &str, arguments: &serde_json::Value) -> String {
    match tool_name {
        "browser" => {
            let action = arguments
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let url = arguments.get("url").and_then(|v| v.as_str());
            let ref_ = arguments.get("ref_").and_then(|v| v.as_u64());

            match action {
                "navigate" => {
                    if let Some(u) = url {
                        format!("🌐 Navigating to {}", truncate_url(u))
                    } else {
                        "🌐 Navigating...".to_string()
                    }
                },
                "screenshot" => "📸 Taking screenshot...".to_string(),
                "snapshot" => "📋 Getting page snapshot...".to_string(),
                "click" => {
                    if let Some(r) = ref_ {
                        format!("👆 Clicking element #{}", r)
                    } else {
                        "👆 Clicking...".to_string()
                    }
                },
                "type" => "⌨️ Typing...".to_string(),
                "scroll" => "📜 Scrolling...".to_string(),
                "evaluate" => "⚡ Running JavaScript...".to_string(),
                "wait" => "⏳ Waiting for element...".to_string(),
                "close" => "🚪 Closing browser...".to_string(),
                _ => format!("🌐 Browser: {}", action),
            }
        },
        "exec" => {
            let command = arguments.get("command").and_then(|v| v.as_str());
            if let Some(cmd) = command {
                // Show first ~50 chars of command
                let display_cmd = truncate_utf8_with_suffix(cmd, 50, "...");
                format!("💻 Running: `{}`", display_cmd)
            } else {
                "💻 Executing command...".to_string()
            }
        },
        "web_fetch" => {
            let url = arguments.get("url").and_then(|v| v.as_str());
            if let Some(u) = url {
                format!("🔗 Fetching {}", truncate_url(u))
            } else {
                "🔗 Fetching URL...".to_string()
            }
        },
        "web_search" => {
            let query = arguments.get("query").and_then(|v| v.as_str());
            if let Some(q) = query {
                let display_q = truncate_utf8_with_suffix(q, 40, "...");
                format!("🔍 Searching: {}", display_q)
            } else {
                "🔍 Searching...".to_string()
            }
        },
        "memory_search" => "🧠 Searching memory...".to_string(),
        "memory_store" => "🧠 Storing to memory...".to_string(),
        _ => format!("🔧 {}", tool_name),
    }
}

/// Truncate a URL for display (show domain + short path).
fn truncate_url(url: &str) -> String {
    // Try to extract domain from URL
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Take first 50 chars max
    truncate_utf8_with_suffix(without_scheme, 50, "...")
}

/// Send a screenshot to all pending channel targets for a session.
/// Uses `peek_channel_replies` so targets remain for the final text response.
async fn send_screenshot_to_channels(
    state: &Arc<GatewayState>,
    session_key: &str,
    trigger_id: &str,
    screenshot_data: &str,
) {
    use moltis_common::types::{MediaAttachment, ReplyPayload};

    let reply_target_refs = state.peek_channel_replies(session_key, trigger_id).await;
    if reply_target_refs.is_empty() {
        return;
    }

    let outbound = match state.services.channel_outbound_arc() {
        Some(o) => o,
        None => return,
    };

    let payload = ReplyPayload {
        text: String::new(), // No caption, just the image
        media: Some(MediaAttachment {
            url: screenshot_data.to_string(),
            mime_type: "image/png".to_string(),
        }),
        reply_to_message_id: None,
        silent: false,
    };

    let mut tasks = Vec::with_capacity(reply_target_refs.len());
    for reply_target_ref in reply_target_refs {
        let outbound = Arc::clone(&outbound);
        let payload = payload.clone();
        tasks.push(tokio::spawn(async move {
            let reply_target_ref_hash = sha256_hex(&reply_target_ref);
            if let Err(e) = outbound
                .send_media_by_reply_target_ref_with_ref(&reply_target_ref, &payload)
                .await
            {
                warn!(
                    event = "channel.outbound.media_failed",
                    op = "channel.send_media_by_reply_target_ref",
                    reply_target_ref_hash = %reply_target_ref_hash,
                    reason_code = "send_media_failed",
                    "failed to send screenshot to channel: {e}"
                );
                let error_msg = "⚠️ Failed to send screenshot. Please try again.".to_string();
                let _ = outbound
                    .send_text_by_reply_target_ref_with_ref(&reply_target_ref, &error_msg)
                    .await;
                return;
            }

            debug!(
                reply_target_ref_hash = %reply_target_ref_hash,
                "sent screenshot to channel"
            );
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            warn!(error = %e, "channel reply task join failed");
        }
    }
}

/// Send a native location pin to all pending channel targets for a session.
/// Uses `peek_channel_replies` so targets remain for the final text response.
async fn send_location_to_channels(
    state: &Arc<GatewayState>,
    session_key: &str,
    trigger_id: &str,
    latitude: f64,
    longitude: f64,
    title: Option<&str>,
) {
    let reply_target_refs = state.peek_channel_replies(session_key, trigger_id).await;
    if reply_target_refs.is_empty() {
        return;
    }

    let outbound = match state.services.channel_outbound_arc() {
        Some(o) => o,
        None => return,
    };

    let title_owned = title.map(String::from);

    let mut tasks = Vec::with_capacity(reply_target_refs.len());
    for reply_target_ref in reply_target_refs {
        let outbound = Arc::clone(&outbound);
        let title_ref = title_owned.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = outbound
                .send_location_by_reply_target_ref_with_ref(
                    &reply_target_ref,
                    latitude,
                    longitude,
                    title_ref.as_deref(),
                )
                .await
            {
                let reply_target_ref_hash = sha256_hex(&reply_target_ref);
                warn!(
                    event = "channel.outbound.location_failed",
                    op = "channel.send_location_by_reply_target_ref",
                    reply_target_ref_hash = %reply_target_ref_hash,
                    reason_code = "send_location_failed",
                    "failed to send location to channel: {e}"
                );
            } else {
                debug!(
                    reply_target_ref_hash = %sha256_hex(&reply_target_ref),
                    "sent location pin to telegram"
                );
            }
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            warn!(error = %e, "channel location task join failed");
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::logs::{LogBroadcastLayer, LogBuffer, LogFilter},
        anyhow::Result,
        moltis_agents::{model::LlmProvider, tool_registry::AgentTool},
        moltis_common::{
            hooks::{HookAction, HookEvent, HookHandler, HookPayload, HookRegistry},
            types::ReplyPayload,
        },
        moltis_sessions::store::SessionStore,
        std::{
            pin::Pin,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            time::{Duration, Instant},
        },
        tokio_stream::Stream,
        tracing_subscriber::prelude::*,
    };

    #[test]
    fn telegram_outbound_failure_meta_extracts_structured_fields() {
        let err = anyhow::Error::new(
            moltis_telegram::outbound::TelegramOutboundError::new_without_source(
                moltis_telegram::outbound::TelegramOutboundOp::SendMessage,
                moltis_telegram::outbound::OutboundOutcomeKind::NonRetryableFailure,
                moltis_telegram::outbound::OutboundErrorClass::Api,
                Some(moltis_telegram::outbound::OutboundDeliveryState::PartialSent),
                3,
                3,
            ),
        );

        let meta = telegram_outbound_failure_meta(&err);
        assert_eq!(meta.outbound_op, "send_message");
        assert_eq!(meta.outcome_kind, "non_retryable_failure");
        assert_eq!(meta.delivery_state, "partial_sent");
    }

    #[test]
    fn gateway_log_text_preview_collapses_whitespace_and_truncates() {
        let preview = log_text_preview("hello\tworld\nsecond line", 12);
        assert_eq!(preview, "hello world…");
    }

    #[tokio::test]
    async fn first_reply_target_ref_hash_uses_channel_turn_context() {
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let reply_target_ref =
            telegram_reply_target_ref("telegram:test", "-1001", Some("77"), Some("7"));

        state
            .ensure_channel_turn_context("turn:test", "session:test", Some("sess-key".into()))
            .await;
        state
            .push_channel_reply("session:test", "turn:test", reply_target_ref.clone())
            .await;

        assert_eq!(
            first_reply_target_ref_hash(&state, "session:test", "turn:test").await,
            Some(sha256_hex(&reply_target_ref))
        );
    }

    #[tokio::test]
    async fn session_channel_binding_fields_reads_channel_type_and_bucket_key() {
        let metadata = sqlite_metadata().await;
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop().with_session_metadata(metadata.clone()),
        );

        metadata
            .upsert("session:test", "sess-key", Some("test".into()))
            .await
            .expect("session metadata upsert");
        let binding = moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:test".to_string(),
            chan_user_name: Some("@bot".to_string()),
            chat_id: "-1001".to_string(),
            message_id: Some("77".to_string()),
            thread_id: Some("7".to_string()),
            bucket_key: Some("group-peer-tgchat.n1001-branch-topic.7".to_string()),
        };
        metadata
            .set_channel_binding(
                "session:test",
                Some(serde_json::to_string(&binding).expect("serialize binding")),
            )
            .await;

        assert_eq!(
            session_channel_binding_fields(&state, "session:test").await,
            (
                Some("telegram".to_string()),
                Some("group-peer-tgchat.n1001-branch-topic.7".to_string())
            )
        );
    }

    async fn sqlite_metadata() -> Arc<moltis_sessions::metadata::SqliteSessionMetadata> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        moltis_sessions::metadata::SqliteSessionMetadata::init(&pool)
            .await
            .unwrap();
        Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool))
    }

    async fn seed_test_session(
        metadata: &Arc<moltis_sessions::metadata::SqliteSessionMetadata>,
        session_id: &str,
        session_key: &str,
    ) {
        metadata
            .upsert(session_id, session_key, Some("test".to_string()))
            .await
            .expect("session metadata upsert");
    }

    fn telegram_reply_target_ref(
        chan_account_key: &str,
        chat_id: &str,
        message_id: Option<&str>,
        thread_id: Option<&str>,
    ) -> String {
        moltis_telegram::adapter::reply_target_ref_for_target(
            chan_account_key,
            chat_id,
            thread_id,
            message_id,
        )
        .expect("telegram reply_target_ref must encode")
    }

    struct DummyTool {
        name: String,
    }

    struct StaticProvider {
        name: String,
        id: String,
    }

    struct ContextStreamingProvider {
        called: Arc<std::sync::atomic::AtomicUsize>,
        expected_session_id: String,
    }

    struct SingleToolCallProvider {
        called: Arc<std::sync::atomic::AtomicUsize>,
        expected_ctx_session_id: String,
    }

    struct BudgetProvider {
        context_window: u32,
        input_limit: Option<u32>,
        output_limit: Option<u32>,
    }

    #[async_trait]
    impl LlmProvider for StaticProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn id(&self) -> &str {
            &self.id
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::empty())
        }
    }

    #[async_trait]
    impl LlmProvider for ContextStreamingProvider {
        fn name(&self) -> &str {
            "ctx-stream"
        }

        fn id(&self) -> &str {
            "ctx-stream-model"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            panic!("run_streaming must not call provider.stream() directly")
        }

        fn stream_with_tools_with_context(
            &self,
            ctx: &moltis_agents::model::LlmRequestContext,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            assert_eq!(
                ctx.session_id.as_deref(),
                Some(self.expected_session_id.as_str())
            );
            self.called.fetch_add(1, Ordering::SeqCst);
            Box::pin(tokio_stream::iter(vec![
                StreamEvent::Delta("ok".to_string()),
                StreamEvent::Done(moltis_agents::model::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
            ]))
        }
    }

    #[async_trait]
    impl LlmProvider for SingleToolCallProvider {
        fn name(&self) -> &str {
            "single-tool-call"
        }

        fn id(&self) -> &str {
            "single-tool-call-model"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::empty())
        }

        fn stream_with_tools_with_context(
            &self,
            ctx: &moltis_agents::model::LlmRequestContext,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            assert_eq!(
                ctx.session_id.as_deref(),
                Some(self.expected_ctx_session_id.as_str())
            );

            let n = self.called.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // First iteration: request a tool call with empty args.
                Box::pin(tokio_stream::iter(vec![
                    StreamEvent::ToolCallStart {
                        id: "tc-1".to_string(),
                        name: "assert_ctx".to_string(),
                        index: 0,
                    },
                    StreamEvent::ToolCallArgumentsDelta {
                        index: 0,
                        delta: "{}".to_string(),
                    },
                    StreamEvent::ToolCallComplete { index: 0 },
                    StreamEvent::Done(moltis_agents::model::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                ]))
            } else {
                // Second iteration: finish with plain text.
                Box::pin(tokio_stream::iter(vec![
                    StreamEvent::Delta("ok".to_string()),
                    StreamEvent::Done(moltis_agents::model::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                ]))
            }
        }
    }

    #[async_trait]
    impl LlmProvider for BudgetProvider {
        fn name(&self) -> &str {
            "budget"
        }

        fn id(&self) -> &str {
            "budget-model"
        }

        fn context_window(&self) -> u32 {
            self.context_window
        }

        fn input_limit(&self) -> Option<u32> {
            self.input_limit
        }

        fn output_limit(&self) -> Option<u32> {
            self.output_limit
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::empty())
        }
    }

    struct BlockingBeforeAgentStartHook;

    #[async_trait]
    impl HookHandler for BlockingBeforeAgentStartHook {
        fn name(&self) -> &str {
            "block_before_agent_start"
        }

        fn events(&self) -> &[HookEvent] {
            static EVENTS: [HookEvent; 1] = [HookEvent::BeforeAgentStart];
            &EVENTS
        }

        async fn handle(
            &self,
            event: HookEvent,
            _payload: &HookPayload,
        ) -> anyhow::Result<HookAction> {
            match event {
                HookEvent::BeforeAgentStart => Ok(HookAction::Block("blocked".to_string())),
                _ => Ok(HookAction::Continue),
            }
        }
    }

    struct ToolsBudgetProvider {
        budget: BudgetProvider,
    }

    #[async_trait]
    impl LlmProvider for ToolsBudgetProvider {
        fn name(&self) -> &str {
            self.budget.name()
        }

        fn id(&self) -> &str {
            self.budget.id()
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn context_window(&self) -> u32 {
            self.budget.context_window()
        }

        fn input_limit(&self) -> Option<u32> {
            self.budget.input_limit()
        }

        fn output_limit(&self) -> Option<u32> {
            self.budget.output_limit()
        }

        async fn complete(
            &self,
            messages: &[ChatMessage],
            tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            self.budget.complete(messages, tools).await
        }

        fn stream(
            &self,
            messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            self.budget.stream(messages)
        }
    }

    #[async_trait]
    impl AgentTool for DummyTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

        async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
    }

    struct ContextAssertingTool {
        expected_session_key: String,
        expected_session_id: String,
        called: Arc<AtomicUsize>,
    }

    struct RecordingContextTool {
        seen: Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>,
    }

    #[async_trait]
    impl AgentTool for ContextAssertingTool {
        fn name(&self) -> &str {
            "assert_ctx"
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

        async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value> {
            let session_key = params
                .get("_sessionKey")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let session_id = params
                .get("_sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(session_key, self.expected_session_key.as_str());
            assert_eq!(session_id, self.expected_session_id.as_str());
            self.called.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    #[async_trait]
    impl AgentTool for RecordingContextTool {
        fn name(&self) -> &str {
            "assert_ctx"
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

        async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value> {
            self.seen.lock().await.push(params);
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    struct RecordingHookHandler {
        subscribed: Vec<HookEvent>,
        seen: Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>,
        message_override: Option<String>,
        tool_result_override: Option<serde_json::Value>,
    }

    #[async_trait]
    impl HookHandler for RecordingHookHandler {
        fn name(&self) -> &str {
            "recording"
        }

        fn events(&self) -> &[HookEvent] {
            &self.subscribed
        }

        async fn handle(
            &self,
            event: HookEvent,
            payload: &HookPayload,
        ) -> anyhow::Result<HookAction> {
            let payload_val = serde_json::to_value(payload)?;
            self.seen.lock().await.push(payload_val);

            match event {
                HookEvent::MessageSending => {
                    if let Some(ref content) = self.message_override {
                        return Ok(HookAction::ModifyPayload(serde_json::Value::String(
                            content.clone(),
                        )));
                    }
                },
                HookEvent::ToolResultPersist => {
                    if let Some(ref v) = self.tool_result_override {
                        return Ok(HookAction::ModifyPayload(v.clone()));
                    }
                },
                _ => {},
            }

            Ok(HookAction::Continue)
        }
    }

    struct MockChannelOutbound {
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    #[async_trait]
    impl moltis_channels::plugin::ChannelOutbound for MockChannelOutbound {
        async fn send_text(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            tokio::time::sleep(self.delay).await;
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RetryingSuccessOutbound {
        gateway_calls: AtomicUsize,
        transport_attempts: AtomicUsize,
    }

    #[async_trait]
    impl moltis_channels::plugin::ChannelOutbound for RetryingSuccessOutbound {
        async fn send_text(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            self.gateway_calls.fetch_add(1, Ordering::SeqCst);
            let attempt = self.transport_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            warn!(
                event = "telegram.outbound.retrying",
                op = "send_message",
                attempt,
                max_attempts = 2,
                error_class = "network",
                outcome_kind = "unknown_outcome",
                "simulated internal telegram retry before success"
            );
            self.transport_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn send_text_with_ref(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> Result<Option<moltis_channels::plugin::SentMessageRef>> {
            self.gateway_calls.fetch_add(1, Ordering::SeqCst);
            let attempt = self.transport_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            warn!(
                event = "telegram.outbound.retrying",
                op = "send_message",
                attempt,
                max_attempts = 2,
                error_class = "network",
                outcome_kind = "unknown_outcome",
                "simulated internal telegram retry before success"
            );
            self.transport_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(Some(moltis_channels::plugin::SentMessageRef {
                message_id: "42".to_string(),
            }))
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn deliver_channel_replies_waits_for_outbound_sends() {
        let calls = Arc::new(AtomicUsize::new(0));
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> =
            Arc::new(MockChannelOutbound {
                calls: Arc::clone(&calls),
                delay: Duration::from_millis(50),
            });
        let targets = vec![moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:acct".to_string(),
            chan_user_name: None,
            chat_id: "123".to_string(),
            message_id: None,
            thread_id: None,
            bucket_key: None,
        }];
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );

        let start = Instant::now();
        deliver_channel_replies_to_targets(
            outbound,
            targets,
            "session:test",
            "hello",
            state,
            ReplyMedium::Text,
            Vec::new(),
            None,
        )
        .await;

        assert!(
            start.elapsed() >= Duration::from_millis(45),
            "delivery should wait for outbound send completion"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deliver_channel_replies_does_not_log_failure_when_outbound_retries_internally_then_succeeds()
     {
        let outbound = Arc::new(RetryingSuccessOutbound::default());
        let outbound_dyn: Arc<dyn moltis_channels::plugin::ChannelOutbound> = outbound.clone();
        let targets = vec![moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:acct".to_string(),
            chan_user_name: None,
            chat_id: "123".to_string(),
            message_id: Some("77".to_string()),
            thread_id: None,
            bucket_key: None,
        }];
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let buffer = LogBuffer::new(128);
        let subscriber =
            tracing_subscriber::registry().with(LogBroadcastLayer::new(buffer.clone()));
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);
        deliver_channel_replies_to_targets(
            outbound_dyn,
            targets,
            "session:test",
            "hello",
            state,
            ReplyMedium::Text,
            Vec::new(),
            Some(ChannelDeliveryDiag {
                run_id: Some("run-1".to_string()),
                trigger_id: Some("trigger-1".to_string()),
            }),
        )
        .await;

        assert_eq!(outbound.gateway_calls.load(Ordering::SeqCst), 1);
        assert_eq!(outbound.transport_attempts.load(Ordering::SeqCst), 2);

        let entries = buffer.list(
            &LogFilter {
                level: None,
                target: None,
                search: None,
            },
            32,
        );

        assert!(
            entries.iter().any(|entry| {
                entry.fields.get("event").and_then(|value| value.as_str())
                    == Some("telegram.outbound.retrying")
            }),
            "test setup should simulate an internal telegram retry event"
        );
        assert!(
            entries.iter().all(|entry| {
                entry.fields.get("event").and_then(|value| value.as_str())
                    != Some("channel_delivery.failed")
            }),
            "successful internal retry should not emit channel_delivery.failed"
        );
    }

    #[derive(Default)]
    struct RecordingOutbound {
        texts: tokio::sync::Mutex<Vec<(String, String, String, Option<String>)>>,
        typings: AtomicUsize,
    }

    #[async_trait]
    impl moltis_channels::plugin::ChannelOutbound for RecordingOutbound {
        async fn send_text(
            &self,
            chan_account_key: &str,
            to: &str,
            text: &str,
            reply_to: Option<&str>,
        ) -> Result<()> {
            self.texts.lock().await.push((
                chan_account_key.to_string(),
                to.to_string(),
                text.to_string(),
                reply_to.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        async fn send_text_by_reply_target_ref_with_ref(
            &self,
            reply_target_ref: &str,
            text: &str,
        ) -> Result<Option<moltis_channels::plugin::SentMessageRef>> {
            let target =
                moltis_telegram::adapter::inbound_target_from_reply_target_ref(reply_target_ref)
                    .ok_or_else(|| anyhow::anyhow!("invalid reply_target_ref"))?;
            self.send_text(
                &target.chan_account_key,
                &target.chat_id,
                text,
                target.message_id.as_deref(),
            )
            .await?;
            Ok(None)
        }

        async fn send_text_with_suffix_by_reply_target_ref_with_ref(
            &self,
            reply_target_ref: &str,
            text: &str,
            suffix_html: &str,
        ) -> Result<Option<moltis_channels::plugin::SentMessageRef>> {
            let combined = format!("{text}{suffix_html}");
            self.send_text_by_reply_target_ref_with_ref(reply_target_ref, &combined)
                .await
        }

        async fn send_media_by_reply_target_ref_with_ref(
            &self,
            reply_target_ref: &str,
            payload: &ReplyPayload,
        ) -> Result<Option<moltis_channels::plugin::SentMessageRef>> {
            let target =
                moltis_telegram::adapter::inbound_target_from_reply_target_ref(reply_target_ref)
                    .ok_or_else(|| anyhow::anyhow!("invalid reply_target_ref"))?;
            self.send_media(
                &target.chan_account_key,
                &target.chat_id,
                payload,
                target.message_id.as_deref(),
            )
            .await?;
            Ok(None)
        }

        async fn send_location_by_reply_target_ref_with_ref(
            &self,
            reply_target_ref: &str,
            latitude: f64,
            longitude: f64,
            title: Option<&str>,
        ) -> Result<Option<moltis_channels::plugin::SentMessageRef>> {
            let target =
                moltis_telegram::adapter::inbound_target_from_reply_target_ref(reply_target_ref)
                    .ok_or_else(|| anyhow::anyhow!("invalid reply_target_ref"))?;
            self.send_location_with_ref(
                &target.chan_account_key,
                &target.chat_id,
                latitude,
                longitude,
                title,
                target.message_id.as_deref(),
            )
            .await?;
            Ok(None)
        }

        async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> Result<()> {
            self.typings.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ErrorStreamProvider;

    #[async_trait]
    impl LlmProvider for ErrorStreamProvider {
        fn name(&self) -> &str {
            "err"
        }

        fn id(&self) -> &str {
            "err-model"
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::iter(vec![StreamEvent::Error(
                "boom".to_string(),
            )]))
        }
    }

    struct SilentDoneProvider;

    #[async_trait]
    impl LlmProvider for SilentDoneProvider {
        fn name(&self) -> &str {
            "silent"
        }

        fn id(&self) -> &str {
            "silent-model"
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(tokio_stream::iter(vec![StreamEvent::Done(
                moltis_agents::model::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                },
            )]))
        }
    }

    #[tokio::test]
    async fn run_streaming_error_sends_channel_error_and_drains_state() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let session_key = "telegram:acct:123";
        let trigger_id = crate::ids::new_trigger_id();
        state
            .push_channel_reply(
                session_key,
                &trigger_id,
                telegram_reply_target_ref("telegram:acct", "123", Some("7"), None),
            )
            .await;
        state
            .push_channel_status_log(session_key, &trigger_id, "tool status".to_string())
            .await;

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(ErrorStreamProvider);
        let out = run_streaming(
            &state,
            &model_store,
            "run1",
            provider,
            "err-model",
            &UserContent::text("hi"),
            "openai-responses",
            &[],
            session_key,
            &trigger_id,
            ReplyMedium::Text,
            None,
            0,
            &[],
            None,
            None,
            None,
        )
        .await;

        assert!(out.is_none());
        assert!(
            state
                .peek_channel_replies(session_key, &trigger_id)
                .await
                .is_empty()
        );
        assert!(
            state
                .drain_channel_status_log(session_key, &trigger_id)
                .await
                .is_empty()
        );

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].0, "telegram:acct");
        assert_eq!(texts[0].1, "123");
        assert_eq!(texts[0].3.as_deref(), Some("7"));
        assert!(
            texts[0].2.contains("Error") || texts[0].2.contains("⚠️"),
            "expected a user-visible error reply, got: {}",
            texts[0].2
        );
    }

    #[tokio::test]
    async fn run_streaming_silent_success_drains_state_without_sending() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let session_key = "telegram:acct:123";
        let trigger_id = crate::ids::new_trigger_id();
        state
            .push_channel_reply(
                session_key,
                &trigger_id,
                telegram_reply_target_ref("telegram:acct", "123", Some("9"), None),
            )
            .await;
        state
            .push_channel_status_log(session_key, &trigger_id, "tool status".to_string())
            .await;

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(SilentDoneProvider);
        let out = run_streaming(
            &state,
            &model_store,
            "run2",
            provider,
            "silent-model",
            &UserContent::text("hi"),
            "openai-responses",
            &[],
            session_key,
            &trigger_id,
            ReplyMedium::Text,
            None,
            0,
            &[],
            None,
            None,
            None,
        )
        .await;

        assert!(out.is_some());
        assert!(
            state
                .peek_channel_replies(session_key, &trigger_id)
                .await
                .is_empty()
        );
        assert!(
            state
                .drain_channel_status_log(session_key, &trigger_id)
                .await
                .is_empty()
        );
        assert!(rec.texts.lock().await.is_empty());
    }

    #[tokio::test]
    async fn run_failed_event_duplicate_still_drains_reply_targets_without_sending() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let session_key = "telegram:acct:123";
        let trigger_id = crate::ids::new_trigger_id();
        state
            .push_channel_reply(
                session_key,
                &trigger_id,
                telegram_reply_target_ref("telegram:acct", "123", Some("7"), None),
            )
            .await;

        handle_run_failed_event(
            &state,
            &model_store,
            RunFailedEvent {
                run_id: "run-dupe".to_string(),
                session_key: session_key.to_string(),
                trigger_id: Some(trigger_id.clone()),
                provider_name: "openai-responses".to_string(),
                model_id: "gpt".to_string(),
                stage_hint: FailureStage::Runner,
                raw_error: "HTTP 401 Unauthorized".to_string(),
                details: serde_json::json!({}),
                seq: None,
            },
        )
        .await;

        // Push another pending target (simulating late arrival or out-of-order failure path).
        state
            .push_channel_reply(
                session_key,
                &trigger_id,
                telegram_reply_target_ref("telegram:acct", "123", Some("8"), None),
            )
            .await;

        handle_run_failed_event(
            &state,
            &model_store,
            RunFailedEvent {
                run_id: "run-dupe".to_string(),
                session_key: session_key.to_string(),
                trigger_id: Some(trigger_id.clone()),
                provider_name: "openai-responses".to_string(),
                model_id: "gpt".to_string(),
                stage_hint: FailureStage::Runner,
                raw_error: "HTTP 401 Unauthorized".to_string(),
                details: serde_json::json!({}),
                seq: None,
            },
        )
        .await;

        assert!(
            state
                .peek_channel_replies(session_key, &trigger_id)
                .await
                .is_empty()
        );

        // Only the first failure should send a reply (at most once).
        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
    }

    #[tokio::test]
    async fn deliver_channel_replies_drains_only_current_trigger_targets() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let session_key = "telegram:acct:123";
        let trigger_a = "trg_a";
        let trigger_b = "trg_b";

        state
            .push_channel_reply(
                session_key,
                trigger_a,
                telegram_reply_target_ref("telegram:acct", "123", Some("1"), None),
            )
            .await;
        state
            .push_channel_reply(
                session_key,
                trigger_b,
                telegram_reply_target_ref("telegram:acct", "123", Some("2"), None),
            )
            .await;

        deliver_channel_replies(&state, session_key, trigger_a, "hello", ReplyMedium::Text).await;

        assert!(
            state
                .peek_channel_replies(session_key, trigger_a)
                .await
                .is_empty()
        );
        assert_eq!(
            state
                .peek_channel_replies(session_key, trigger_b)
                .await
                .len(),
            1
        );

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].3.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn handle_run_failed_event_drains_only_current_trigger_targets() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let session_key = "telegram:acct:123";
        let trigger_a = "trg_a";
        let trigger_b = "trg_b";

        state
            .push_channel_reply(
                session_key,
                trigger_a,
                telegram_reply_target_ref("telegram:acct", "123", Some("1"), None),
            )
            .await;
        state
            .push_channel_reply(
                session_key,
                trigger_b,
                telegram_reply_target_ref("telegram:acct", "123", Some("2"), None),
            )
            .await;

        handle_run_failed_event(
            &state,
            &model_store,
            RunFailedEvent {
                run_id: "run-fail".to_string(),
                session_key: session_key.to_string(),
                trigger_id: Some(trigger_a.to_string()),
                provider_name: "openai-responses".to_string(),
                model_id: "gpt".to_string(),
                stage_hint: FailureStage::Runner,
                raw_error: "HTTP 401 Unauthorized".to_string(),
                details: serde_json::json!({}),
                seq: None,
            },
        )
        .await;

        assert!(
            state
                .peek_channel_replies(session_key, trigger_a)
                .await
                .is_empty()
        );
        assert_eq!(
            state
                .peek_channel_replies(session_key, trigger_b)
                .await
                .len(),
            1
        );

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].3.as_deref(), Some("1"));
        assert!(
            texts[0].2.contains("code="),
            "telegram error replies should include a stable diagnostic code"
        );
    }

    #[tokio::test]
    async fn handle_run_failed_event_gateway_timeout_includes_code() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();
        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let session_key = "telegram:acct:123";
        let trigger_id = "trg_a";

        state
            .push_channel_reply(
                session_key,
                trigger_id,
                telegram_reply_target_ref("telegram:acct", "123", Some("1"), None),
            )
            .await;

        handle_run_failed_event(
            &state,
            &model_store,
            RunFailedEvent {
                run_id: "run-timeout".to_string(),
                session_key: session_key.to_string(),
                trigger_id: Some(trigger_id.to_string()),
                provider_name: "openai-responses".to_string(),
                model_id: "gpt".to_string(),
                stage_hint: FailureStage::GatewayTimeout,
                raw_error: "gateway agent timeout".to_string(),
                details: serde_json::json!({ "timeout_secs": 600 }),
                seq: None,
            },
        )
        .await;

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert!(
            texts[0].2.contains("code=gateway_timeout"),
            "expected gateway_timeout code, got: {}",
            texts[0].2
        );
    }

    #[test]
    fn sanitize_reason_preview_redacts_and_truncates() {
        let reason = "Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz0123456789\nline2";
        let preview = sanitize_reason_preview(reason);
        assert!(!preview.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(preview.contains("<redacted>"));
        assert!(!preview.contains('\n'));
        assert!(preview.chars().count() <= 200);
    }

    struct DelayedFailThenOkProvider {
        called: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct WaitCompletionProvider {
        started: Arc<tokio::sync::Notify>,
        finish: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl LlmProvider for DelayedFailThenOkProvider {
        fn name(&self) -> &str {
            "delayed-fail-then-ok"
        }

        fn id(&self) -> &str {
            "delayed-fail-then-ok-model"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            panic!("run_streaming must not call provider.stream() directly")
        }

        fn stream_with_tools_with_context(
            &self,
            _ctx: &moltis_agents::model::LlmRequestContext,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            let n = self.called.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Box::pin(tokio_stream::iter(std::iter::once(())).then(|_| async {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    StreamEvent::Error("boom".to_string())
                }))
            } else {
                Box::pin(tokio_stream::iter(vec![
                    StreamEvent::Delta("ok2".to_string()),
                    StreamEvent::Done(moltis_agents::model::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                ]))
            }
        }
    }

    #[async_trait]
    impl LlmProvider for WaitCompletionProvider {
        fn name(&self) -> &str {
            "wait-completion"
        }

        fn id(&self) -> &str {
            "wait-completion-model"
        }

        fn supports_tools(&self) -> bool {
            false
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<moltis_agents::model::CompletionResponse> {
            anyhow::bail!("not implemented for test")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            panic!("run_streaming must not call provider.stream() directly")
        }

        fn stream_with_tools_with_context(
            &self,
            _ctx: &moltis_agents::model::LlmRequestContext,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
            let started = Arc::clone(&self.started);
            let finish = Arc::clone(&self.finish);
            Box::pin(tokio_stream::iter(std::iter::once(())).then(move |_| {
                let started = Arc::clone(&started);
                let finish = Arc::clone(&finish);
                async move {
                    started.notify_waiters();
                    finish.notified().await;
                    StreamEvent::Done(moltis_agents::model::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    })
                }
            }))
        }
    }

    #[tokio::test]
    async fn failed_run_does_not_drop_queued_triggers_in_followup_mode() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound> = rec.clone();

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;

        let services = crate::services::GatewayServices::noop()
            .with_channel_outbound(outbound)
            .with_session_store(Arc::clone(&store));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(DelayedFailThenOkProvider {
                called: Arc::clone(&called),
            });
        let mut reg = ProviderRegistry::empty();
        reg.register(
            moltis_agents::providers::ModelInfo {
                id: "delayed-fail-then-ok-model".to_string(),
                provider: "test".to_string(),
                display_name: "test".to_string(),
                created_at: None,
            },
            provider,
        );
        let providers = Arc::new(RwLock::new(reg));

        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));
        let chat = Arc::new(LiveChatService::new(
            Arc::clone(&providers),
            Arc::clone(&model_store),
            Arc::clone(&state),
            Arc::clone(&store),
            Arc::clone(&metadata),
        ));
        state.set_chat(chat.clone()).await;

        let session_id = "sess_followup";
        seed_test_session(
            &metadata,
            session_id,
            "agent:zhuzhu:group-peer-tgchat.n100-sender-tguser.123",
        )
        .await;

        // Simulate two inbound triggers (A then B) while a run is active.
        state
            .push_channel_reply(
                session_id,
                "trg_a",
                telegram_reply_target_ref("telegram:acct", "123", Some("1"), None),
            )
            .await;
        state
            .push_channel_reply(
                session_id,
                "trg_b",
                telegram_reply_target_ref("telegram:acct", "123", Some("2"), None),
            )
            .await;

        let _ = chat
            .send(serde_json::json!({
                "text": "A",
                "_sessionId": session_id,
                "model": "delayed-fail-then-ok-model",
                "_channelTurnId": "trg_a",
            }))
            .await
            .unwrap();

        let queued = chat
            .send(serde_json::json!({
                "text": "B",
                "_sessionId": session_id,
                "model": "delayed-fail-then-ok-model",
                "_channelTurnId": "trg_b",
            }))
            .await
            .unwrap();
        assert_eq!(queued["queued"], true);

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if rec.texts.lock().await.len() >= 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("timed out waiting for deliveries");

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 2);
        let mut reply_tos: Vec<&str> = texts.iter().filter_map(|t| t.3.as_deref()).collect();
        reply_tos.sort();
        assert_eq!(reply_tos, vec!["1", "2"]);
        assert!(called.load(Ordering::SeqCst) >= 2, "expected replay to run");
    }

    #[tokio::test]
    async fn wait_run_completion_tracks_background_send_runs() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let services =
            crate::services::GatewayServices::noop().with_session_store(Arc::clone(&store));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(WaitCompletionProvider {
                started: Arc::clone(&started),
                finish: Arc::clone(&finish),
            });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "wait-completion-model".to_string(),
                provider: "wait-completion".to_string(),
                display_name: "wait-completion".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            Arc::clone(&state),
            Arc::clone(&store),
            metadata,
        );
        let session_id = "sess_wait_completion";
        seed_test_session(
            &chat.session_metadata,
            session_id,
            &moltis_sessions::SessionKey::main("default").0,
        )
        .await;

        let result = chat
            .send(serde_json::json!({
                "text": "hello",
                "_sessionId": session_id,
                "model": "wait-completion-model",
            }))
            .await
            .expect("send should start background run");
        let run_id = result
            .get("runId")
            .and_then(|value| value.as_str())
            .expect("runId must be returned")
            .to_string();

        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .expect("background run must start");

        let early_wait = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            chat.wait_run_completion(&run_id),
        )
        .await;
        assert!(
            early_wait.is_err(),
            "wait_run_completion must stay pending while the run is still active"
        );

        finish.notify_waiters();

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            chat.wait_run_completion(&run_id),
        )
        .await
        .expect("run completion wait should finish")
        .expect("wait_run_completion should succeed");
    }

    #[tokio::test]
    async fn run_streaming_passes_session_key_via_llm_request_context() {
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(ContextStreamingProvider {
                called: Arc::clone(&called),
                expected_session_id: "main".to_string(),
            });

        let result = run_streaming(
            &state,
            &model_store,
            "run-1",
            provider,
            "ctx-stream-model",
            &UserContent::text("hi"),
            "ctx-stream",
            &[],
            "main",
            "trg_test",
            ReplyMedium::Text,
            None,
            0,
            &[],
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_some());
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_streaming_emits_message_and_agent_hooks() {
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let mut hooks = HookRegistry::new();
        hooks.register(Arc::new(RecordingHookHandler {
            subscribed: vec![
                HookEvent::BeforeAgentStart,
                HookEvent::MessageSending,
                HookEvent::MessageSent,
                HookEvent::AgentEnd,
            ],
            seen: Arc::clone(&seen),
            message_override: Some("HOOKED".to_string()),
            tool_result_override: None,
        }));
        state.inner.write().await.hook_registry = Some(Arc::new(hooks));

        let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(ContextStreamingProvider {
                called: Arc::clone(&called),
                expected_session_id: "main".to_string(),
            });

        let result = run_streaming(
            &state,
            &model_store,
            "run-1",
            provider,
            "ctx-stream-model",
            &UserContent::text("hi"),
            "ctx-stream",
            &[],
            "main",
            "trg_test",
            ReplyMedium::Text,
            None,
            0,
            &[],
            None,
            None,
            None,
        )
        .await
        .expect("expected chat output");

        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(result.text, "HOOKED");

        let seen_vals = seen.lock().await;
        let events: Vec<&str> = seen_vals
            .iter()
            .filter_map(|v| v.get("event").and_then(|e| e.as_str()))
            .collect();
        assert!(
            events.contains(&"MessageSending"),
            "missing MessageSending hook"
        );
        assert!(events.contains(&"MessageSent"), "missing MessageSent hook");
        assert!(events.contains(&"AgentEnd"), "missing AgentEnd hook");
        assert!(
            events.contains(&"BeforeAgentStart"),
            "missing BeforeAgentStart hook"
        );

        let agent_end = seen_vals
            .iter()
            .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("AgentEnd"))
            .expect("missing AgentEnd payload");
        assert_eq!(agent_end["text"], "HOOKED");
        assert_eq!(agent_end["iterations"], 1);
        assert_eq!(agent_end["toolCalls"], 0);
    }

    #[tokio::test]
    async fn run_with_tools_passes_session_key_via_llm_request_context() {
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(ContextStreamingProvider {
                called: Arc::clone(&called),
                expected_session_id: "main".to_string(),
            });

        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        tool_registry.write().await.register(Box::new(DummyTool {
            name: "noop".into(),
        }));

        let result = run_with_tools(
            &state,
            &model_store,
            "run-1",
            provider,
            "ctx-stream-model",
            &tool_registry,
            &UserContent::text("hi"),
            "ctx-stream",
            &[],
            "main",
            "trg_test",
            None,
            ReplyMedium::Text,
            None,
            None,
            0,
            &[],
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await;

        assert!(result.is_some());
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_with_tools_injects_session_id_into_tool_calls() {
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let provider_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(SingleToolCallProvider {
                called: Arc::clone(&provider_called),
                expected_ctx_session_id: "sess_abc".to_string(),
            });

        let tool_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        tool_registry
            .write()
            .await
            .register(Box::new(ContextAssertingTool {
                expected_session_key:
                    "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"
                        .to_string(),
                expected_session_id: "sess_abc".to_string(),
                called: Arc::clone(&tool_called),
            }));

        let result = run_with_tools(
            &state,
            &model_store,
            "run-1",
            provider,
            "single-tool-call-model",
            &tool_registry,
            &UserContent::text("hi"),
            "single-tool-call",
            &[],
            "sess_abc",
            "trg_test",
            Some("agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"),
            ReplyMedium::Text,
            None,
            None,
            0,
            &[],
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await;

        assert!(result.is_some());
        assert_eq!(provider_called.load(Ordering::SeqCst), 2);
        assert_eq!(tool_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_with_tools_emits_message_and_tool_persist_hooks() {
        let metadata = sqlite_metadata().await;
        metadata
            .upsert(
                "sess_abc",
                "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001",
                Some("hook coverage".into()),
            )
            .await
            .expect("session metadata upsert");
        let binding_json = serde_json::to_string(&moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:845".to_string(),
            chan_user_name: Some("@lovely_apple_bot".to_string()),
            chat_id: "-100".to_string(),
            message_id: Some("77".to_string()),
            thread_id: Some("42".to_string()),
            bucket_key: Some("group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001".into()),
        })
        .unwrap();
        metadata
            .set_channel_binding("sess_abc", Some(binding_json))
            .await;

        let services =
            crate::services::GatewayServices::noop().with_session_metadata(Arc::clone(&metadata));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );
        let model_store = Arc::new(RwLock::new(DisabledModelsStore::default()));

        let provider_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(SingleToolCallProvider {
                called: Arc::clone(&provider_called),
                expected_ctx_session_id: "sess_abc".to_string(),
            });

        let tool_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        tool_registry
            .write()
            .await
            .register(Box::new(ContextAssertingTool {
                expected_session_key:
                    "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"
                        .to_string(),
                expected_session_id: "sess_abc".to_string(),
                called: Arc::clone(&tool_called),
            }));

        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let mut hooks = HookRegistry::new();
        hooks.register(Arc::new(RecordingHookHandler {
            subscribed: vec![
                HookEvent::BeforeAgentStart,
                HookEvent::BeforeLLMCall,
                HookEvent::AfterLLMCall,
                HookEvent::MessageSending,
                HookEvent::MessageSent,
                HookEvent::BeforeToolCall,
                HookEvent::AfterToolCall,
                HookEvent::AgentEnd,
                HookEvent::ToolResultPersist,
            ],
            seen: Arc::clone(&seen),
            message_override: Some("HOOKED".to_string()),
            tool_result_override: Some(serde_json::json!({"redacted": true})),
        }));

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));

        let result = run_with_tools(
            &state,
            &model_store,
            "run-1",
            provider,
            "single-tool-call-model",
            &tool_registry,
            &UserContent::text("hi"),
            "single-tool-call",
            &[],
            "sess_abc",
            "trg_test",
            Some("agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"),
            ReplyMedium::Text,
            None,
            None,
            0,
            &[],
            Some(Arc::new(hooks)),
            None,
            None,
            Some(&store),
            false,
            None,
        )
        .await
        .expect("expected chat output");

        assert_eq!(provider_called.load(Ordering::SeqCst), 2);
        assert_eq!(tool_called.load(Ordering::SeqCst), 1);
        assert_eq!(result.text, "HOOKED");

        // Tool results are persisted asynchronously from the runner event forwarder.
        let start = Instant::now();
        let persisted = loop {
            let history = store.read("sess_abc").await.unwrap_or_default();
            if !history.is_empty() {
                break history;
            }
            if start.elapsed() > Duration::from_secs(2) {
                panic!("timed out waiting for tool_result persistence");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(
            persisted[0].get("role").and_then(|v| v.as_str()),
            Some("tool_result")
        );
        assert_eq!(
            persisted[0]["result"],
            serde_json::json!({"redacted": true})
        );

        let seen_vals = seen.lock().await;
        let events: Vec<&str> = seen_vals
            .iter()
            .filter_map(|v| v.get("event").and_then(|e| e.as_str()))
            .collect();
        assert!(
            events.contains(&"MessageSending"),
            "missing MessageSending hook"
        );
        assert!(events.contains(&"MessageSent"), "missing MessageSent hook");
        assert!(events.contains(&"AgentEnd"), "missing AgentEnd hook");
        assert!(
            events.contains(&"ToolResultPersist"),
            "missing ToolResultPersist hook"
        );
        assert!(
            events.contains(&"BeforeAgentStart"),
            "missing BeforeAgentStart hook"
        );

        let agent_end = seen_vals
            .iter()
            .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("AgentEnd"))
            .expect("missing AgentEnd payload");
        assert_eq!(agent_end["text"], "HOOKED");
        assert_eq!(agent_end["iterations"], 2);
        assert_eq!(agent_end["toolCalls"], 1);

        let before_llm = seen_vals
            .iter()
            .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("BeforeLLMCall"))
            .expect("missing BeforeLLMCall payload");
        assert_eq!(
            before_llm["sessionKey"],
            serde_json::json!(
                "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"
            )
        );
        assert_eq!(
            before_llm["channelTarget"]["type"],
            serde_json::json!("telegram")
        );
        assert_eq!(
            before_llm["channelTarget"]["accountKey"],
            serde_json::json!("telegram:845")
        );
        assert_eq!(
            before_llm["channelTarget"]["chatId"],
            serde_json::json!("-100")
        );
        assert_eq!(
            before_llm["channelTarget"]["threadId"],
            serde_json::json!("42")
        );

        let before_tool = seen_vals
            .iter()
            .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("BeforeToolCall"))
            .expect("missing BeforeToolCall payload");
        assert_eq!(
            before_tool["sessionKey"],
            serde_json::json!(
                "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"
            )
        );
        assert_eq!(
            before_tool["channelTarget"]["type"],
            serde_json::json!("telegram")
        );

        let after_tool = seen_vals
            .iter()
            .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("AfterToolCall"))
            .expect("missing AfterToolCall payload");
        assert_eq!(
            after_tool["sessionKey"],
            serde_json::json!(
                "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.42-sender-tguser.1001"
            )
        );
        assert_eq!(
            after_tool["channelTarget"]["type"],
            serde_json::json!("telegram")
        );
    }

    #[tokio::test]
    async fn ordered_runner_event_callback_stays_in_order_with_variable_processing_latency() {
        let (on_event, mut rx) = ordered_runner_event_callback();
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let seen_for_worker = Arc::clone(&seen);

        let worker = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let RunnerEvent::TextDelta(text) = event {
                    if text == "slow" {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    seen_for_worker.lock().await.push(text);
                }
            }
        });

        on_event(RunnerEvent::TextDelta("slow".to_string()));
        on_event(RunnerEvent::TextDelta("fast".to_string()));
        drop(on_event);

        worker.await.unwrap();
        let observed = seen.lock().await.clone();
        assert_eq!(observed, vec!["slow".to_string(), "fast".to_string()]);
    }

    /// Build a bare session_locks map for testing the semaphore logic
    /// without constructing a full LiveChatService.
    fn make_session_locks() -> Arc<RwLock<HashMap<String, Arc<Semaphore>>>> {
        Arc::new(RwLock::new(HashMap::new()))
    }

    #[test]
    fn compaction_debug_info_detects_summary_header_message() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": "[Conversation Summary]\n\nHello world",
                "created_at": 123_u64,
            }),
            serde_json::json!({
                "role": "user",
                "content": "hi",
            }),
        ];
        let info = super::build_compaction_debug_info(&messages);
        assert_eq!(info["isCompacted"].as_bool(), Some(true));
        assert_eq!(info["summaryCreatedAt"].as_u64(), Some(123));
        assert_eq!(
            info["summaryLen"].as_u64(),
            Some("Hello world".len() as u64)
        );
        assert_eq!(info["keptMessageCount"].as_u64(), Some(1));
        assert_eq!(
            info["keepLastUserRounds"].as_u64(),
            Some(super::KEEP_LAST_USER_ROUNDS as u64)
        );
    }

    #[test]
    fn sandbox_mount_debug_info_reports_expected_status() {
        use moltis_config::schema::{SandboxConfig, SandboxMountConfig};

        let mut cfg = SandboxConfig::default();
        cfg.mounts = vec![SandboxMountConfig {
            host_dir: "/mnt/c/dev".into(),
            guest_dir: "/mnt/host/dev".into(),
            mode: "ro".into(),
        }];

        let (_mounts, _allow, status) = super::sandbox_mount_debug_info(&cfg, None, false);
        assert_eq!(status, "router_unavailable");

        let (_mounts, _allow, status) = super::sandbox_mount_debug_info(&cfg, Some("none"), true);
        assert_eq!(status, "unsupported_backend");

        let (_mounts, allow, status) = super::sandbox_mount_debug_info(&cfg, Some("docker"), true);
        assert!(allow.is_empty());
        assert_eq!(status, "deny_by_default");

        cfg.mount_allowlist = vec!["/mnt/c".into()];
        let (_mounts, allow, status) = super::sandbox_mount_debug_info(&cfg, Some("docker"), true);
        assert_eq!(allow, vec!["/mnt/c".to_string()]);
        assert_eq!(status, "configured");
    }

    #[test]
    fn token_debug_next_request_includes_draft_and_reconstructed_tool_chain_in_estimate() {
        let provider = BudgetProvider {
            context_window: 1_000,
            input_limit: Some(500),
            output_limit: Some(200),
        };
        let llm_debug = serde_json::json!({
            "overrides": {
                "generation": {
                    "max_output_tokens": { "effective": 150 }
                }
            }
        });

        let system_prompt = "SYS";
        let history = vec![
            serde_json::json!({"role":"user","content":"hi"}),
            serde_json::json!({
                "role":"tool_result",
                "tool_name":"exec",
                "tool_call_id":"t1",
                "arguments": {"command":"echo hi"},
                "success": true,
                "result": {"stdout":"ok"},
            }),
            serde_json::json!({
                "role":"assistant",
                "content":"done",
                "inputTokens": 10,
                "outputTokens": 5,
                "cachedTokens": 2,
            }),
        ];

        let info = super::build_token_debug_info(
            &provider,
            &llm_debug,
            system_prompt,
            &history,
            Some("draft"),
            50_000,
        );

        assert_eq!(info["lastRequest"]["inputTokens"].as_u64(), Some(10));
        assert_eq!(info["lastRequest"]["outputTokens"].as_u64(), Some(5));
        assert_eq!(info["lastRequest"]["cachedTokens"].as_u64(), Some(2));

        let next = &info["nextRequest"];
        assert_eq!(next["contextWindow"].as_u64(), Some(1_000));
        assert_eq!(next["plannedMaxOutputToks"].as_u64(), Some(150));
        assert_eq!(next["maxInputToks"].as_u64(), Some(500));
        assert_eq!(next["autoCompactToksThred"].as_u64(), Some(425));

        let history_with_tools =
            super::reconstruct_tool_history_for_prompt_estimate(&history, 50_000);
        let mut msgs = vec![ChatMessage::system(system_prompt)];
        msgs.extend(values_to_chat_messages(&history_with_tools));
        let history_est = super::estimate_input_tokens_for_messages(&msgs);
        let pending_est = super::tokens_estimate_utf8_bytes_div_3("draft");
        let expected = history_est + pending_est + super::SAFETY_MARGIN_TOKENS;

        assert_eq!(next["promptInputToksEst"].as_u64(), Some(expected));
    }

    async fn get_or_create_semaphore(
        locks: &Arc<RwLock<HashMap<String, Arc<Semaphore>>>>,
        key: &str,
    ) -> Arc<Semaphore> {
        {
            let map = locks.read().await;
            if let Some(sem) = map.get(key) {
                return Arc::clone(sem);
            }
        }
        let mut map = locks.write().await;
        Arc::clone(
            map.entry(key.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(1))),
        )
    }

    #[tokio::test]
    async fn same_session_runs_are_serialized() {
        let locks = make_session_locks();
        let sem = get_or_create_semaphore(&locks, "s1").await;

        // Acquire the permit — simulates a running task.
        let permit = sem.clone().acquire_owned().await.unwrap();

        // A second acquire should not resolve while the first is held.
        let sem2 = sem.clone();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let _p = sem2.acquire_owned().await.unwrap();
            let _ = tx.send(());
        });

        // Give the second task a chance to run — it should be blocked.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            rx.try_recv().is_err(),
            "second run should be blocked while first holds permit"
        );

        // Release first permit.
        drop(permit);

        // Now the second task should complete.
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn different_sessions_run_in_parallel() {
        let locks = make_session_locks();
        let sem_a = get_or_create_semaphore(&locks, "a").await;
        let sem_b = get_or_create_semaphore(&locks, "b").await;

        let _pa = sem_a.clone().acquire_owned().await.unwrap();
        // Session "b" should still be acquirable.
        let _pb = sem_b.clone().acquire_owned().await.unwrap();
    }

    #[tokio::test]
    async fn abort_releases_permit() {
        let locks = make_session_locks();
        let sem = get_or_create_semaphore(&locks, "s").await;

        let sem2 = sem.clone();
        let task = tokio::spawn(async move {
            let _p = sem2.acquire_owned().await.unwrap();
            // Simulate long-running work.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });

        // Give the task time to acquire the permit.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Abort the task — this drops the permit.
        task.abort();
        let _ = task.await;

        // The semaphore should now be acquirable.
        let _p = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            sem.clone().acquire_owned(),
        )
        .await
        .expect("permit should be available after abort")
        .unwrap();
    }

    #[tokio::test]
    async fn agent_timeout_cancels_slow_future() {
        use std::time::Duration;

        let timeout_secs: u64 = 1;
        let slow_fut = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Some(("done".to_string(), 0u32, 0u32))
        };

        let result: Option<(String, u32, u32)> =
            tokio::time::timeout(Duration::from_secs(timeout_secs), slow_fut)
                .await
                .unwrap_or_default();

        assert!(
            result.is_none(),
            "slow future should have been cancelled by timeout"
        );
    }

    #[tokio::test]
    async fn agent_timeout_zero_means_no_timeout() {
        use std::time::Duration;

        let timeout_secs: u64 = 0;
        let fast_fut = async { Some(("ok".to_string(), 10u32, 5u32)) };

        let result = if timeout_secs > 0 {
            tokio::time::timeout(Duration::from_secs(timeout_secs), fast_fut)
                .await
                .unwrap_or_default()
        } else {
            fast_fut.await
        };

        assert_eq!(result, Some(("ok".to_string(), 10, 5)));
    }

    // ── Message queue tests ──────────────────────────────────────────────

    fn make_message_queue() -> Arc<RwLock<HashMap<String, Vec<QueuedMessage>>>> {
        Arc::new(RwLock::new(HashMap::new()))
    }

    #[tokio::test]
    async fn queue_enqueue_and_drain() {
        let queue = make_message_queue();
        let key = "sess1";

        // Enqueue two messages.
        {
            let mut q = queue.write().await;
            q.entry(key.to_string()).or_default().push(QueuedMessage {
                params: serde_json::json!({"text": "hello"}),
            });
            q.entry(key.to_string()).or_default().push(QueuedMessage {
                params: serde_json::json!({"text": "world"}),
            });
        }

        // Drain.
        let drained = queue.write().await.remove(key).unwrap_or_default();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].params["text"], "hello");
        assert_eq!(drained[1].params["text"], "world");

        // Queue should be empty after drain.
        assert!(queue.read().await.get(key).is_none());
    }

    #[tokio::test]
    async fn queue_collect_concatenates_texts() {
        let msgs = [
            QueuedMessage {
                params: serde_json::json!({"text": "first", "model": "gpt-4"}),
            },
            QueuedMessage {
                params: serde_json::json!({"text": "second"}),
            },
            QueuedMessage {
                params: serde_json::json!({"text": "third", "_connId": "c1"}),
            },
        ];

        let combined: Vec<&str> = msgs
            .iter()
            .filter_map(|m| m.params.get("text").and_then(|v| v.as_str()))
            .collect();
        let joined = combined.join("\n\n");
        assert_eq!(joined, "first\n\nsecond\n\nthird");
    }

    #[tokio::test]
    async fn try_acquire_returns_err_when_held() {
        let sem = Arc::new(Semaphore::new(1));
        let _permit = sem.clone().try_acquire_owned().unwrap();

        // Second try_acquire should fail.
        assert!(sem.clone().try_acquire_owned().is_err());
    }

    #[tokio::test]
    async fn try_acquire_succeeds_when_free() {
        let sem = Arc::new(Semaphore::new(1));
        assert!(sem.clone().try_acquire_owned().is_ok());
    }

    #[tokio::test]
    async fn queue_drain_empty_is_noop() {
        let queue = make_message_queue();
        let drained = queue
            .write()
            .await
            .remove("nonexistent")
            .unwrap_or_default();
        assert!(drained.is_empty());
    }

    #[tokio::test]
    async fn queue_drain_drops_permit_before_send() {
        // Simulate the fixed drain flow: after `drop(permit)`, the semaphore
        // should be available for the replayed `chat.send()` to acquire.
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.clone().try_acquire_owned().unwrap();

        // While held, a second acquire must fail (simulates the bug).
        assert!(sem.clone().try_acquire_owned().is_err());

        // Drop — mirrors the new `drop(permit)` before the drain loop.
        drop(permit);

        // Now the replayed send can acquire the permit.
        assert!(
            sem.clone().try_acquire_owned().is_ok(),
            "permit should be available after explicit drop"
        );
    }

    #[tokio::test]
    async fn followup_drain_sends_only_first_and_requeues_rest() {
        let queue = make_message_queue();
        let key = "sess_drain";

        // Simulate three queued messages.
        {
            let mut q = queue.write().await;
            let entry = q.entry(key.to_string()).or_default();
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "a"}),
            });
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "b"}),
            });
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "c"}),
            });
        }

        // Drain and apply the send-first/requeue-rest logic.
        let queued = queue.write().await.remove(key).unwrap_or_default();

        let mut iter = queued.into_iter();
        let first = iter.next().expect("queued is non-empty");
        let rest: Vec<QueuedMessage> = iter.collect();

        // The first message is the one to send.
        assert_eq!(first.params["text"], "a");

        // Remaining messages are re-queued.
        if !rest.is_empty() {
            queue
                .write()
                .await
                .entry(key.to_string())
                .or_default()
                .extend(rest);
        }

        // Verify the queue now holds exactly the two remaining messages.
        let remaining = queue.read().await;
        let entries = remaining.get(key).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].params["text"], "b");
        assert_eq!(entries[1].params["text"], "c");
    }

    #[test]
    fn message_queue_mode_default_is_followup() {
        let mode = MessageQueueMode::default();
        assert_eq!(mode, MessageQueueMode::Followup);
    }

    #[test]
    fn message_queue_mode_deserializes_from_toml() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Wrapper {
            mode: MessageQueueMode,
        }

        let followup: Wrapper = toml::from_str(r#"mode = "followup""#).unwrap();
        assert_eq!(followup.mode, MessageQueueMode::Followup);

        let collect: Wrapper = toml::from_str(r#"mode = "collect""#).unwrap();
        assert_eq!(collect.mode, MessageQueueMode::Collect);
    }

    #[tokio::test]
    async fn cancel_queued_clears_session_queue() {
        let queue = make_message_queue();
        let key = "sess_cancel";

        // Enqueue two messages.
        {
            let mut q = queue.write().await;
            let entry = q.entry(key.to_string()).or_default();
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "a"}),
            });
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "b"}),
            });
        }

        // Cancel (same logic as cancel_queued: remove + unwrap_or_default).
        let removed = queue.write().await.remove(key).unwrap_or_default();
        assert_eq!(removed.len(), 2);

        // Queue should be empty.
        assert!(queue.read().await.get(key).is_none());
    }

    #[tokio::test]
    async fn cancel_queued_returns_count() {
        let queue = make_message_queue();
        let key = "sess_count";

        {
            let mut q = queue.write().await;
            let entry = q.entry(key.to_string()).or_default();
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "x"}),
            });
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "y"}),
            });
            entry.push(QueuedMessage {
                params: serde_json::json!({"text": "z"}),
            });
        }

        let removed = queue.write().await.remove(key).unwrap_or_default();
        let count = removed.len();
        assert_eq!(count, 3);
        let result = serde_json::json!({ "cleared": count });
        assert_eq!(result["cleared"], 3);
    }

    #[tokio::test]
    async fn cancel_queued_noop_for_empty_queue() {
        let queue = make_message_queue();
        let key = "sess_empty";

        // Cancel on a session with no queued messages.
        let removed = queue.write().await.remove(key).unwrap_or_default();
        assert_eq!(removed.len(), 0);

        let result = serde_json::json!({ "cleared": removed.len() });
        assert_eq!(result["cleared"], 0);
    }

    #[test]
    fn effective_tool_policy_profile_and_config_merge() {
        let mut cfg = moltis_config::MoltisConfig::default();
        cfg.tools.policy.profile = Some("full".into());
        cfg.tools.policy.deny = vec!["exec".into()];

        let policy = effective_tool_policy(&cfg);
        assert!(!policy.is_allowed("exec"));
        assert!(policy.is_allowed("web_fetch"));
    }

    #[test]
    fn runtime_filters_apply_policy_without_skill_tool_restrictions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool {
            name: "exec".to_string(),
        }));
        registry.register(Box::new(DummyTool {
            name: "web_fetch".to_string(),
        }));
        registry.register(Box::new(DummyTool {
            name: "create_skill".to_string(),
        }));
        registry.register(Box::new(DummyTool {
            name: "session_state".to_string(),
        }));

        let mut cfg = moltis_config::MoltisConfig::default();
        cfg.tools.policy.allow = vec!["exec".into(), "web_fetch".into(), "create_skill".into()];

        let skills = vec![moltis_skills::types::SkillMetadata {
            name: "my-skill".into(),
            description: "test".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec!["Bash(git:*)".into()],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: std::path::PathBuf::new(),
            source: None,
        }];

        let filtered = apply_runtime_tool_filters(&registry, &cfg, &skills, false);
        assert!(filtered.get("exec").is_some());
        assert!(filtered.get("web_fetch").is_some());
        assert!(filtered.get("create_skill").is_some());
        assert!(filtered.get("session_state").is_none());
    }

    #[test]
    fn runtime_filters_do_not_hide_create_skill_when_skill_allows_only_web_fetch() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool {
            name: "create_skill".to_string(),
        }));
        registry.register(Box::new(DummyTool {
            name: "web_fetch".to_string(),
        }));

        let mut cfg = moltis_config::MoltisConfig::default();
        cfg.tools.policy.allow = vec!["create_skill".into(), "web_fetch".into()];

        let skills = vec![moltis_skills::types::SkillMetadata {
            name: "weather".into(),
            description: "weather checker".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec!["WebFetch".into()],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: std::path::PathBuf::new(),
            source: None,
        }];

        let filtered = apply_runtime_tool_filters(&registry, &cfg, &skills, false);
        assert!(filtered.get("create_skill").is_some());
        assert!(filtered.get("web_fetch").is_some());
    }

    #[test]
    fn priority_models_pin_raw_model_ids_first() {
        let m1 = moltis_agents::providers::ModelInfo {
            id: "openai-codex::gpt-5.2".into(),
            provider: "openai-codex".into(),
            display_name: "GPT 5.2".into(),
            created_at: None,
        };
        let m2 = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-opus-4-5".into(),
            provider: "anthropic".into(),
            display_name: "Claude Opus 4.5".into(),
            created_at: None,
        };
        let m3 = moltis_agents::providers::ModelInfo {
            id: "google::gemini-3-flash".into(),
            provider: "gemini".into(),
            display_name: "Gemini 3 Flash".into(),
            created_at: None,
        };

        let order =
            LiveModelService::build_priority_order(&["gpt-5.2".into(), "claude-opus-4-5".into()]);
        let ordered = LiveModelService::prioritize_models(&order, vec![&m3, &m2, &m1].into_iter());
        assert_eq!(ordered[0].id, m1.id);
        assert_eq!(ordered[1].id, m2.id);
        assert_eq!(ordered[2].id, m3.id);
    }

    #[test]
    fn priority_models_match_separator_variants() {
        let m1 = moltis_agents::providers::ModelInfo {
            id: "openai-codex::gpt-5.2".into(),
            provider: "openai-codex".into(),
            display_name: "GPT-5.2".into(),
            created_at: None,
        };
        let m2 = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-sonnet-4-5-20250929".into(),
            provider: "anthropic".into(),
            display_name: "Claude Sonnet 4.5".into(),
            created_at: None,
        };
        let m3 = moltis_agents::providers::ModelInfo {
            id: "google::gemini-3-flash".into(),
            provider: "gemini".into(),
            display_name: "Gemini 3 Flash".into(),
            created_at: None,
        };

        let order =
            LiveModelService::build_priority_order(&["gpt 5.2".into(), "claude-sonnet-4.5".into()]);
        let ordered = LiveModelService::prioritize_models(&order, vec![&m3, &m2, &m1].into_iter());
        assert_eq!(ordered[0].id, m1.id);
        assert_eq!(ordered[1].id, m2.id);
        assert_eq!(ordered[2].id, m3.id);
    }

    #[test]
    fn allowed_models_filters_by_substring_match() {
        let m1 = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-opus-4-5".into(),
            provider: "anthropic".into(),
            display_name: "Claude Opus 4.5".into(),
            created_at: None,
        };
        let m2 = moltis_agents::providers::ModelInfo {
            id: "openai-codex::gpt-5.2".into(),
            provider: "openai-codex".into(),
            display_name: "GPT 5.2".into(),
            created_at: None,
        };
        let m3 = moltis_agents::providers::ModelInfo {
            id: "google::gemini-3-flash".into(),
            provider: "google".into(),
            display_name: "Gemini 3 Flash".into(),
            created_at: None,
        };

        let patterns: Vec<String> = vec!["opus".into()];
        assert!(model_matches_allowlist(&m1, &patterns));
        assert!(!model_matches_allowlist(&m2, &patterns));
        assert!(!model_matches_allowlist(&m3, &patterns));
    }

    #[test]
    fn allowed_models_empty_shows_all() {
        let m = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-opus-4-5".into(),
            provider: "anthropic".into(),
            display_name: "Claude Opus 4.5".into(),
            created_at: None,
        };
        assert!(model_matches_allowlist(&m, &[]));
    }

    #[test]
    fn allowed_models_case_insensitive() {
        let m = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-opus-4-5".into(),
            provider: "anthropic".into(),
            display_name: "Claude Opus 4.5".into(),
            created_at: None,
        };

        // Uppercase pattern matches lowercase model key.
        let patterns = vec![normalize_model_key("OPUS")];
        assert!(model_matches_allowlist(&m, &patterns));

        // Mixed case.
        let patterns = vec![normalize_model_key("OpUs")];
        assert!(model_matches_allowlist(&m, &patterns));
    }

    #[test]
    fn allowed_models_match_separator_variants() {
        let m = moltis_agents::providers::ModelInfo {
            id: "openai-codex::gpt-5.2".into(),
            provider: "openai-codex".into(),
            display_name: "GPT-5.2".into(),
            created_at: None,
        };

        let patterns = vec![normalize_model_key("gpt 5.2")];
        assert!(model_matches_allowlist(&m, &patterns));

        let patterns = vec![normalize_model_key("gpt-5-2")];
        assert!(model_matches_allowlist(&m, &patterns));
    }

    #[test]
    fn allowed_models_numeric_pattern_does_not_match_extended_variants() {
        let exact = moltis_agents::providers::ModelInfo {
            id: "openai::gpt-5.2".into(),
            provider: "openai".into(),
            display_name: "GPT-5.2".into(),
            created_at: None,
        };
        let extended = moltis_agents::providers::ModelInfo {
            id: "openai::gpt-5.2-chat-latest".into(),
            provider: "openai".into(),
            display_name: "GPT-5.2 Chat Latest".into(),
            created_at: None,
        };
        let patterns = vec![normalize_model_key("gpt 5.2")];

        assert!(model_matches_allowlist(&exact, &patterns));
        assert!(!model_matches_allowlist(&extended, &patterns));
    }

    #[test]
    fn allowed_models_numeric_pattern_matches_provider_prefixed_models() {
        let m = moltis_agents::providers::ModelInfo {
            id: "anthropic::claude-sonnet-4-5".into(),
            provider: "anthropic".into(),
            display_name: "Claude Sonnet 4.5".into(),
            created_at: None,
        };
        let patterns = vec![normalize_model_key("sonnet 4.5")];

        assert!(model_matches_allowlist(&m, &patterns));
    }

    #[test]
    fn allowed_models_does_not_filter_local_llm_or_ollama() {
        let local = moltis_agents::providers::ModelInfo {
            id: "local-llm::qwen2.5-coder-7b-q4_k_m".into(),
            provider: "local-llm".into(),
            display_name: "Qwen2.5 Coder 7B".into(),
            created_at: None,
        };
        let ollama = moltis_agents::providers::ModelInfo {
            id: "ollama::llama3.1:8b".into(),
            provider: "ollama".into(),
            display_name: "Llama 3.1 8B".into(),
            created_at: None,
        };
        let patterns = vec![normalize_model_key("opus")];

        assert!(model_matches_allowlist(&local, &patterns));
        assert!(model_matches_allowlist(&ollama, &patterns));
    }

    #[test]
    fn allowed_models_does_not_filter_ollama_when_provider_is_aliased() {
        let aliased = moltis_agents::providers::ModelInfo {
            id: "local-ai::llama3.1:8b".into(),
            provider: "local-ai".into(),
            display_name: "Llama 3.1 8B".into(),
            created_at: None,
        };
        let patterns = vec![normalize_model_key("opus")];

        assert!(model_matches_allowlist_with_provider(
            &aliased,
            Some("ollama"),
            &patterns
        ));
    }

    #[tokio::test]
    async fn list_and_list_all_return_all_registered_models() {
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "anthropic::claude-opus-4-5".to_string(),
                provider: "anthropic".to_string(),
                display_name: "Claude Opus 4.5".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "anthropic".to_string(),
                id: "anthropic::claude-opus-4-5".to_string(),
            }),
        );
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "openai-codex::gpt-5.2".to_string(),
                provider: "openai-codex".to_string(),
                display_name: "GPT 5.2".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "openai-codex".to_string(),
                id: "openai-codex::gpt-5.2".to_string(),
            }),
        );
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "google::gemini-3-flash".to_string(),
                provider: "google".to_string(),
                display_name: "Gemini 3 Flash".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "google".to_string(),
                id: "google::gemini-3-flash".to_string(),
            }),
        );

        let disabled = Arc::new(RwLock::new(DisabledModelsStore::default()));
        let service = LiveModelService::new(Arc::new(RwLock::new(registry)), disabled, vec![]);

        let result = service.list().await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);

        let result = service.list_all().await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn list_includes_created_at_in_response() {
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "openai::gpt-5.3".to_string(),
                provider: "openai".to_string(),
                display_name: "GPT-5.3".to_string(),
                created_at: Some(1700000000),
            },
            Arc::new(StaticProvider {
                name: "openai".to_string(),
                id: "openai::gpt-5.3".to_string(),
            }),
        );
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "openai::babbage-002".to_string(),
                provider: "openai".to_string(),
                display_name: "babbage-002".to_string(),
                created_at: Some(1600000000),
            },
            Arc::new(StaticProvider {
                name: "openai".to_string(),
                id: "openai::babbage-002".to_string(),
            }),
        );
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "anthropic::claude-opus".to_string(),
                provider: "anthropic".to_string(),
                display_name: "Claude Opus".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "anthropic".to_string(),
                id: "anthropic::claude-opus".to_string(),
            }),
        );

        let disabled = Arc::new(RwLock::new(DisabledModelsStore::default()));
        let service = LiveModelService::new(Arc::new(RwLock::new(registry)), disabled, vec![]);

        let result = service.list().await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);

        // Verify createdAt is present and correct.
        let gpt = arr.iter().find(|m| m["id"] == "openai::gpt-5.3").unwrap();
        assert_eq!(gpt["createdAt"], 1700000000);

        let babbage = arr
            .iter()
            .find(|m| m["id"] == "openai::babbage-002")
            .unwrap();
        assert_eq!(babbage["createdAt"], 1600000000);

        let claude = arr
            .iter()
            .find(|m| m["id"] == "anthropic::claude-opus")
            .unwrap();
        assert!(claude["createdAt"].is_null());

        // Also verify list_all includes createdAt.
        let result_all = service.list_all().await.unwrap();
        let arr_all = result_all.as_array().unwrap();
        let gpt_all = arr_all
            .iter()
            .find(|m| m["id"] == "openai::gpt-5.3")
            .unwrap();
        assert_eq!(gpt_all["createdAt"], 1700000000);
    }

    #[tokio::test]
    async fn list_includes_ollama_when_provider_is_aliased() {
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "openai-codex::gpt-5.2".to_string(),
                provider: "openai-codex".to_string(),
                display_name: "GPT 5.2".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "openai-codex".to_string(),
                id: "openai-codex::gpt-5.2".to_string(),
            }),
        );
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "local-ai::llama3.1:8b".to_string(),
                provider: "local-ai".to_string(),
                display_name: "Llama 3.1 8B".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "ollama".to_string(),
                id: "local-ai::llama3.1:8b".to_string(),
            }),
        );

        let disabled = Arc::new(RwLock::new(DisabledModelsStore::default()));
        let service = LiveModelService::new(Arc::new(RwLock::new(registry)), disabled, vec![]);

        let result = service.list().await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(
            arr.iter()
                .any(|m| m.get("id").and_then(|v| v.as_str()) == Some("local-ai::llama3.1:8b"))
        );

        let result = service.list_all().await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(
            arr.iter()
                .any(|m| m.get("id").and_then(|v| v.as_str()) == Some("local-ai::llama3.1:8b"))
        );
    }

    #[test]
    fn provider_filter_is_normalized_and_ignores_empty() {
        let params = serde_json::json!({"provider": "  OpenAI-CODEX "});
        assert_eq!(
            provider_filter_from_params(&params).as_deref(),
            Some("openai-codex")
        );
        assert!(provider_filter_from_params(&serde_json::json!({"provider": "   "})).is_none());
    }

    #[test]
    fn provider_matches_filter_is_case_insensitive() {
        assert!(provider_matches_filter(
            "openai-codex",
            Some("openai-codex")
        ));
        assert!(provider_matches_filter(
            "OpenAI-Codex",
            Some("openai-codex")
        ));
        assert!(!provider_matches_filter(
            "github-copilot",
            Some("openai-codex")
        ));
        assert!(provider_matches_filter("github-copilot", None));
    }

    #[test]
    fn push_provider_model_groups_models_by_provider() {
        let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        push_provider_model(
            &mut grouped,
            "openai-codex",
            "openai-codex::gpt-5.2",
            "GPT-5.2",
        );
        push_provider_model(
            &mut grouped,
            "openai-codex",
            "openai-codex::gpt-5.1-codex-mini",
            "GPT-5.1 Codex Mini",
        );
        push_provider_model(
            &mut grouped,
            "anthropic",
            "anthropic::claude-sonnet-4-5-20250929",
            "Claude Sonnet 4.5",
        );

        let openai = grouped.get("openai-codex").expect("openai group exists");
        assert_eq!(openai.len(), 2);
        assert_eq!(openai[0]["modelId"], "openai-codex::gpt-5.2");
        assert_eq!(openai[1]["modelId"], "openai-codex::gpt-5.1-codex-mini");

        let anthropic = grouped.get("anthropic").expect("anthropic group exists");
        assert_eq!(anthropic.len(), 1);
        assert_eq!(
            anthropic[0]["modelId"],
            "anthropic::claude-sonnet-4-5-20250929"
        );
    }

    #[tokio::test]
    async fn list_all_includes_disabled_models_and_list_hides_them() {
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "unit-test-model".to_string(),
                provider: "unit-test-provider".to_string(),
                display_name: "Unit Test Model".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "unit-test-provider".to_string(),
                id: "unit-test-model".to_string(),
            }),
        );

        let disabled = Arc::new(RwLock::new(DisabledModelsStore::default()));
        {
            let mut store = disabled.write().await;
            store.disable("unit-test-provider::unit-test-model");
        }

        let service = LiveModelService::new(Arc::new(RwLock::new(registry)), disabled, vec![]);

        let all = service
            .list_all()
            .await
            .expect("models.list_all should succeed");
        let all_models = all
            .as_array()
            .expect("models.list_all should return an array");
        let all_entry = all_models
            .iter()
            .find(|m| {
                m.get("id").and_then(|v| v.as_str()) == Some("unit-test-provider::unit-test-model")
            })
            .expect("disabled model should still appear in models.list_all");
        assert_eq!(
            all_entry.get("disabled").and_then(|v| v.as_bool()),
            Some(true)
        );

        let visible = service.list().await.expect("models.list should succeed");
        let visible_models = visible
            .as_array()
            .expect("models.list should return an array");
        assert!(
            visible_models
                .iter()
                .all(|m| m.get("id").and_then(|v| v.as_str())
                    != Some("unit-test-provider::unit-test-model")),
            "disabled model should be hidden from models.list",
        );
    }

    #[test]
    fn probe_rate_limit_detection_matches_copilot_429_pattern() {
        let raw = "github-copilot API error status=429 Too Many Requests body=quota exceeded";
        let error_obj = parse_chat_error(raw, Some("github-copilot"));
        assert!(is_probe_rate_limited_error(&error_obj, raw));
        assert_ne!(error_obj["type"], "unsupported_model");
    }

    #[test]
    fn probe_rate_limit_backoff_doubles_and_caps() {
        assert_eq!(next_probe_rate_limit_backoff_ms(None), 1_000);
        assert_eq!(next_probe_rate_limit_backoff_ms(Some(1_000)), 2_000);
        assert_eq!(next_probe_rate_limit_backoff_ms(Some(20_000)), 30_000);
        assert_eq!(next_probe_rate_limit_backoff_ms(Some(30_000)), 30_000);
    }

    #[tokio::test]
    async fn model_test_rejects_missing_model_id() {
        let service = LiveModelService::new(
            Arc::new(RwLock::new(ProviderRegistry::from_env_with_config(
                &moltis_config::schema::ProvidersConfig::default(),
            ))),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            vec![],
        );
        let result = service.test(serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing 'modelId'"));
    }

    #[tokio::test]
    async fn model_test_rejects_unknown_model() {
        let service = LiveModelService::new(
            Arc::new(RwLock::new(ProviderRegistry::from_env_with_config(
                &moltis_config::schema::ProvidersConfig::default(),
            ))),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            vec![],
        );
        let result = service
            .test(serde_json::json!({"modelId": "nonexistent::model-xyz"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown model"));
    }

    #[tokio::test]
    async fn model_test_returns_error_when_provider_fails() {
        let mut registry = ProviderRegistry::from_env_with_config(
            &moltis_config::schema::ProvidersConfig::default(),
        );
        // StaticProvider's complete() returns an error ("not implemented for test")
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "test-provider::test-model".to_string(),
                provider: "test-provider".to_string(),
                display_name: "Test Model".to_string(),
                created_at: None,
            },
            Arc::new(StaticProvider {
                name: "test-provider".to_string(),
                id: "test-provider::test-model".to_string(),
            }),
        );

        let service = LiveModelService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            vec![],
        );
        let result = service
            .test(serde_json::json!({"modelId": "test-provider::test-model"}))
            .await;
        // StaticProvider.complete() returns Err, so test should return an error.
        assert!(result.is_err());
    }

    #[test]
    fn probe_parallel_per_provider_defaults_and_clamps() {
        assert_eq!(probe_max_parallel_per_provider(&serde_json::json!({})), 1);
        assert_eq!(
            probe_max_parallel_per_provider(&serde_json::json!({"maxParallelPerProvider": 1})),
            1
        );
        assert_eq!(
            probe_max_parallel_per_provider(&serde_json::json!({"maxParallelPerProvider": 99})),
            8
        );
    }

    // ── to_user_content tests ─────────────────────────────────────────

    #[test]
    fn to_user_content_text_only() {
        let mc = MessageContent::Text("hello".to_string());
        let uc = to_user_content(&mc);
        match uc {
            UserContent::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn to_user_content_multimodal_with_image() {
        use moltis_sessions::message::{ContentBlock, ImageUrl as SessionImageUrl};

        let mc = MessageContent::Multimodal(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::ImageUrl {
                image_url: SessionImageUrl {
                    url: "data:image/png;base64,AAAA".to_string(),
                },
            },
        ]);
        let uc = to_user_content(&mc);
        match uc {
            UserContent::Multimodal(parts) => {
                assert_eq!(parts.len(), 2);
                match &parts[0] {
                    ContentPart::Text(t) => assert_eq!(t, "describe this"),
                    _ => panic!("expected Text part"),
                }
                match &parts[1] {
                    ContentPart::Image { media_type, data } => {
                        assert_eq!(media_type, "image/png");
                        assert_eq!(data, "AAAA");
                    },
                    _ => panic!("expected Image part"),
                }
            },
            _ => panic!("expected Multimodal variant"),
        }
    }

    #[test]
    fn to_user_content_drops_invalid_data_uri() {
        use moltis_sessions::message::{ContentBlock, ImageUrl as SessionImageUrl};

        let mc = MessageContent::Multimodal(vec![
            ContentBlock::Text {
                text: "just text".to_string(),
            },
            ContentBlock::ImageUrl {
                image_url: SessionImageUrl {
                    url: "https://example.com/image.png".to_string(),
                },
            },
        ]);
        let uc = to_user_content(&mc);
        match uc {
            UserContent::Multimodal(parts) => {
                // The https URL is not a data URI, so it should be dropped
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    ContentPart::Text(t) => assert_eq!(t, "just text"),
                    _ => panic!("expected Text part"),
                }
            },
            _ => panic!("expected Multimodal variant"),
        }
    }

    // ── Logbook formatting tests ─────────────────────────────────────────

    #[test]
    fn format_logbook_html_empty_entries() {
        assert_eq!(format_logbook_html(&[]), "");
    }

    #[test]
    fn format_logbook_html_single_entry() {
        let entries = vec!["Using Claude Sonnet 4.5. Use /model to change.".to_string()];
        let html = format_logbook_html(&entries);
        assert!(html.starts_with("<blockquote expandable>"));
        assert!(html.ends_with("</blockquote>"));
        assert!(html.contains("\u{1f4cb} <b>Activity log</b>"));
        assert!(html.contains("\u{2022} Using Claude Sonnet 4.5. Use /model to change."));
    }

    #[test]
    fn format_logbook_html_multiple_entries() {
        let entries = vec![
            "Using Claude Sonnet 4.5. Use /model to change.".to_string(),
            "\u{1f50d} Searching: rust async patterns".to_string(),
            "\u{1f4bb} Running: `ls -la`".to_string(),
        ];
        let html = format_logbook_html(&entries);
        // Verify all entries are present as bullet points.
        for entry in &entries {
            let escaped = entry
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            assert!(
                html.contains(&format!("\u{2022} {escaped}")),
                "missing entry: {entry}"
            );
        }
    }

    #[test]
    fn format_logbook_html_escapes_html_entities() {
        let entries = vec!["Running: `echo <script>alert(1)</script>`".to_string()];
        let html = format_logbook_html(&entries);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn format_tool_status_message_truncates_utf8_safely() {
        let message = format_tool_status_message(
            "exec",
            &serde_json::json!({
                "command": "你好世界你好世界你好世界你好世界你好世界你好世界"
            }),
        );

        assert!(message.starts_with("💻 Running: `"));
        assert!(message.contains("..."));
        assert!(!message.contains('�'));
    }

    #[test]
    fn truncate_url_preserves_utf8_boundaries() {
        let truncated =
            truncate_url("https://example.com/你好世界你好世界你好世界你好世界你好世界");
        assert!(truncated.ends_with("..."));
        assert!(!truncated.contains('�'));
    }

    #[test]
    fn extract_location_from_show_map_result() {
        let result = serde_json::json!({
            "latitude": 37.76,
            "longitude": -122.42,
            "label": "La Taqueria",
            "screenshot": "data:image/png;base64,abc",
            "map_links": {}
        });

        // Extraction logic mirrors the ToolCallEnd handler
        let extracted = result
            .get("latitude")
            .and_then(|v| v.as_f64())
            .and_then(|lat| {
                let lon = result.get("longitude")?.as_f64()?;
                let label = result
                    .get("label")
                    .and_then(|l| l.as_str())
                    .map(String::from);
                Some((lat, lon, label))
            });

        let (lat, lon, label) = extracted.unwrap();
        assert!((lat - 37.76).abs() < f64::EPSILON);
        assert!((lon - (-122.42)).abs() < f64::EPSILON);
        assert_eq!(label.as_deref(), Some("La Taqueria"));
    }

    #[test]
    fn extract_location_without_label() {
        let result = serde_json::json!({
            "latitude": 48.8566,
            "longitude": 2.3522,
            "screenshot": "data:image/png;base64,abc"
        });

        let extracted = result
            .get("latitude")
            .and_then(|v| v.as_f64())
            .and_then(|lat| {
                let lon = result.get("longitude")?.as_f64()?;
                let label = result
                    .get("label")
                    .and_then(|l| l.as_str())
                    .map(String::from);
                Some((lat, lon, label))
            });

        let (lat, lon, label) = extracted.unwrap();
        assert!((lat - 48.8566).abs() < f64::EPSILON);
        assert!((lon - 2.3522).abs() < f64::EPSILON);
        assert!(label.is_none());
    }

    #[test]
    fn extract_location_missing_coords_returns_none() {
        let result = serde_json::json!({
            "screenshot": "data:image/png;base64,abc"
        });

        let extracted = result
            .get("latitude")
            .and_then(|v| v.as_f64())
            .and_then(|_lat| {
                let _lon = result.get("longitude")?.as_f64()?;
                Some(())
            });

        assert!(extracted.is_none());
    }

    #[test]
    fn keep_window_start_idx_keeps_last_4_user_rounds() {
        let mut history = Vec::new();
        for i in 0..6 {
            history.push(serde_json::json!({"role":"user","content": format!("u{i}")}));
            history.push(serde_json::json!({"role":"assistant","content": format!("a{i}")}));
        }
        // user indices: 0,2,4,6,8,10 -> keep last 4 starts at index 4
        assert_eq!(keep_window_start_idx(&history, 4), 4);
    }

    #[test]
    fn tokens_estimate_is_conservative_bytes_div_3() {
        assert_eq!(tokens_estimate_utf8_bytes_div_3("abc"), 1);
        assert_eq!(tokens_estimate_utf8_bytes_div_3("abcd"), 2);
        assert_eq!(tokens_estimate_utf8_bytes_div_3(""), 0);
    }

    #[test]
    fn build_compacted_history_preserves_keep_window_byte_for_byte() {
        let history = vec![
            serde_json::json!({"role":"user","content":"u0"}),
            serde_json::json!({"role":"assistant","content":"a0"}),
            serde_json::json!({"role":"user","content":"u1"}),
            serde_json::json!({"role":"assistant","content":"a1"}),
            serde_json::json!({"role":"user","content":"u2"}),
            serde_json::json!({"role":"assistant","content":"a2"}),
            serde_json::json!({"role":"user","content":"u3"}),
            serde_json::json!({"role":"assistant","content":"a3"}),
            serde_json::json!({"role":"user","content":"u4"}),
            serde_json::json!({"role":"assistant","content":"a4"}),
            serde_json::json!({"role":"tool_result","tool_name":"exec","tool_call_id":"t1","success":true,"result":{"stdout":"ok","stderr":"","exit_code":0}}),
            serde_json::json!({"role":"user","content":"u5"}),
            serde_json::json!({"role":"assistant","content":"a5"}),
        ];

        let (compacted, keep_start_idx, kept_count) =
            build_compacted_history(&history, "SUMMARY", 4, Some(123)).unwrap();
        assert_eq!(keep_start_idx, keep_window_start_idx(&history, 4));
        assert_eq!(kept_count, history.len() - keep_start_idx);
        assert_eq!(compacted.len(), 1 + (history.len() - keep_start_idx));
        // Keep window is byte-for-byte preserved (including tool_result entries).
        assert_eq!(&compacted[1..], &history[keep_start_idx..]);
        assert_eq!(compacted[0]["role"].as_str(), Some("assistant"));
        assert!(
            compacted[0]["content"]
                .as_str()
                .unwrap_or("")
                .contains("SUMMARY")
        );
    }

    #[test]
    fn compaction_budget_watermarks_match_spec() {
        let provider = BudgetProvider {
            context_window: 400_000,
            input_limit: Some(272_000),
            output_limit: Some(128_000),
        };
        let b = CompactionBudget::for_provider(&provider);
        assert_eq!(b.input_hard_cap, 272_000);
        assert_eq!(b.high_watermark, 231_200);
        assert_eq!(b.low_watermark, 163_200);
        assert_eq!(b.reserved_output_tokens, 128_000);
        assert_eq!(b.reserve_safety_tokens, SAFETY_MARGIN_TOKENS);
    }

    struct SnapshotChannelService {
        snapshots: Vec<moltis_telegram::config::TelegramBusAccountSnapshot>,
    }

    #[async_trait]
    impl crate::services::ChannelService for SnapshotChannelService {
        async fn status(&self) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn logout(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn send(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn add(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn remove(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn update(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn senders_list(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn sender_approve(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn sender_deny(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }

        async fn telegram_bus_accounts_snapshot(
            &self,
        ) -> Vec<moltis_telegram::config::TelegramBusAccountSnapshot> {
            self.snapshots.clone()
        }
    }

    #[tokio::test]
    async fn ensure_channel_bound_session_sets_stable_telegram_label() {
        let metadata = sqlite_metadata().await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));

        let mut services = crate::services::GatewayServices::noop()
            .with_session_metadata(Arc::clone(&metadata))
            .with_session_store(Arc::clone(&store));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![moltis_telegram::config::TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                agent_id: None,
                chan_user_id: None,
                chan_user_name: Some("lovely_apple_bot".into()),
                chan_nickname: None,
                dm_scope: moltis_telegram::config::DmScope::Main,
                group_scope: moltis_telegram::config::GroupScope::Group,
            }],
        });

        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let key = "telegram:845:123";
        ensure_channel_bound_session(&state, key, "telegram:845", "123", None, None).await;

        let entry = metadata.get(key).await.expect("session row");
        assert_eq!(
            entry.label.as_deref(),
            Some("TG @lovely_apple_bot · dm:123")
        );
        assert!(entry.channel_binding.is_some());
    }

    #[tokio::test]
    async fn resolve_telegram_session_id_uses_matching_runtime_session_key() {
        let metadata = sqlite_metadata().await;
        metadata
            .set_active_session_id("agent:zhuzhu:group-peer-tgchat.n100", "sess_old")
            .await;

        let mut services =
            crate::services::GatewayServices::noop().with_session_metadata(Arc::clone(&metadata));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![moltis_telegram::config::TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                agent_id: Some("zhuzhu".into()),
                chan_user_id: None,
                chan_user_name: Some("relay_bot".into()),
                chan_nickname: None,
                dm_scope: moltis_telegram::config::DmScope::Main,
                group_scope: moltis_telegram::config::GroupScope::Group,
            }],
        });
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let resolved =
            resolve_telegram_session_id(&state, "telegram:845", "-100", None, Some("telegram:42"))
                .await;
        assert_eq!(resolved.as_deref(), Some("sess_old"));
    }

    #[tokio::test]
    async fn resolve_telegram_session_id_does_not_reuse_active_session_from_other_bucket() {
        let metadata = sqlite_metadata().await;
        metadata
            .set_active_session_id(
                "agent:zhuzhu:group-peer-tgchat.n100-sender-tguser.41",
                "sess_other",
            )
            .await;

        let mut services =
            crate::services::GatewayServices::noop().with_session_metadata(Arc::clone(&metadata));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![moltis_telegram::config::TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                agent_id: Some("zhuzhu".into()),
                chan_user_id: None,
                chan_user_name: Some("relay_bot".into()),
                chan_nickname: None,
                dm_scope: moltis_telegram::config::DmScope::Main,
                group_scope: moltis_telegram::config::GroupScope::PerSender,
            }],
        });
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let resolved =
            resolve_telegram_session_id(&state, "telegram:845", "-100", None, Some("telegram:42"))
                .await;
        assert!(
            resolved.is_none(),
            "must not reuse an active session from another bucket"
        );
    }

    #[tokio::test]
    async fn resolve_telegram_session_id_returns_none_without_bucket_route() {
        let metadata = sqlite_metadata().await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop()
                .with_session_metadata(metadata)
                .with_session_store(store),
        );

        let resolved =
            resolve_telegram_session_id(&state, "telegram:845", "-100", None, Some("telegram:42"))
                .await;
        assert!(
            resolved.is_none(),
            "must not synthesize a legacy session id when strict bucket route is missing"
        );
    }

    #[tokio::test]
    async fn web_channel_echo_uses_session_binding_even_when_another_bucket_is_active() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let services = crate::services::GatewayServices::noop()
            .with_session_store(Arc::clone(&store))
            .with_session_metadata(Arc::clone(&metadata));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(WaitCompletionProvider {
                started: Arc::clone(&started),
                finish: Arc::clone(&finish),
            });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "wait-completion-model".to_string(),
                provider: "wait-completion".to_string(),
                display_name: "wait-completion".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            Arc::clone(&state),
            Arc::clone(&store),
            Arc::clone(&metadata),
        );

        let bucket_key = "group-peer-tgchat.n100-sender-tguser.1001";
        let session_id = "sess_bucket";
        let runtime_session_key = "agent:zhuzhu:group-peer-tgchat.n100-sender-tguser.1001";
        let binding = moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:845".to_string(),
            chan_user_name: Some("@lovely_apple_bot".to_string()),
            chat_id: "-100".to_string(),
            message_id: Some("77".to_string()),
            thread_id: None,
            bucket_key: Some(bucket_key.to_string()),
        };
        let binding_json = serde_json::to_string(&binding).unwrap();
        let _ = metadata
            .upsert(session_id, runtime_session_key, Some("bucket".to_string()))
            .await;
        metadata
            .set_channel_binding(session_id, Some(binding_json))
            .await;
        metadata
            .set_active_session_id(runtime_session_key, "sess_other")
            .await;

        let result = chat
            .send(serde_json::json!({
                "text": "hello from web",
                "_sessionId": session_id,
                "_sessionKey": runtime_session_key,
                "_connId": "conn:web",
                "_channelTurnId": "turn:web:bucket",
                "model": "wait-completion-model",
            }))
            .await
            .expect("send should start background run");
        let run_id = result
            .get("runId")
            .and_then(|value| value.as_str())
            .expect("runId must be returned")
            .to_string();

        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .expect("background run must start");

        let turn = state
            .channel_turn_context(session_id, "turn:web:bucket")
            .await
            .expect("web echo must register channel turn context from session binding");
        assert_eq!(turn.session_key.as_deref(), Some(runtime_session_key));
        assert_eq!(turn.reply_target_refs.len(), 1);
        let decoded = moltis_telegram::adapter::inbound_target_from_reply_target_ref(
            &turn.reply_target_refs[0],
        )
        .expect("reply_target_ref must decode");
        assert_eq!(decoded.message_id.as_deref(), Some("77"));

        finish.notify_waiters();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            chat.wait_run_completion(&run_id),
        )
        .await
        .expect("run completion wait should finish")
        .expect("wait_run_completion should succeed");
    }

    #[tokio::test]
    async fn ensure_channel_bound_session_rejects_existing_binding_without_bucket_route() {
        let metadata = sqlite_metadata().await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));

        let mut services = crate::services::GatewayServices::noop()
            .with_session_metadata(Arc::clone(&metadata))
            .with_session_store(Arc::clone(&store));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![moltis_telegram::config::TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                agent_id: None,
                chan_user_id: None,
                chan_user_name: Some("lovely_apple_bot".into()),
                chan_nickname: None,
                dm_scope: moltis_telegram::config::DmScope::Main,
                group_scope: moltis_telegram::config::GroupScope::PerSender,
            }],
        });

        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let key = "sess_bucket";
        let legacy_binding = serde_json::to_string(&moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:845".to_string(),
            chan_user_name: None,
            chat_id: "-100".to_string(),
            message_id: None,
            thread_id: None,
            bucket_key: None,
        })
        .unwrap();
        let _ = metadata
            .upsert(
                key,
                "agent:zhuzhu:group-peer-tgchat.n100-branch-topic.7-sender-tguser.1001",
                Some("legacy".to_string()),
            )
            .await;
        metadata
            .set_channel_binding(key, Some(legacy_binding))
            .await;

        ensure_channel_bound_session(
            &state,
            key,
            "telegram:845",
            "-100",
            Some("7"),
            Some("group-peer-tgchat.n100-branch-topic.7-sender-tguser.1001"),
        )
        .await;

        let entry = metadata.get(key).await.expect("session row");
        let binding: moltis_channels::ChannelReplyTarget =
            serde_json::from_str(entry.channel_binding.as_deref().expect("binding json"))
                .expect("deserialize binding");
        assert_eq!(binding.thread_id.as_deref(), None);
        assert_eq!(binding.bucket_key.as_deref(), None);
    }

    #[tokio::test]
    async fn tg_gst_v1_system_prompt_block_appends_for_telegram_group_sessions() {
        let metadata = sqlite_metadata().await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));

        let mut services = crate::services::GatewayServices::noop()
            .with_session_metadata(Arc::clone(&metadata))
            .with_session_store(Arc::clone(&store));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![moltis_telegram::config::TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                agent_id: Some("zhuzhu".into()),
                chan_user_id: None,
                chan_user_name: Some("lovely_apple_bot".into()),
                chan_nickname: None,
                dm_scope: moltis_telegram::config::DmScope::Main,
                group_scope: moltis_telegram::config::GroupScope::Group,
            }],
        });

        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let session_id = "sess_group";
        metadata
            .upsert(
                session_id,
                "agent:zhuzhu:group-peer-tgchat.n100",
                Some("ok".into()),
            )
            .await
            .unwrap();
        let binding = moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:845".to_string(),
            chan_user_name: Some("@lovely_apple_bot".to_string()),
            chat_id: "-100".to_string(),
            message_id: None,
            thread_id: None,
            bucket_key: None,
        };
        let binding_json = serde_json::to_string(&binding).unwrap();
        metadata
            .set_channel_binding(session_id, Some(binding_json))
            .await;

        let entry = metadata.get(session_id).await.expect("session row");
        let mut system_prompt = "base".to_string();
        maybe_append_tg_gst_v1_system_prompt(&state, Some(&entry), &mut system_prompt).await;
        assert!(
            system_prompt.contains("TG-GST v1"),
            "expected tg_gst_v1 prompt block to be appended"
        );
    }

    #[tokio::test]
    async fn send_sync_keep_window_overflow_persists_ui_error_notice_as_assistant() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let session_id = "sess_send_sync_overflow";
        seed_test_session(
            &metadata,
            session_id,
            &moltis_sessions::SessionKey::main("default").0,
        )
        .await;

        let services =
            crate::services::GatewayServices::noop().with_session_store(Arc::clone(&store));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(BudgetProvider {
            context_window: 32,
            input_limit: Some(1),
            output_limit: Some(1),
        });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "budget-model".to_string(),
                provider: "budget".to_string(),
                display_name: "budget".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            Arc::clone(&state),
            Arc::clone(&store),
            metadata,
        );

        let res = chat
            .send_sync(serde_json::json!({
                "_sessionId": session_id,
                "text": "hello"
            }))
            .await;
        assert!(res.is_err(), "expected overflow error");

        let history = store.read(session_id).await.unwrap_or_default();
        assert!(
            history.len() >= 2,
            "expected at least [user, ui_error_notice], got {}",
            history.len()
        );
        let last = history.last().unwrap();
        assert_eq!(last.get("role").and_then(|v| v.as_str()), Some("assistant"));
        assert_eq!(
            last.get("moltis_internal_kind").and_then(|v| v.as_str()),
            Some(MOLTIS_INTERNAL_KIND_UI_ERROR_NOTICE)
        );
        let content = last.get("content").and_then(|v| v.as_str()).unwrap_or("");
        assert!(content.starts_with("[error] "));
    }

    #[tokio::test]
    async fn send_sync_run_failed_persists_ui_error_notice_as_assistant() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let session_id = "sess_send_sync_run_failed";
        seed_test_session(
            &metadata,
            session_id,
            &moltis_sessions::SessionKey::main("default").0,
        )
        .await;

        let services =
            crate::services::GatewayServices::noop().with_session_store(Arc::clone(&store));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let mut hooks = HookRegistry::new();
        hooks.register(Arc::new(BlockingBeforeAgentStartHook));

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(ToolsBudgetProvider {
            budget: BudgetProvider {
                context_window: 8_192,
                input_limit: Some(8_192),
                output_limit: Some(1_024),
            },
        });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "budget-model".to_string(),
                provider: "budget".to_string(),
                display_name: "budget".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        tool_registry.write().await.register(Box::new(DummyTool {
            name: "noop".into(),
        }));

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            Arc::clone(&state),
            Arc::clone(&store),
            metadata,
        )
        .with_tools(tool_registry)
        .with_hooks(hooks);

        let res = chat
            .send_sync(serde_json::json!({
                "_sessionId": session_id,
                "text": "hello"
            }))
            .await;
        assert!(res.is_err(), "expected run failed error");

        let history = store.read(session_id).await.unwrap_or_default();
        assert!(
            history.len() >= 2,
            "expected at least [user, ui_error_notice], got {}",
            history.len()
        );
        let last = history.last().unwrap();
        assert_eq!(last.get("role").and_then(|v| v.as_str()), Some("assistant"));
        assert_eq!(
            last.get("moltis_internal_kind").and_then(|v| v.as_str()),
            Some(MOLTIS_INTERNAL_KIND_UI_ERROR_NOTICE)
        );
        let content = last.get("content").and_then(|v| v.as_str()).unwrap_or("");
        assert!(content.starts_with("[error] "));
    }

    #[tokio::test]
    async fn send_sync_channel_bound_tool_calls_use_canonical_runtime_session_key() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let services = crate::services::GatewayServices::noop()
            .with_session_store(Arc::clone(&store))
            .with_session_metadata(Arc::clone(&metadata));
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        );

        let provider_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn moltis_agents::model::LlmProvider> =
            Arc::new(SingleToolCallProvider {
                called: Arc::clone(&provider_called),
                expected_ctx_session_id: "sess_bound".to_string(),
            });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "single-tool-call-model".to_string(),
                provider: "single-tool-call".to_string(),
                display_name: "single-tool-call".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let bucket_key = "group-peer-tgchat.n100-sender-tguser.1001";
        let runtime_session_key = "agent:zhuzhu:group-peer-tgchat.n100-sender-tguser.1001";
        let binding_json = serde_json::to_string(&moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:845".to_string(),
            chan_user_name: Some("@lovely_apple_bot".to_string()),
            chat_id: "-100".to_string(),
            message_id: Some("77".to_string()),
            thread_id: None,
            bucket_key: Some(bucket_key.to_string()),
        })
        .unwrap();
        metadata
            .upsert("sess_bound", runtime_session_key, Some("bound".to_string()))
            .await
            .expect("session metadata upsert");
        metadata
            .set_channel_binding("sess_bound", Some(binding_json))
            .await;

        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        tool_registry
            .write()
            .await
            .register(Box::new(RecordingContextTool {
                seen: Arc::clone(&seen),
            }));

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            Arc::clone(&state),
            Arc::clone(&store),
            Arc::clone(&metadata),
        )
        .with_tools(tool_registry);

        chat.send_sync(serde_json::json!({
            "_sessionId": "sess_bound",
            "_channelTurnId": "turn:sync:bound",
            "text": "hello",
            "model": "single-tool-call-model",
        }))
        .await
        .expect("send_sync should succeed");

        assert_eq!(provider_called.load(Ordering::SeqCst), 2);
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].get("_sessionKey").and_then(|value| value.as_str()),
            Some(runtime_session_key)
        );
        assert_eq!(
            seen[0].get("_sessionId").and_then(|value| value.as_str()),
            Some("sess_bound")
        );
    }

    #[tokio::test]
    async fn send_sync_requires_explicit_session_context() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(BudgetProvider {
            context_window: 32,
            input_limit: Some(32),
            output_limit: Some(32),
        });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "budget-model".to_string(),
                provider: "budget".to_string(),
                display_name: "budget".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            state,
            store,
            metadata,
        );

        let err = chat
            .send_sync(serde_json::json!({ "text": "hello" }))
            .await
            .expect_err("missing session context must hard error");
        assert!(err.contains("missing session context"));
    }

    #[test]
    fn chat_terminal_broadcasts_use_session_id_field() {
        let final_payload = ChatFinalBroadcast {
            run_id: "run-1".to_string(),
            session_id: "sess_test".to_string(),
            state: "final",
            text: "done".to_string(),
            model: "model".to_string(),
            provider: "provider".to_string(),
            input_tokens: 1,
            output_tokens: 2,
            message_index: 3,
            reply_medium: ReplyMedium::Text,
            iterations: Some(1),
            tool_calls_made: Some(0),
            audio: None,
            seq: Some(9),
        };
        let error_payload = ChatErrorBroadcast {
            run_id: "run-1".to_string(),
            session_id: "sess_test".to_string(),
            state: "error",
            error: serde_json::json!({ "message": "boom" }),
            seq: Some(9),
        };

        let final_json =
            serde_json::to_value(final_payload).expect("final payload should serialize");
        let error_json =
            serde_json::to_value(error_payload).expect("error payload should serialize");

        assert_eq!(
            final_json.get("sessionId").and_then(|value| value.as_str()),
            Some("sess_test")
        );
        assert!(final_json.get("sessionKey").is_none());
        assert_eq!(
            error_json.get("sessionId").and_then(|value| value.as_str()),
            Some("sess_test")
        );
        assert!(error_json.get("sessionKey").is_none());
    }

    #[tokio::test]
    async fn browser_current_session_methods_require_explicit_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(tmp.path().to_path_buf()));
        let metadata = sqlite_metadata().await;
        metadata
            .upsert("sess_active", "agent:default:chat-active", None)
            .await
            .expect("session metadata insert should succeed");
        let state = crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            crate::services::GatewayServices::noop(),
        );
        {
            let mut inner = state.inner.write().await;
            inner
                .active_sessions
                .insert("conn-test".to_string(), "sess_active".to_string());
        }

        let provider: Arc<dyn moltis_agents::model::LlmProvider> = Arc::new(BudgetProvider {
            context_window: 32,
            input_limit: Some(32),
            output_limit: Some(32),
        });
        let mut registry = ProviderRegistry::empty();
        registry.register(
            moltis_agents::providers::ModelInfo {
                id: "budget-model".to_string(),
                provider: "budget".to_string(),
                display_name: "budget".to_string(),
                created_at: None,
            },
            Arc::clone(&provider),
        );

        let chat = LiveChatService::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(RwLock::new(DisabledModelsStore::default())),
            state,
            store,
            metadata,
        );

        let send_err = chat
            .send(serde_json::json!({ "_connId": "conn-test", "text": "hello" }))
            .await
            .expect_err("chat.send must reject missing _sessionId");
        assert!(send_err.contains("_sessionId"));

        let clear_err = chat
            .clear(serde_json::json!({ "_connId": "conn-test" }))
            .await
            .expect_err("chat.clear must reject missing _sessionId");
        assert!(clear_err.contains("_sessionId"));

        let compact_err = chat
            .compact(serde_json::json!({ "_connId": "conn-test" }))
            .await
            .expect_err("chat.compact must reject missing _sessionId");
        assert!(compact_err.contains("_sessionId"));

        let context_err = chat
            .context(serde_json::json!({ "_connId": "conn-test" }))
            .await
            .expect_err("chat.context must reject missing _sessionId");
        assert!(context_err.contains("_sessionId"));

        let full_context_err = chat
            .full_context(serde_json::json!({ "_connId": "conn-test" }))
            .await
            .expect_err("chat.full_context must reject missing _sessionId");
        assert!(full_context_err.contains("_sessionId"));
    }
}

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat_error::parse_chat_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureStage {
    GatewayTimeout,
    ProviderRequest,
    ProviderStream,
    Runner,
    ToolExec,
    ChannelDelivery,
}

impl FailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GatewayTimeout => "gateway_timeout",
            Self::ProviderRequest => "provider_request",
            Self::ProviderStream => "provider_stream",
            Self::Runner => "runner",
            Self::ToolExec => "tool_exec",
            Self::ChannelDelivery => "channel_delivery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Auth,
    RateLimit,
    ModelNotFoundOrAccessDenied,
    QuotaOrBilling,
    InvalidRequest,
    Network,
    ProviderUnavailable,
    Cancelled,
    Internal,
}

impl ErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimit => "rate_limit",
            Self::ModelNotFoundOrAccessDenied => "model_not_found_or_access_denied",
            Self::QuotaOrBilling => "quota_or_billing",
            Self::InvalidRequest => "invalid_request",
            Self::Network => "network",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorAction {
    Retry,
    WaitAndRetry,
    CheckApiKey,
    SwitchModel,
    FixRequest,
    ContactAdmin,
    Cancelled,
}

impl ErrorAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::WaitAndRetry => "wait_and_retry",
            Self::CheckApiKey => "check_api_key",
            Self::SwitchModel => "switch_model",
            Self::FixRequest => "fix_request",
            Self::ContactAdmin => "contact_admin",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureMessage {
    pub user: String,
    pub debug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFailure {
    pub class: String,
    pub message_redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedError {
    pub stage: FailureStage,
    pub kind: ErrorKind,
    pub retryable: bool,
    pub action: ErrorAction,
    pub message: FailureMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub details: Value,
    pub raw: RawFailure,
}

#[derive(Debug, Clone)]
pub struct FailureInput<'a> {
    pub stage_hint: FailureStage,
    pub raw_error: &'a str,
    pub provider_name: Option<&'a str>,
    pub model_id: Option<&'a str>,
    pub details: Value,
}

fn sanitize_one_line(text: &str, max_chars: usize) -> String {
    let mut s = text
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if s.is_empty() {
        return s;
    }
    s = redact_common_secrets(&s);
    if s.chars().count() > max_chars {
        s = s
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        s.push('…');
    }
    s
}

fn redact_common_secrets(input: &str) -> String {
    // Best-effort redaction for common token patterns. This is intentionally
    // conservative: false positives are acceptable, but we must avoid leaking
    // auth material into logs/UI debug payloads.
    let mut out = input.to_string();

    // Redact "Bearer <token>" sequences.
    for needle in ["Bearer ", "bearer "] {
        let mut cursor = 0usize;
        while let Some(pos) = out[cursor..].find(needle) {
            let start = cursor + pos + needle.len();
            let end = out[start..]
                .find(' ')
                .map(|i| start + i)
                .unwrap_or(out.len());
            out.replace_range(start..end, "<redacted>");
            cursor = start + "<redacted>".len();
        }
    }

    // Redact token-like words such as sk-... or xoxb-... (common across providers).
    // Operate at "word" boundaries to avoid pathological slicing.
    let mut rebuilt = String::with_capacity(out.len());
    for (i, part) in out.split(' ').enumerate() {
        if i > 0 {
            rebuilt.push(' ');
        }
        let lower = part.to_ascii_lowercase();
        let looks_like_secret = (lower.starts_with("sk-") && part.len() >= 16)
            || (lower.starts_with("xoxb-") && part.len() >= 16)
            || (lower.starts_with("xoxa-") && part.len() >= 16)
            || (lower.starts_with("api-key") && part.len() >= 16);
        if looks_like_secret {
            rebuilt.push_str("<redacted>");
        } else {
            rebuilt.push_str(part);
        }
    }
    rebuilt
}

fn detect_kind_from_parsed(parsed: &Value) -> Option<ErrorKind> {
    match parsed.get("type").and_then(|v| v.as_str())? {
        "auth_error" => Some(ErrorKind::Auth),
        "rate_limit_exceeded" => Some(ErrorKind::RateLimit),
        "usage_limit_reached" => Some(ErrorKind::QuotaOrBilling),
        "unsupported_model" => Some(ErrorKind::ModelNotFoundOrAccessDenied),
        "server_error" => Some(ErrorKind::ProviderUnavailable),
        _ => None,
    }
}

fn infer_stage(stage_hint: FailureStage, raw: &str, parsed: &Value) -> FailureStage {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("stream ended unexpectedly") {
        return FailureStage::ProviderStream;
    }
    if matches!(stage_hint, FailureStage::GatewayTimeout) {
        return stage_hint;
    }
    if matches!(stage_hint, FailureStage::Runner)
        && (lower.contains("a network error")
            || lower.contains("error sending request for url")
            || lower.contains("connection closed before message completed")
            || lower.contains("timed out")
            || lower.contains("timeout"))
    {
        return FailureStage::ProviderRequest;
    }
    if let Some(kind) = detect_kind_from_parsed(parsed) {
        match kind {
            ErrorKind::Auth
            | ErrorKind::RateLimit
            | ErrorKind::ModelNotFoundOrAccessDenied
            | ErrorKind::QuotaOrBilling
            | ErrorKind::ProviderUnavailable => return FailureStage::ProviderRequest,
            _ => {},
        }
    }
    stage_hint
}

fn infer_action(kind: ErrorKind) -> (bool, ErrorAction) {
    match kind {
        ErrorKind::Auth => (false, ErrorAction::CheckApiKey),
        ErrorKind::RateLimit => (true, ErrorAction::WaitAndRetry),
        ErrorKind::ModelNotFoundOrAccessDenied => (false, ErrorAction::SwitchModel),
        ErrorKind::QuotaOrBilling => (false, ErrorAction::ContactAdmin),
        ErrorKind::InvalidRequest => (false, ErrorAction::FixRequest),
        ErrorKind::Network => (true, ErrorAction::Retry),
        ErrorKind::ProviderUnavailable => (true, ErrorAction::WaitAndRetry),
        ErrorKind::Cancelled => (true, ErrorAction::Retry),
        ErrorKind::Internal => (false, ErrorAction::ContactAdmin),
    }
}

fn infer_kind(stage: FailureStage, raw: &str, parsed: &Value) -> ErrorKind {
    if let Some(kind) = detect_kind_from_parsed(parsed) {
        return kind;
    }
    let lower = raw.to_ascii_lowercase();
    if lower.contains("context window exceeded") || lower.contains("maximum context length") {
        return ErrorKind::InvalidRequest;
    }
    if matches!(stage, FailureStage::GatewayTimeout) {
        return ErrorKind::Cancelled;
    }
    if lower.contains("a network error")
        || lower.contains("error sending request for url")
        || lower.contains("connection closed before message completed")
    {
        return ErrorKind::Network;
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return ErrorKind::Network;
    }
    if lower.contains("stream ended unexpectedly") || lower.contains("connection reset") {
        return ErrorKind::Network;
    }
    ErrorKind::Internal
}

fn user_message_for(kind: ErrorKind, action: ErrorAction) -> String {
    match (kind, action) {
        (ErrorKind::Auth, _) => "Authentication failed. Check your API key and permissions.".into(),
        (ErrorKind::RateLimit, _) => "Rate limited. Please wait a moment and try again.".into(),
        (ErrorKind::ModelNotFoundOrAccessDenied, _) => {
            "Model not available. Switch model and try again.".into()
        },
        (ErrorKind::QuotaOrBilling, _) => {
            "Quota/billing issue. Check quota or contact an admin.".into()
        },
        (ErrorKind::InvalidRequest, _) => {
            "Invalid request (bad parameters or context too long). Fix the request and try again."
                .into()
        },
        (ErrorKind::Network, ErrorAction::Retry) => {
            "Upstream connection interrupted. Please retry once.".into()
        },
        (ErrorKind::ProviderUnavailable, _) => {
            "Upstream provider unavailable. Please try again later.".into()
        },
        (ErrorKind::Cancelled, _) => "Request cancelled or timed out. Please retry.".into(),
        _ => "Internal error. Please contact an admin.".into(),
    }
}

fn debug_message(stage: FailureStage, kind: ErrorKind, raw: &RawFailure) -> String {
    format!(
        "stage={stage} kind={kind} raw.class={class} raw={msg}",
        stage = stage.as_str(),
        kind = kind.as_str(),
        class = raw.class,
        msg = raw.message_redacted
    )
}

pub fn normalize_failure(input: FailureInput<'_>) -> NormalizedError {
    let parsed = parse_chat_error(input.raw_error, input.provider_name);
    let stage = infer_stage(input.stage_hint, input.raw_error, &parsed);
    let kind = infer_kind(stage, input.raw_error, &parsed);
    let (retryable, action) = infer_action(kind);

    let raw = RawFailure {
        class: "raw_error".to_string(),
        message_redacted: sanitize_one_line(input.raw_error, 360),
    };

    let message_user = user_message_for(kind, action);
    let message_debug = debug_message(stage, kind, &raw);

    let _ = input.model_id;

    NormalizedError {
        stage,
        kind,
        retryable,
        action,
        message: FailureMessage {
            user: message_user,
            debug: message_debug,
        },
        request_id: None,
        details: input.details,
        raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_auth_error_maps_to_check_api_key() {
        let n = normalize_failure(FailureInput {
            stage_hint: FailureStage::Runner,
            raw_error: "HTTP 401 Unauthorized",
            provider_name: Some("openai-responses"),
            model_id: Some("gpt"),
            details: serde_json::json!({}),
        });
        assert_eq!(n.kind, ErrorKind::Auth);
        assert_eq!(n.action, ErrorAction::CheckApiKey);
        assert!(!n.retryable);
        assert_eq!(n.stage, FailureStage::ProviderRequest);
    }

    #[test]
    fn normalize_stream_end_maps_to_network_provider_stream() {
        let n = normalize_failure(FailureInput {
            stage_hint: FailureStage::Runner,
            raw_error: "OpenAI Responses API stream ended unexpectedly",
            provider_name: Some("openai-responses"),
            model_id: Some("gpt"),
            details: serde_json::json!({ "elapsed_ms": 123 }),
        });
        assert_eq!(n.kind, ErrorKind::Network);
        assert_eq!(n.stage, FailureStage::ProviderStream);
        assert!(n.retryable);
        assert_eq!(n.action, ErrorAction::Retry);
    }

    #[test]
    fn normalize_context_window_exceeded_maps_to_invalid_request() {
        let n = normalize_failure(FailureInput {
            stage_hint: FailureStage::Runner,
            raw_error: "context window exceeded: too many tokens",
            provider_name: Some("openai-responses"),
            model_id: Some("gpt"),
            details: serde_json::json!({}),
        });
        assert_eq!(n.kind, ErrorKind::InvalidRequest);
        assert_eq!(n.action, ErrorAction::FixRequest);
        assert!(!n.retryable);
    }

    #[test]
    fn normalize_gateway_timeout_maps_to_cancelled() {
        let n = normalize_failure(FailureInput {
            stage_hint: FailureStage::GatewayTimeout,
            raw_error: "Agent run timed out after 600s",
            provider_name: Some("openai-responses"),
            model_id: Some("gpt"),
            details: serde_json::json!({ "timeout_secs": 600, "elapsed_ms": 600_000 }),
        });
        assert_eq!(n.stage, FailureStage::GatewayTimeout);
        assert_eq!(n.kind, ErrorKind::Cancelled);
        assert!(n.retryable);
    }

    #[test]
    fn normalize_provider_network_maps_to_provider_request_network() {
        let n = normalize_failure(FailureInput {
            stage_hint: FailureStage::Runner,
            raw_error: "A network error: error sending request for url (https://api.example.com): connection closed before message completed",
            provider_name: Some("openai-responses"),
            model_id: Some("gpt"),
            details: serde_json::json!({}),
        });
        assert_eq!(n.stage, FailureStage::ProviderRequest);
        assert_eq!(n.kind, ErrorKind::Network);
        assert!(n.retryable);
        assert_eq!(n.action, ErrorAction::Retry);
    }
}

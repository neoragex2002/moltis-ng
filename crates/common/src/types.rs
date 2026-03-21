use serde::{Deserialize, Serialize};

/// Unique identifier for an agent.
pub type AgentId = String;

pub type ChanAccountKey = String;

/// Unique identifier for a peer (user on a channel).
pub type PeerId = String;

/// Channel identifier (e.g. "telegram", "discord", "whatsapp").
pub type ChannelId = String;

/// Minimal cross-channel delivery target details derived from `channel_binding`.
///
/// This is **optional** and intended for observability/hooks. It must never be
/// used as a routing key; routing is based on `session_id/session_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTarget {
    /// Channel type (e.g. "telegram").
    #[serde(rename = "type")]
    pub channel_type: String,
    pub account_key: String,
    pub chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Chat type for routing and session scoping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    Dm,
    Group,
    Channel,
}

/// V3 inbound message context placeholder (internal only).
///
/// This type exists to keep legacy pipeline crates compiling while the
/// system transitions to V3 `session_id` / `session_key` semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundContextV3 {
    /// Unique identifier for a specific session instance.
    pub session_id: String,
    /// Cross-domain logical session bucket key.
    pub session_key: String,
    /// Canonical inbound text body.
    pub body: String,
}

/// Outbound reply payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyPayload {
    pub text: String,
    pub media: Option<MediaAttachment>,
    pub reply_to_message_id: Option<String>,
    pub silent: bool,
}

/// Media attachment for outbound messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    pub url: String,
    pub mime_type: String,
}

use serde::{Deserialize, Serialize};

/// Unique identifier for an agent.
pub type AgentId = String;

pub type ChanAccountKey = String;

/// Unique identifier for a peer (user on a channel).
pub type PeerId = String;

/// Channel identifier (e.g. "telegram", "discord", "whatsapp").
pub type ChannelId = String;

/// Chat type for routing and session scoping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    Dm,
    Group,
    Channel,
}

/// Normalized inbound message context (mirrors MsgContext from TypeScript).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsgContext {
    pub body: String,
    pub from: PeerId,
    pub to: String,
    pub chan_type: ChannelId,
    pub chan_account_key: ChanAccountKey,
    pub chat_type: ChatType,
    pub session_id: String,
    pub chan_chat_key: String,
    pub reply_to_message_id: Option<String>,
    pub media_path: Option<String>,
    pub media_url: Option<String>,
    pub group_id: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    pub sender_name: Option<String>,
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

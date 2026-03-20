use anyhow::Result;
use async_trait::async_trait;

use crate::config::{
    DmScope, GroupScope, GroupSessionTranscriptFormat, TelegramAccountConfig,
    TelegramBusAccountSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgTranscriptFormat {
    Legacy,
    TgGstV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgInboundKind {
    Dm,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgInboundMode {
    Dispatch,
    RecordOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgContent {
    pub text: String,
    pub has_attachments: bool,
    pub has_location: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgPrivateSource {
    pub account_handle: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
    pub peer: String,
    pub sender: Option<String>,
    pub addressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgInbound {
    pub kind: TgInboundKind,
    pub mode: TgInboundMode,
    pub body: TgContent,
    pub private_source: TgPrivateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgRoute {
    pub peer: String,
    pub sender: Option<String>,
    pub bucket_key: String,
    pub addressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgPrivateTarget {
    pub account_handle: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgReply {
    pub output: String,
    pub private_target: TgPrivateTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgAttachment {
    pub media_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgFollowUpTarget {
    pub route: TgRoute,
    pub private_target: TgPrivateTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgInboundRequest {
    pub inbound: TgInbound,
    pub route: TgRoute,
    pub private_target: TgPrivateTarget,
    pub transcript_format: TgTranscriptFormat,
    pub sender_name: Option<String>,
    pub username: Option<String>,
    pub sender_id: Option<u64>,
    pub sender_is_bot: bool,
    pub model: Option<String>,
    pub message_kind: Option<moltis_channels::ChannelMessageKind>,
    pub attachments: Vec<TgAttachment>,
}

#[async_trait]
pub trait TelegramCoreBridge: Send + Sync {
    async fn handle_inbound(&self, request: TgInboundRequest);

    async fn dispatch_command(
        &self,
        command: &str,
        target: TgFollowUpTarget,
    ) -> Result<String>;

    async fn request_voice_transcription(&self, audio_data: &[u8], format: &str) -> Result<String>;

    async fn voice_transcription_available(&self) -> bool;

    async fn update_location(
        &self,
        target: TgFollowUpTarget,
        latitude: f64,
        longitude: f64,
    ) -> bool;
}

pub const TG_GST_V1_SYSTEM_PROMPT_BLOCK: &str = r#"## Telegram Group Transcript (TG-GST v1)
- Some inbound messages in this session may be formatted as: <speaker><addr_flag>: <body>
- If <addr_flag> is " -> you", the message is explicitly addressed to you and requires your attention.
- When replying/summarizing:
  - Do NOT output transcript-style lines like "<speaker>: ...". Use normal prose/bullets.
  - Do NOT start a line with "@someone" unless you intentionally want to delegate (this may trigger relay).
  - If you must quote a line containing "@mentions", wrap it in '>' quote lines or fenced code blocks."#;

pub fn transcript_format_from_snapshot(
    snapshots: &[TelegramBusAccountSnapshot],
    account_handle: &str,
) -> TgTranscriptFormat {
    match snapshots
        .iter()
        .find(|snapshot| snapshot.account_handle == account_handle)
        .map(|snapshot| snapshot.group_session_transcript_format.clone())
        .unwrap_or(GroupSessionTranscriptFormat::Legacy)
    {
        GroupSessionTranscriptFormat::Legacy => TgTranscriptFormat::Legacy,
        GroupSessionTranscriptFormat::TgGstV1 => TgTranscriptFormat::TgGstV1,
    }
}

pub fn tg_gst_v1_system_prompt_block_for_binding(
    binding: &str,
    snapshots: &[TelegramBusAccountSnapshot],
) -> Option<&'static str> {
    let target = serde_json::from_str::<moltis_channels::ChannelReplyTarget>(binding).ok()?;
    if target.chan_type != moltis_channels::ChannelType::Telegram {
        return None;
    }
    let chat_i64 = target.chat_id.parse::<i64>().ok()?;
    if chat_i64 >= 0 {
        return None;
    }
    (transcript_format_from_snapshot(snapshots, &target.chan_account_key) == TgTranscriptFormat::TgGstV1)
        .then_some(TG_GST_V1_SYSTEM_PROMPT_BLOCK)
}

pub fn resolve_group_relay_route(
    snapshots: &[TelegramBusAccountSnapshot],
    account_handle: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    sender_account_key: Option<&str>,
) -> Option<TgRoute> {
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.account_handle == account_handle)?;
    let sender = sender_account_key.map(|value| value.strip_prefix("telegram:").unwrap_or(value));
    Some(TgRoute {
        peer: chat_id.to_string(),
        sender: sender.map(str::to_string),
        bucket_key: resolve_group_bucket_key(
            &snapshot.group_scope,
            &snapshot.account_handle,
            chat_id,
            sender,
            thread_id,
        ),
        addressed: true,
    })
}

pub fn build_group_relay_reply(
    snapshots: &[TelegramBusAccountSnapshot],
    target: TgPrivateTarget,
    source_account_id: &str,
    source_account_handle: Option<&str>,
    task_text: &str,
) -> TgReply {
    let source_handle = source_account_handle
        .map(str::to_string)
        .unwrap_or_else(|| format!("@{source_account_id}"));
    let source_username = source_handle.strip_prefix('@').unwrap_or(&source_handle);
    let output = match transcript_format_from_snapshot(snapshots, &target.account_handle) {
        TgTranscriptFormat::Legacy => format!("（来自 {source_handle}）{}", task_text.trim()),
        TgTranscriptFormat::TgGstV1 => {
            format!("{source_username}(bot) -> you: {}", task_text.trim())
        },
    };
    TgReply {
        output,
        private_target: target,
    }
}

pub fn resolve_tg_route(config: &TelegramAccountConfig, inbound: &TgInbound) -> TgRoute {
    let peer = inbound.private_source.peer.clone();
    let sender = inbound.private_source.sender.clone();
    let branch = inbound.private_source.thread_id.as_deref();
    let bucket_key = match inbound.kind {
        TgInboundKind::Dm => resolve_dm_bucket_key(
            &config.dm_scope,
            &inbound.private_source.account_handle,
            &peer,
        ),
        TgInboundKind::Group => resolve_group_bucket_key(
            &config.group_scope,
            &inbound.private_source.account_handle,
            &peer,
            sender.as_deref(),
            branch,
        ),
    };

    TgRoute {
        peer,
        sender,
        bucket_key,
        addressed: inbound.private_source.addressed,
    }
}

pub fn resolve_dm_bucket_key(dm_scope: &DmScope, account_handle: &str, peer: &str) -> String {
    match dm_scope {
        DmScope::Main => "dm:main".to_string(),
        DmScope::PerPeer => format!("dm:peer:{peer}"),
        DmScope::PerChannel => format!("dm:channel:telegram:peer:{peer}"),
        DmScope::PerAccount => format!("dm:account:{account_handle}:peer:{peer}"),
    }
}

pub fn resolve_group_bucket_key(
    group_scope: &GroupScope,
    account_handle: &str,
    peer: &str,
    sender: Option<&str>,
    branch: Option<&str>,
) -> String {
    let prefix = format!("group:account:{account_handle}:peer:{peer}");
    match group_scope {
        GroupScope::Group => prefix,
        GroupScope::PerSender => sender
            .map(|sender| format!("{prefix}:sender:{sender}"))
            .unwrap_or(prefix),
        GroupScope::PerBranch => branch
            .map(|branch| format!("{prefix}:branch:{branch}"))
            .unwrap_or(prefix),
        GroupScope::PerBranchSender => match (branch, sender) {
            (Some(branch), Some(sender)) => format!("{prefix}:branch:{branch}:sender:{sender}"),
            (Some(branch), None) => format!("{prefix}:branch:{branch}"),
            (None, Some(sender)) => format!("{prefix}:sender:{sender}"),
            (None, None) => prefix,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_bucket_key_follows_scope() {
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::Main, "telegram:a", "peer-1"),
            "dm:main"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerPeer, "telegram:a", "peer-1"),
            "dm:peer:peer-1"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerChannel, "telegram:a", "peer-1"),
            "dm:channel:telegram:peer:peer-1"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerAccount, "telegram:a", "peer-1"),
            "dm:account:telegram:a:peer:peer-1"
        );
    }

    #[test]
    fn group_bucket_key_degrades_when_sender_or_branch_missing() {
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerSender,
                "telegram:a",
                "peer-1",
                None,
                Some("7"),
            ),
            "group:account:telegram:a:peer:peer-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranch,
                "telegram:a",
                "peer-1",
                Some("sender-1"),
                None,
            ),
            "group:account:telegram:a:peer:peer-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranchSender,
                "telegram:a",
                "peer-1",
                Some("sender-1"),
                None,
            ),
            "group:account:telegram:a:peer:peer-1:sender:sender-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranchSender,
                "telegram:a",
                "peer-1",
                None,
                Some("7"),
            ),
            "group:account:telegram:a:peer:peer-1:branch:7"
        );
    }

    #[test]
    fn resolve_route_uses_configured_scope() {
        let config = TelegramAccountConfig {
            dm_scope: DmScope::PerAccount,
            group_scope: GroupScope::PerBranchSender,
            ..Default::default()
        };
        let inbound = TgInbound {
            kind: TgInboundKind::Group,
            mode: TgInboundMode::Dispatch,
            body: TgContent {
                text: "hello".into(),
                has_attachments: false,
                has_location: false,
            },
            private_source: TgPrivateSource {
                account_handle: "telegram:test".into(),
                chat_id: "-1001".into(),
                message_id: Some("99".into()),
                thread_id: Some("7".into()),
                peer: "-1001".into(),
                sender: Some("u-1".into()),
                addressed: true,
            },
        };
        let route = resolve_tg_route(&config, &inbound);
        assert_eq!(route.peer, "-1001");
        assert_eq!(route.sender.as_deref(), Some("u-1"));
        assert_eq!(
            route.bucket_key,
            "group:account:telegram:test:peer:-1001:branch:7:sender:u-1"
        );
        assert!(route.addressed);
    }
}

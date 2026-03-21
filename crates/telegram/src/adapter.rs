use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::{DmScope, GroupScope, TelegramAccountConfig, TelegramBusAccountSnapshot};

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

    async fn dispatch_command(&self, command: &str, target: TgFollowUpTarget) -> Result<String>;

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
    _snapshots: &[TelegramBusAccountSnapshot],
    _account_handle: &str,
) -> TgTranscriptFormat {
    TgTranscriptFormat::TgGstV1
}

pub fn tg_gst_v1_system_prompt_block_for_binding(
    binding: &str,
    snapshots: &[TelegramBusAccountSnapshot],
) -> Option<&'static str> {
    let target = telegram_reply_target_from_binding(binding)?;
    let chat_i64 = target.chat_id.parse::<i64>().ok()?;
    if chat_i64 >= 0 {
        return None;
    }
    let _ = snapshots;
    Some(TG_GST_V1_SYSTEM_PROMPT_BLOCK)
}

fn tg_gst_v1_apply_media_placeholder(
    kind: Option<moltis_channels::ChannelMessageKind>,
    body: &str,
) -> String {
    let body = body.to_string();
    let Some(kind) = kind else {
        return body;
    };
    if matches!(kind, moltis_channels::ChannelMessageKind::Text) {
        return body;
    }

    let tag = match kind {
        moltis_channels::ChannelMessageKind::Photo => "photo",
        moltis_channels::ChannelMessageKind::Video => "video",
        moltis_channels::ChannelMessageKind::Voice => "voice",
        moltis_channels::ChannelMessageKind::Audio => "audio",
        moltis_channels::ChannelMessageKind::Document => "file",
        moltis_channels::ChannelMessageKind::Location => "location",
        _ => "attachment",
    };

    if body.trim().is_empty() {
        format!("[{tag}]")
    } else {
        format!("[{tag}] caption: {body}")
    }
}

fn tg_gst_v1_format_speaker(
    username: Option<&str>,
    user_id: Option<u64>,
    sender_name: Option<&str>,
    sender_is_bot: bool,
) -> String {
    fn normalize_display_name(name: &str) -> String {
        let normalized = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let max_chars = 64usize;
        if normalized.chars().count() <= max_chars {
            return normalized;
        }
        normalized.chars().take(max_chars).collect()
    }

    let mut speaker = if let Some(u) = username.filter(|s| !s.trim().is_empty()) {
        u.trim().to_string()
    } else if let Some(id) = user_id {
        let display = sender_name
            .map(normalize_display_name)
            .filter(|s| !s.is_empty());
        if let Some(d) = display {
            format!("tg:{id}({d})")
        } else {
            format!("tg:{id}")
        }
    } else {
        "tg:unknown".to_string()
    };

    if sender_is_bot {
        speaker.push_str("(bot)");
    }
    speaker
}

pub fn tg_gst_v1_format_inbound_text(
    body: &str,
    message_kind: Option<moltis_channels::ChannelMessageKind>,
    username: Option<&str>,
    sender_id: Option<u64>,
    sender_name: Option<&str>,
    sender_is_bot: bool,
    addressed: bool,
) -> String {
    let body = tg_gst_v1_apply_media_placeholder(message_kind, body);
    if body.trim().is_empty() {
        return body;
    }
    let speaker = tg_gst_v1_format_speaker(username, sender_id, sender_name, sender_is_bot);
    let addr_flag = if addressed {
        " -> you"
    } else {
        ""
    };
    format!("{speaker}{addr_flag}: {body}")
}

pub fn channel_target_from_binding(binding: &str) -> Option<moltis_common::types::ChannelTarget> {
    let target = telegram_reply_target_from_binding(binding)?;
    Some(moltis_common::types::ChannelTarget {
        channel_type: target.chan_type.as_str().to_string(),
        account_key: target.chan_account_key,
        chat_id: target.chat_id,
        thread_id: target.thread_id,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelegramReplyTargetRefV1 {
    v: u8,
    #[serde(rename = "type")]
    channel_type: String,
    account_key: String,
    chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyTelegramChannelBinding {
    channel_type: String,
    #[serde(default)]
    account_handle: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    chat_id: String,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    bucket_key: Option<String>,
}

fn telegram_reply_target_from_binding(
    binding: &str,
) -> Option<moltis_channels::ChannelReplyTarget> {
    if let Ok(target) = serde_json::from_str::<moltis_channels::ChannelReplyTarget>(binding) {
        if target.chan_type == moltis_channels::ChannelType::Telegram {
            return Some(target);
        }
        return None;
    }

    let legacy: LegacyTelegramChannelBinding = serde_json::from_str(binding).ok()?;
    if legacy.channel_type != "telegram" {
        return None;
    }
    let account_key = legacy.account_handle.or(legacy.account_id)?;
    Some(moltis_channels::ChannelReplyTarget {
        chan_type: moltis_channels::ChannelType::Telegram,
        chan_account_key: account_key,
        chan_user_name: None,
        chat_id: legacy.chat_id,
        message_id: legacy.message_id,
        thread_id: legacy.thread_id,
        bucket_key: legacy.bucket_key,
    })
}

pub fn reply_target_ref_for_target(
    account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    message_id: Option<&str>,
) -> Option<String> {
    let v1 = TelegramReplyTargetRefV1 {
        v: 1,
        channel_type: "telegram".to_string(),
        account_key: account_key.to_string(),
        chat_id: chat_id.to_string(),
        thread_id: thread_id.map(str::to_string),
        message_id: message_id.map(str::to_string),
    };
    serde_json::to_string(&v1).ok()
}

pub fn reply_target_ref_from_inbound_target(
    target: &moltis_channels::ChannelReplyTarget,
) -> Option<String> {
    if target.chan_type != moltis_channels::ChannelType::Telegram {
        return None;
    }
    reply_target_ref_for_target(
        &target.chan_account_key,
        &target.chat_id,
        target.thread_id.as_deref(),
        target.message_id.as_deref(),
    )
}

pub fn inbound_target_from_reply_target_ref(
    reply_target_ref: &str,
) -> Option<moltis_channels::ChannelReplyTarget> {
    let v1: TelegramReplyTargetRefV1 = serde_json::from_str(reply_target_ref).ok()?;
    if v1.v != 1 || v1.channel_type != "telegram" {
        return None;
    }
    Some(moltis_channels::ChannelReplyTarget {
        chan_type: moltis_channels::ChannelType::Telegram,
        chan_account_key: v1.account_key,
        chan_user_name: None,
        chat_id: v1.chat_id,
        message_id: v1.message_id,
        thread_id: v1.thread_id,
        bucket_key: None,
    })
}

pub fn reply_target_ref_from_binding(binding: &str) -> Option<String> {
    let target = telegram_reply_target_from_binding(binding)?;
    reply_target_ref_from_inbound_target(&target)
}

pub fn session_key_from_binding(binding: &str) -> Option<String> {
    let target = telegram_reply_target_from_binding(binding)?;
    target.bucket_key
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannelBindingInfo {
    pub account_key: String,
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub bucket_key: Option<String>,
}

pub fn telegram_channel_binding_info(binding: &str) -> Option<TelegramChannelBindingInfo> {
    let target = telegram_reply_target_from_binding(binding)?;
    Some(TelegramChannelBindingInfo {
        account_key: target.chan_account_key,
        chat_id: target.chat_id,
        thread_id: target.thread_id,
        bucket_key: target.bucket_key,
    })
}

pub fn telegram_binding_is_compatible_for_bucket(
    binding: &str,
    expected: &TelegramChannelBindingInfo,
    bucket_key: &str,
) -> bool {
    let Some(info) = telegram_channel_binding_info(binding) else {
        return false;
    };
    if info.account_key != expected.account_key
        || info.chat_id != expected.chat_id
        || info.thread_id != expected.thread_id
    {
        return false;
    }
    info.bucket_key
        .as_deref()
        .is_none_or(|existing| existing == bucket_key)
}

pub fn telegram_binding_json_for_bucket(
    account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    bucket_key: Option<&str>,
) -> Option<String> {
    let target = moltis_channels::ChannelReplyTarget {
        chan_type: moltis_channels::ChannelType::Telegram,
        chan_account_key: account_key.to_string(),
        chan_user_name: None,
        chat_id: chat_id.to_string(),
        message_id: None,
        thread_id: thread_id.map(str::to_string),
        bucket_key: bucket_key.map(str::to_string),
    };
    serde_json::to_string(&target).ok()
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
    let _ = snapshots;
    let output = format!("{source_username}(bot) -> you: {}", task_text.trim());
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

    fn legacy_binding_json(account_field: &str, bucket_key: Option<&str>) -> String {
        let bucket = bucket_key
            .map(|value| format!(r#","bucket_key":"{value}""#))
            .unwrap_or_default();
        format!(
            r#"{{"channel_type":"telegram","{account_field}":"telegram:test","chat_id":"-1001","message_id":"99","thread_id":"7"{bucket}}}"#
        )
    }

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

    #[test]
    fn binding_helpers_accept_legacy_account_handle_shape() {
        let binding = legacy_binding_json(
            "account_handle",
            Some("group:account:telegram:test:peer:-1001:branch:7"),
        );

        let info = telegram_channel_binding_info(&binding).expect("legacy binding info");
        assert_eq!(info.account_key, "telegram:test");
        assert_eq!(info.chat_id, "-1001");
        assert_eq!(info.thread_id.as_deref(), Some("7"));
        assert_eq!(
            info.bucket_key.as_deref(),
            Some("group:account:telegram:test:peer:-1001:branch:7")
        );

        let reply_target_ref =
            reply_target_ref_from_binding(&binding).expect("reply_target_ref from legacy binding");
        let inbound = inbound_target_from_reply_target_ref(&reply_target_ref)
            .expect("decode reply_target_ref");
        assert_eq!(inbound.chan_account_key, "telegram:test");
        assert_eq!(inbound.chat_id, "-1001");
        assert_eq!(inbound.message_id.as_deref(), Some("99"));
        assert_eq!(inbound.thread_id.as_deref(), Some("7"));

        let channel_target =
            channel_target_from_binding(&binding).expect("channel_target from legacy binding");
        assert_eq!(channel_target.channel_type, "telegram");
        assert_eq!(channel_target.account_key, "telegram:test");
        assert_eq!(channel_target.chat_id, "-1001");
        assert_eq!(channel_target.thread_id.as_deref(), Some("7"));

        assert_eq!(
            session_key_from_binding(&binding).as_deref(),
            Some("group:account:telegram:test:peer:-1001:branch:7")
        );
        assert_eq!(
            tg_gst_v1_system_prompt_block_for_binding(&binding, &[]),
            Some(TG_GST_V1_SYSTEM_PROMPT_BLOCK)
        );
    }

    #[test]
    fn binding_helpers_accept_legacy_account_id_shape() {
        let binding = legacy_binding_json("account_id", None);

        let info = telegram_channel_binding_info(&binding).expect("legacy binding info");
        assert_eq!(info.account_key, "telegram:test");
        assert_eq!(info.chat_id, "-1001");
        assert_eq!(info.thread_id.as_deref(), Some("7"));
        assert!(info.bucket_key.is_none());

        let reply_target_ref =
            reply_target_ref_from_binding(&binding).expect("reply_target_ref from legacy binding");
        let inbound = inbound_target_from_reply_target_ref(&reply_target_ref)
            .expect("decode reply_target_ref");
        assert_eq!(inbound.chan_account_key, "telegram:test");
        assert_eq!(inbound.chat_id, "-1001");
        assert_eq!(inbound.message_id.as_deref(), Some("99"));
        assert_eq!(inbound.thread_id.as_deref(), Some("7"));
        assert!(session_key_from_binding(&binding).is_none());
    }
}

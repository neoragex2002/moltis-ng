use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::config::{
    DmScope, GroupScope, TelegramAccountConfig, TelegramBusAccountSnapshot, TelegramIdentityLink,
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
  - Do NOT start a line with "@someone" unless you intentionally want to delegate (this may trigger Telegram dispatch).
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

fn tg_gst_v1_normalize_display_name(name: &str) -> String {
    let normalized = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_chars = 64usize;
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized.chars().take(max_chars).collect()
}

fn tg_gst_v1_display_value(value: Option<&str>) -> Option<String> {
    value
        .map(tg_gst_v1_normalize_display_name)
        .filter(|value| !value.is_empty())
}

fn tg_gst_v1_normalize_username(username: &str) -> String {
    username.trim().trim_start_matches('@').to_ascii_lowercase()
}

fn tg_gst_v1_short_id(user_id: Option<u64>) -> String {
    let Some(user_id) = user_id else {
        return "unknown".to_string();
    };
    let digits = user_id.to_string();
    let keep = 5usize.min(digits.len());
    digits[digits.len() - keep..].to_string()
}

fn tg_gst_v1_find_managed_snapshot<'a>(
    snapshots: &'a [TelegramBusAccountSnapshot],
    user_id: Option<u64>,
    username: Option<&str>,
) -> Option<&'a TelegramBusAccountSnapshot> {
    let username = username.map(tg_gst_v1_normalize_username);
    snapshots.iter().find(|snapshot| {
        snapshot.chan_user_id == user_id
            || username.as_deref().is_some_and(|username| {
                snapshot
                    .chan_user_name
                    .as_deref()
                    .map(tg_gst_v1_normalize_username)
                    .as_deref()
                    == Some(username)
            })
    })
}

fn tg_gst_v1_find_identity_link_by_agent_id<'a>(
    identity_links: &'a [TelegramIdentityLink],
    agent_id: Option<&str>,
) -> Option<&'a TelegramIdentityLink> {
    let agent_id = agent_id?;
    identity_links.iter().find(|link| link.agent_id == agent_id)
}

fn tg_gst_v1_find_identity_link<'a>(
    identity_links: &'a [TelegramIdentityLink],
    user_id: Option<u64>,
    username: Option<&str>,
) -> Option<(&'a TelegramIdentityLink, &'static str)> {
    if let Some(user_id) = user_id
        && let Some(link) = identity_links
            .iter()
            .find(|link| link.telegram_user_id == Some(user_id))
    {
        return Some((link, "identity_link_user_id"));
    }
    let username = username.map(tg_gst_v1_normalize_username)?;
    identity_links
        .iter()
        .find(|link| {
            link.telegram_user_name
                .as_deref()
                .map(tg_gst_v1_normalize_username)
                .as_deref()
                == Some(username.as_str())
        })
        .map(|link| (link, "identity_link_user_name"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TgGstV1RenderedText {
    pub text: String,
    pub match_method: &'static str,
    pub reason_code: &'static str,
    pub degraded: bool,
    pub disambiguated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TgSpeakerResolution {
    speaker: String,
    actor_key: Option<String>,
    username_hint: Option<String>,
    user_id: Option<u64>,
    match_method: &'static str,
    reason_code: &'static str,
    degraded: bool,
}

fn tg_gst_v1_actor_key_from_snapshot(snapshot: &TelegramBusAccountSnapshot) -> String {
    if let Some(user_id) = snapshot.chan_user_id {
        return format!("uid:{user_id}");
    }
    if let Some(username) = snapshot.chan_user_name.as_deref() {
        return format!("uname:{}", tg_gst_v1_normalize_username(username));
    }
    format!("acct:{}", snapshot.account_handle)
}

fn tg_gst_v1_actor_key_from_identity_link(link: &TelegramIdentityLink) -> Option<String> {
    if let Some(user_id) = link.telegram_user_id {
        return Some(format!("uid:{user_id}"));
    }
    link.telegram_user_name
        .as_deref()
        .map(tg_gst_v1_normalize_username)
        .map(|username| format!("uname:{username}"))
}

fn tg_gst_v1_known_speaker_for_snapshot(
    snapshot: &TelegramBusAccountSnapshot,
    identity_links: &[TelegramIdentityLink],
) -> String {
    let identity_link =
        tg_gst_v1_find_identity_link_by_agent_id(identity_links, snapshot.agent_id.as_deref());
    if let Some(display_name) =
        identity_link.and_then(|link| tg_gst_v1_display_value(link.display_name.as_deref()))
    {
        return format!("{display_name}(bot)");
    }
    if let Some(chan_nickname) = tg_gst_v1_display_value(snapshot.chan_nickname.as_deref()) {
        return format!("{chan_nickname}(bot)");
    }
    if let Some(link_display) = identity_link
        .and_then(|link| tg_gst_v1_display_value(link.telegram_display_name.as_deref()))
    {
        return format!("{link_display}(bot)");
    }
    if let Some(username) = tg_gst_v1_display_value(snapshot.chan_user_name.as_deref()) {
        return format!("{username}(bot)");
    }
    format!("tg-bot-{}(bot)", tg_gst_v1_short_id(snapshot.chan_user_id))
}

fn tg_gst_v1_format_speaker_name(base_name: String, sender_is_bot: bool) -> String {
    if sender_is_bot {
        format!("{base_name}(bot)")
    } else {
        base_name
    }
}

fn tg_gst_v1_known_speaker_for_identity_link(
    link: &TelegramIdentityLink,
    sender_is_bot: bool,
) -> Option<String> {
    if let Some(display_name) = tg_gst_v1_display_value(link.display_name.as_deref()) {
        return Some(tg_gst_v1_format_speaker_name(display_name, sender_is_bot));
    }
    if let Some(link_display) = tg_gst_v1_display_value(link.telegram_display_name.as_deref()) {
        return Some(tg_gst_v1_format_speaker_name(link_display, sender_is_bot));
    }
    tg_gst_v1_display_value(link.telegram_user_name.as_deref())
        .map(|username| tg_gst_v1_format_speaker_name(username, sender_is_bot))
}

fn tg_gst_v1_has_speaker_collision(
    speaker: &str,
    actor_key: Option<&str>,
    sender_is_bot: bool,
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[TelegramIdentityLink],
) -> bool {
    let Some(actor_key) = actor_key else {
        return false;
    };

    if managed_snapshots.iter().any(|snapshot| {
        tg_gst_v1_actor_key_from_snapshot(snapshot) != actor_key
            && tg_gst_v1_known_speaker_for_snapshot(snapshot, identity_links) == speaker
    }) {
        return true;
    }

    identity_links.iter().any(|link| {
        tg_gst_v1_actor_key_from_identity_link(link)
            .filter(|key| key != actor_key)
            .and_then(|_| tg_gst_v1_known_speaker_for_identity_link(link, sender_is_bot))
            .is_some_and(|candidate| candidate == speaker)
    })
}

fn tg_gst_v1_apply_speaker_disambiguation(
    speaker: &str,
    actor_key: Option<&str>,
    username_hint: Option<&str>,
    user_id: Option<u64>,
    sender_is_bot: bool,
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[TelegramIdentityLink],
) -> String {
    if !tg_gst_v1_has_speaker_collision(
        speaker,
        actor_key,
        sender_is_bot,
        managed_snapshots,
        identity_links,
    ) {
        return speaker.to_string();
    }

    if let Some(username) = username_hint
        .map(tg_gst_v1_normalize_username)
        .filter(|value| !value.is_empty())
    {
        return format!("{speaker}[{username}]");
    }

    format!("{speaker}[{}]", tg_gst_v1_short_id(user_id))
}

fn tg_gst_v1_resolve_speaker(
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[TelegramIdentityLink],
    username: Option<&str>,
    user_id: Option<u64>,
    sender_name: Option<&str>,
    sender_is_bot: bool,
) -> TgSpeakerResolution {
    let managed_snapshot = if sender_is_bot {
        tg_gst_v1_find_managed_snapshot(managed_snapshots, user_id, username)
    } else {
        None
    };

    if let Some(snapshot) = managed_snapshot {
        let identity_link =
            tg_gst_v1_find_identity_link_by_agent_id(identity_links, snapshot.agent_id.as_deref());
        if let Some(display_name) =
            identity_link.and_then(|link| tg_gst_v1_display_value(link.display_name.as_deref()))
        {
            return TgSpeakerResolution {
                speaker: format!("{display_name}(bot)"),
                actor_key: Some(tg_gst_v1_actor_key_from_snapshot(snapshot)),
                username_hint: snapshot
                    .chan_user_name
                    .clone()
                    .or_else(|| username.map(str::to_string)),
                user_id,
                match_method: "managed_bot_agent_id",
                reason_code: "speaker_link_display_name",
                degraded: false,
            };
        }

        if let Some(chan_nickname) = tg_gst_v1_display_value(snapshot.chan_nickname.as_deref()) {
            return TgSpeakerResolution {
                speaker: format!("{chan_nickname}(bot)"),
                actor_key: Some(tg_gst_v1_actor_key_from_snapshot(snapshot)),
                username_hint: snapshot
                    .chan_user_name
                    .clone()
                    .or_else(|| username.map(str::to_string)),
                user_id,
                match_method: "managed_bot_chan_nickname",
                reason_code: "speaker_managed_bot_nickname",
                degraded: false,
            };
        }

        if let Some(link_display) = identity_link
            .and_then(|link| tg_gst_v1_display_value(link.telegram_display_name.as_deref()))
        {
            return TgSpeakerResolution {
                speaker: format!("{link_display}(bot)"),
                actor_key: Some(tg_gst_v1_actor_key_from_snapshot(snapshot)),
                username_hint: snapshot
                    .chan_user_name
                    .clone()
                    .or_else(|| username.map(str::to_string)),
                user_id,
                match_method: "managed_bot_link_telegram_display_name",
                reason_code: "speaker_link_telegram_display_name",
                degraded: false,
            };
        }
    }

    if let Some((identity_link, match_method)) =
        tg_gst_v1_find_identity_link(identity_links, user_id, username)
    {
        if let Some(display_name) = tg_gst_v1_display_value(identity_link.display_name.as_deref()) {
            return TgSpeakerResolution {
                speaker: tg_gst_v1_format_speaker_name(display_name, sender_is_bot),
                actor_key: tg_gst_v1_actor_key_from_identity_link(identity_link),
                username_hint: username
                    .map(str::to_string)
                    .or_else(|| identity_link.telegram_user_name.clone()),
                user_id,
                match_method,
                reason_code: "speaker_link_display_name",
                degraded: false,
            };
        }
        if let Some(link_display) =
            tg_gst_v1_display_value(identity_link.telegram_display_name.as_deref())
        {
            return TgSpeakerResolution {
                speaker: tg_gst_v1_format_speaker_name(link_display, sender_is_bot),
                actor_key: tg_gst_v1_actor_key_from_identity_link(identity_link),
                username_hint: username
                    .map(str::to_string)
                    .or_else(|| identity_link.telegram_user_name.clone()),
                user_id,
                match_method,
                reason_code: "speaker_link_telegram_display_name",
                degraded: false,
            };
        }
    }

    if let Some(sender_name) = tg_gst_v1_display_value(sender_name) {
        return TgSpeakerResolution {
            speaker: tg_gst_v1_format_speaker_name(sender_name, sender_is_bot),
            actor_key: user_id.map(|value| format!("uid:{value}")).or_else(|| {
                username
                    .map(tg_gst_v1_normalize_username)
                    .map(|value| format!("uname:{value}"))
            }),
            username_hint: username.map(str::to_string),
            user_id,
            match_method: "telegram_sender_name",
            reason_code: "speaker_sender_name",
            degraded: false,
        };
    }

    if let Some(username) = tg_gst_v1_display_value(username) {
        let username_hint = username.clone();
        let speaker = tg_gst_v1_format_speaker_name(username.clone(), sender_is_bot);
        return TgSpeakerResolution {
            speaker,
            actor_key: user_id
                .map(|value| format!("uid:{value}"))
                .or_else(|| Some(format!("uname:{}", tg_gst_v1_normalize_username(&username)))),
            username_hint: Some(username_hint),
            user_id,
            match_method: "telegram_username",
            reason_code: "speaker_username",
            degraded: false,
        };
    }

    let prefix = if sender_is_bot {
        "tg-bot"
    } else {
        "tg-user"
    };
    TgSpeakerResolution {
        speaker: format!(
            "{prefix}-{}{}",
            tg_gst_v1_short_id(user_id),
            if sender_is_bot {
                "(bot)"
            } else {
                ""
            }
        ),
        actor_key: user_id.map(|value| format!("uid:{value}")),
        username_hint: None,
        user_id,
        match_method: "technical_short_id",
        reason_code: "speaker_technical_short_id",
        degraded: true,
    }
}

pub(crate) fn tg_gst_v1_render_text(
    body: &str,
    message_kind: Option<moltis_channels::ChannelMessageKind>,
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[TelegramIdentityLink],
    username: Option<&str>,
    sender_id: Option<u64>,
    sender_name: Option<&str>,
    sender_is_bot: bool,
    addressed: bool,
) -> TgGstV1RenderedText {
    let body = tg_gst_v1_apply_media_placeholder(message_kind, body);
    if body.trim().is_empty() {
        return TgGstV1RenderedText {
            text: body,
            match_method: "empty_body",
            reason_code: "empty_body",
            degraded: false,
            disambiguated: false,
        };
    }
    let resolution = tg_gst_v1_resolve_speaker(
        managed_snapshots,
        identity_links,
        username,
        sender_id,
        sender_name,
        sender_is_bot,
    );
    let speaker = tg_gst_v1_apply_speaker_disambiguation(
        &resolution.speaker,
        resolution.actor_key.as_deref(),
        resolution.username_hint.as_deref(),
        resolution.user_id,
        sender_is_bot,
        managed_snapshots,
        identity_links,
    );
    let disambiguated = speaker != resolution.speaker;
    let addr_flag = if addressed {
        " -> you"
    } else {
        ""
    };
    TgGstV1RenderedText {
        text: format!("{speaker}{addr_flag}: {body}"),
        match_method: resolution.match_method,
        reason_code: resolution.reason_code,
        degraded: resolution.degraded,
        disambiguated,
    }
}

pub fn tg_gst_v1_format_inbound_text(
    body: &str,
    message_kind: Option<moltis_channels::ChannelMessageKind>,
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[TelegramIdentityLink],
    username: Option<&str>,
    sender_id: Option<u64>,
    sender_name: Option<&str>,
    sender_is_bot: bool,
    addressed: bool,
) -> String {
    tg_gst_v1_render_text(
        body,
        message_kind,
        managed_snapshots,
        identity_links,
        username,
        sender_id,
        sender_name,
        sender_is_bot,
        addressed,
    )
    .text
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgGroupTargetAction {
    pub mode: TgInboundMode,
    pub body: String,
    pub addressed: bool,
    pub reason_code: &'static str,
}

#[derive(Debug, Clone)]
struct TgLineStartMention {
    account_handle: String,
}

#[derive(Debug, Clone)]
struct TgLineStartMentionGroup {
    task_text: String,
    mentions: Vec<TgLineStartMention>,
}

#[derive(Debug, Clone)]
struct MentionMatch {
    username: String,
    start: usize,
    end: usize,
}

fn sanitize_for_group_dispatch_scan(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;

    for line in input.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.starts_with('>') {
            continue;
        }

        let mut in_inline = false;
        for ch in line.chars() {
            if ch == '`' {
                in_inline = !in_inline;
                continue;
            }
            if !in_inline {
                out.push(ch);
            }
        }
        out.push('\n');
    }

    out
}

fn is_non_bot_broadcast_mention(username: &str) -> bool {
    matches!(username, "all" | "here" | "everyone")
}

fn find_mentions(input: &str) -> Vec<MentionMatch> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        while j < bytes.len() {
            let b = bytes[j];
            let ok = b.is_ascii_digit() || b.is_ascii_alphabetic() || b == b'_';
            if !ok {
                break;
            }
            j += 1;
        }
        let username = &input[start + 1..j];
        if (1..=32).contains(&username.len()) {
            out.push(MentionMatch {
                username: username.to_ascii_lowercase(),
                start,
                end: j,
            });
        }
        i = j;
    }
    out
}

fn only_whitespace_or_punct(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '，' | '、' | ';' | '；' | ':' | '：' | '!' | '！' | '?' | '？' | '。'
            )
    })
}

fn line_start_for_index(text: &str, idx: usize) -> usize {
    text[..idx.min(text.len())]
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0)
}

fn is_line_start_token(text: &str, idx: usize) -> bool {
    let line_start = line_start_for_index(text, idx);
    text[line_start..idx.min(text.len())]
        .chars()
        .all(|c| c.is_whitespace())
}

fn trim_leading_separators(s: &str) -> &str {
    s.trim_start_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '，' | '、' | ';' | '；' | ':' | '：' | '!' | '！' | '?' | '？' | '。'
            )
    })
}

fn trim_trailing_connectors(s: &str) -> &str {
    let mut value = s.trim_end_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '，' | '、' | ';' | '；' | ':' | '：' | '!' | '！' | '?' | '？' | '。'
            )
    });

    for kw in ["请你", "请", "麻烦", "让", "帮"] {
        if value.ends_with(kw) {
            value = value.trim_end_matches(kw).trim_end();
            value = value.trim_end_matches(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        ',' | '，'
                            | '、'
                            | ';'
                            | '；'
                            | ':'
                            | '：'
                            | '!'
                            | '！'
                            | '?'
                            | '？'
                            | '。'
                    )
            });
        }
    }

    value
}

fn extract_line_start_mention_groups(
    body: &str,
    accounts: &[TelegramBusAccountSnapshot],
) -> Vec<TgLineStartMentionGroup> {
    let sanitized = sanitize_for_group_dispatch_scan(body);
    let mentions = find_mentions(&sanitized);
    if mentions.is_empty() {
        return Vec::new();
    }

    let mut username_to_account = HashMap::<String, String>::new();
    for account in accounts {
        if let Some(username) = account.chan_user_name.as_deref() {
            username_to_account.insert(
                username.to_ascii_lowercase(),
                account.account_handle.clone(),
            );
        }
    }

    let mut groups = Vec::new();
    let mut i = 0;
    while i < mentions.len() {
        let group_start = mentions[i].start;
        let mut group_end = mentions[i].end;
        let mut group_mentions = vec![mentions[i].clone()];
        let mut j = i + 1;
        while j < mentions.len()
            && only_whitespace_or_punct(&sanitized[group_end..mentions[j].start])
        {
            group_mentions.push(mentions[j].clone());
            group_end = mentions[j].end;
            j += 1;
        }

        let seg_end = if j < mentions.len() {
            mentions[j].start
        } else {
            sanitized.len()
        };
        if !is_line_start_token(&sanitized, group_start) {
            i = j;
            continue;
        }

        let raw_task = trim_trailing_connectors(&sanitized[group_end..seg_end]);
        let task_text = trim_leading_separators(raw_task).trim().to_string();

        let mut resolved = Vec::new();
        let mut seen = HashSet::new();
        for mention in group_mentions {
            if is_non_bot_broadcast_mention(&mention.username) {
                continue;
            }
            let Some(account_handle) = username_to_account.get(&mention.username) else {
                continue;
            };
            if seen.insert(account_handle.clone()) {
                resolved.push(TgLineStartMention {
                    account_handle: account_handle.clone(),
                });
            }
        }
        if !resolved.is_empty() {
            groups.push(TgLineStartMentionGroup {
                task_text,
                mentions: resolved,
            });
        }

        i = j;
    }

    groups
}

pub fn plan_group_target_action(
    body: &str,
    accounts: &[TelegramBusAccountSnapshot],
    target_account_handle: &str,
    target_username: Option<&str>,
    reply_to_target_account_handle: Option<&str>,
    line_start_mention_dispatch: bool,
    reply_to_dispatch: bool,
    has_non_text_content: bool,
) -> Option<TgGroupTargetAction> {
    let body = body.trim();
    if body.is_empty() && !has_non_text_content {
        return None;
    }

    let mention_groups = extract_line_start_mention_groups(body, accounts);
    let any_line_start_target = !mention_groups.is_empty();
    let mut line_start_targeted = false;
    let mut line_start_has_task = false;
    for group in &mention_groups {
        if group
            .mentions
            .iter()
            .any(|mention| mention.account_handle == target_account_handle)
        {
            line_start_targeted = true;
            line_start_has_task |= !group.task_text.is_empty();
        }
    }

    if line_start_targeted {
        if line_start_mention_dispatch && line_start_has_task {
            return Some(TgGroupTargetAction {
                mode: TgInboundMode::Dispatch,
                body: body.to_string(),
                addressed: true,
                reason_code: "tg_dispatch_line_start_mention",
            });
        }
        return Some(TgGroupTargetAction {
            mode: TgInboundMode::RecordOnly,
            body: body.to_string(),
            addressed: line_start_mention_dispatch,
            reason_code: if line_start_mention_dispatch {
                "tg_record_presence_ping"
            } else {
                "tg_record_context"
            },
        });
    }

    let reply_to_matches = reply_to_target_account_handle == Some(target_account_handle);
    if reply_to_matches && !any_line_start_target && reply_to_dispatch {
        return Some(TgGroupTargetAction {
            mode: TgInboundMode::Dispatch,
            body: body.to_string(),
            addressed: true,
            reason_code: "tg_dispatch_reply_to_bot",
        });
    }

    let _ = target_username;
    Some(TgGroupTargetAction {
        mode: TgInboundMode::RecordOnly,
        body: body.to_string(),
        addressed: false,
        reason_code: "tg_record_context",
    })
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lineage_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TelegramOutboundTargetRef {
    pub target: moltis_channels::ChannelReplyTarget,
    pub lineage_message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyTelegramChannelBinding {
    channel_type: String,
    #[serde(default)]
    account_handle: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

fn telegram_reply_target_from_binding(
    binding: &str,
) -> Option<moltis_channels::ChannelReplyTarget> {
    let target = serde_json::from_str::<moltis_channels::ChannelReplyTarget>(binding).ok()?;
    (target.chan_type == moltis_channels::ChannelType::Telegram).then_some(target)
}

pub fn telegram_binding_uses_legacy_shape(binding: &str) -> bool {
    if telegram_reply_target_from_binding(binding).is_some() {
        return false;
    }

    serde_json::from_str::<LegacyTelegramChannelBinding>(binding)
        .ok()
        .is_some_and(|legacy| {
            legacy.channel_type == "telegram"
                && (legacy.account_handle.is_some() || legacy.account_id.is_some())
        })
}

pub fn reply_target_ref_for_target_with_lineage(
    account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    message_id: Option<&str>,
    lineage_message_id: Option<&str>,
) -> Option<String> {
    let v1 = TelegramReplyTargetRefV1 {
        v: 1,
        channel_type: "telegram".to_string(),
        account_key: account_key.to_string(),
        chat_id: chat_id.to_string(),
        thread_id: thread_id.map(str::to_string),
        message_id: message_id.map(str::to_string),
        lineage_message_id: lineage_message_id.map(str::to_string),
    };
    serde_json::to_string(&v1).ok()
}

pub fn reply_target_ref_for_target(
    account_key: &str,
    chat_id: &str,
    thread_id: Option<&str>,
    message_id: Option<&str>,
) -> Option<String> {
    reply_target_ref_for_target_with_lineage(
        account_key,
        chat_id,
        thread_id,
        message_id,
        message_id,
    )
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
    telegram_outbound_target_from_reply_target_ref(reply_target_ref).map(|parsed| parsed.target)
}

pub fn telegram_outbound_target_from_reply_target_ref(
    reply_target_ref: &str,
) -> Option<TelegramOutboundTargetRef> {
    let v1: TelegramReplyTargetRefV1 = serde_json::from_str(reply_target_ref).ok()?;
    if v1.v != 1 || v1.channel_type != "telegram" {
        return None;
    }
    Some(TelegramOutboundTargetRef {
        lineage_message_id: v1.lineage_message_id.or(v1.message_id.clone()),
        target: moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: v1.account_key,
            chan_user_name: None,
            chat_id: v1.chat_id,
            message_id: v1.message_id,
            thread_id: v1.thread_id,
            bucket_key: None,
        },
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
    info.bucket_key.as_deref() == Some(bucket_key)
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

pub fn resolve_group_bucket_route(
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
    use crate::config::TelegramIdentityLink;

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
    fn binding_helpers_reject_legacy_account_handle_shape() {
        let binding = legacy_binding_json(
            "account_handle",
            Some("group:account:telegram:test:peer:-1001:branch:7"),
        );
        assert!(telegram_binding_uses_legacy_shape(&binding));
        assert!(telegram_channel_binding_info(&binding).is_none());
        assert!(reply_target_ref_from_binding(&binding).is_none());
        assert!(channel_target_from_binding(&binding).is_none());
        assert!(session_key_from_binding(&binding).is_none());
        assert_eq!(
            tg_gst_v1_system_prompt_block_for_binding(&binding, &[]),
            None
        );
    }

    #[test]
    fn binding_helpers_reject_legacy_account_id_shape() {
        let binding = legacy_binding_json("account_id", None);

        assert!(telegram_binding_uses_legacy_shape(&binding));
        assert!(telegram_channel_binding_info(&binding).is_none());
        assert!(reply_target_ref_from_binding(&binding).is_none());
        assert!(channel_target_from_binding(&binding).is_none());
        assert!(session_key_from_binding(&binding).is_none());
    }

    #[test]
    fn binding_without_bucket_key_is_not_compatible_for_bucket() {
        let binding = serde_json::to_string(&moltis_channels::ChannelReplyTarget {
            chan_type: moltis_channels::ChannelType::Telegram,
            chan_account_key: "telegram:test".into(),
            chan_user_name: None,
            chat_id: "-1001".into(),
            message_id: Some("99".into()),
            thread_id: Some("7".into()),
            bucket_key: None,
        })
        .expect("serialize binding");
        let expected = TelegramChannelBindingInfo {
            account_key: "telegram:test".into(),
            chat_id: "-1001".into(),
            thread_id: Some("7".into()),
            bucket_key: Some("group:account:telegram:test:peer:-1001:branch:7".into()),
        };

        assert!(!telegram_binding_is_compatible_for_bucket(
            &binding,
            &expected,
            "group:account:telegram:test:peer:-1001:branch:7"
        ));
    }

    fn snapshot(account_handle: &str, username: &str) -> TelegramBusAccountSnapshot {
        TelegramBusAccountSnapshot {
            account_handle: account_handle.to_string(),
            agent_id: None,
            chan_user_id: None,
            chan_user_name: Some(username.to_string()),
            chan_nickname: None,
            dm_scope: DmScope::Main,
            group_scope: GroupScope::Group,
        }
    }

    fn identity_link(
        agent_id: &str,
        display_name: Option<&str>,
        telegram_user_id: Option<u64>,
        telegram_user_name: Option<&str>,
        telegram_display_name: Option<&str>,
    ) -> TelegramIdentityLink {
        TelegramIdentityLink {
            agent_id: agent_id.to_string(),
            display_name: display_name.map(str::to_string),
            telegram_user_id,
            telegram_user_name: telegram_user_name.map(str::to_string),
            telegram_display_name: telegram_display_name.map(str::to_string),
        }
    }

    #[test]
    fn group_target_plan_merges_line_start_tasks_once_per_target() {
        let body = "@bot_a 做A\n\n@bot_b 做B\n\n@bot_a 补A2";
        let plan = plan_group_target_action(
            body,
            &[
                snapshot("telegram:bot_a", "bot_a"),
                snapshot("telegram:bot_b", "bot_b"),
            ],
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::Dispatch);
        assert_eq!(plan.reason_code, "tg_dispatch_line_start_mention");
        assert_eq!(plan.body, body);
        assert!(plan.addressed);
    }

    #[test]
    fn group_target_plan_prefers_line_start_target_over_reply_to_other_bot() {
        let accounts = [
            snapshot("telegram:bot_a", "bot_a"),
            snapshot("telegram:bot_b", "bot_b"),
        ];
        let body = "@bot_b 你处理这件事";

        let bot_a = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_a",
            Some("bot_a"),
            Some("telegram:bot_a"),
            true,
            true,
            false,
        )
        .expect("plan");
        assert_eq!(bot_a.mode, TgInboundMode::RecordOnly);
        assert_eq!(bot_a.reason_code, "tg_record_context");
        assert!(!bot_a.addressed);

        let bot_b = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_b",
            Some("bot_b"),
            Some("telegram:bot_a"),
            true,
            true,
            false,
        )
        .expect("plan");
        assert_eq!(bot_b.mode, TgInboundMode::Dispatch);
        assert_eq!(bot_b.body, body);
    }

    #[test]
    fn group_target_plan_records_when_dispatch_policies_are_disabled() {
        let body = "@bot_a 处理一下";
        let plan = plan_group_target_action(
            body,
            &[snapshot("telegram:bot_a", "bot_a")],
            "telegram:bot_a",
            Some("bot_a"),
            Some("telegram:bot_a"),
            false,
            false,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::RecordOnly);
        assert_eq!(plan.reason_code, "tg_record_context");
        assert_eq!(plan.body, body);
        assert!(!plan.addressed);
    }

    #[test]
    fn group_target_plan_marks_presence_ping_as_addressed_record() {
        let body = "@bot_a";
        let plan = plan_group_target_action(
            body,
            &[snapshot("telegram:bot_a", "bot_a")],
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::RecordOnly);
        assert_eq!(plan.reason_code, "tg_record_presence_ping");
        assert_eq!(plan.body, body);
        assert!(plan.addressed);
    }

    #[test]
    fn group_target_plan_keeps_target_inside_multi_mention_line_start_cluster() {
        let body = "@a @bot_a @c do X";
        let plan = plan_group_target_action(
            body,
            &[snapshot("telegram:bot_a", "bot_a")],
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::Dispatch);
        assert_eq!(plan.reason_code, "tg_dispatch_line_start_mention");
        assert_eq!(plan.body, body);
        assert!(plan.addressed);
    }

    #[test]
    fn group_target_plan_keeps_full_body_for_bad_example_context() {
        let body = "@cute_alma_bot 我用自己的话复述 + 例子如下：\n\n我会刻意避免的错误写法（示例）\n@cute_alma_bot @lovely_apple_bot 我先说下：我做了一半，等会再补。\n（问题：一条消息正式唤醒了两个人。）";
        let plan = plan_group_target_action(
            body,
            &[
                snapshot("telegram:cute_alma_bot", "cute_alma_bot"),
                snapshot("telegram:lovely_apple_bot", "lovely_apple_bot"),
            ],
            "telegram:lovely_apple_bot",
            Some("lovely_apple_bot"),
            None,
            true,
            true,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::Dispatch);
        assert_eq!(plan.reason_code, "tg_dispatch_line_start_mention");
        assert_eq!(plan.body, body);
        assert!(plan.addressed);
    }

    #[test]
    fn group_target_plan_keeps_same_full_body_for_multiple_targets() {
        let body = "@bot_a 你负责日志\n@bot_b 你负责配置\n下面是统一背景、边界和注意事项...";
        let accounts = [
            snapshot("telegram:bot_a", "bot_a"),
            snapshot("telegram:bot_b", "bot_b"),
        ];

        let plan_a = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan a");
        let plan_b = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_b",
            Some("bot_b"),
            None,
            true,
            true,
            false,
        )
        .expect("plan b");

        assert_eq!(plan_a.mode, TgInboundMode::Dispatch);
        assert_eq!(plan_b.mode, TgInboundMode::Dispatch);
        assert_eq!(plan_a.body, body);
        assert_eq!(plan_b.body, body);
        assert!(plan_a.addressed);
        assert!(plan_b.addressed);
    }

    #[test]
    fn group_target_plan_only_trims_outer_whitespace_and_keeps_internal_newlines() {
        let raw_body = "\n\n  @bot_a 第一段\n\n第二段保留\n  ";
        let expected_body = "@bot_a 第一段\n\n第二段保留";
        let plan = plan_group_target_action(
            raw_body,
            &[snapshot("telegram:bot_a", "bot_a")],
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan");

        assert_eq!(plan.mode, TgInboundMode::Dispatch);
        assert_eq!(plan.body, expected_body);
    }

    #[test]
    fn group_target_plan_ignores_quote_and_code_mentions_for_matching_but_keeps_full_body() {
        let body = "> @bot_a 这是引用示例\n`@bot_a` 这是行内代码\n```text\n@bot_a 这是代码块\n```\n@bot_b 处理正式任务\n补充说明保留在正文里";
        let accounts = [
            snapshot("telegram:bot_a", "bot_a"),
            snapshot("telegram:bot_b", "bot_b"),
        ];

        let plan_a = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_a",
            Some("bot_a"),
            None,
            true,
            true,
            false,
        )
        .expect("plan a");
        let plan_b = plan_group_target_action(
            body,
            &accounts,
            "telegram:bot_b",
            Some("bot_b"),
            None,
            true,
            true,
            false,
        )
        .expect("plan b");

        assert_eq!(plan_a.mode, TgInboundMode::RecordOnly);
        assert_eq!(plan_a.reason_code, "tg_record_context");
        assert!(!plan_a.addressed);
        assert_eq!(plan_a.body, body);

        assert_eq!(plan_b.mode, TgInboundMode::Dispatch);
        assert_eq!(plan_b.reason_code, "tg_dispatch_line_start_mention");
        assert!(plan_b.addressed);
        assert_eq!(plan_b.body, body);
    }

    #[test]
    fn tg_gst_v1_render_text_prefers_link_display_name_for_managed_bot() {
        let mut managed = snapshot("telegram:100", "risk_bot_cn");
        managed.agent_id = Some("risk".into());
        managed.chan_user_id = Some(100);
        managed.chan_nickname = Some("风险助手中文".into());

        let rendered = tg_gst_v1_render_text(
            "已处理",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[managed],
            &[identity_link(
                "risk",
                Some("风险助手"),
                Some(100),
                Some("risk_bot_cn"),
                Some("风险助手中文"),
            )],
            Some("risk_bot_cn"),
            Some(100),
            Some("风险助手中文"),
            true,
            true,
        );

        assert_eq!(rendered.text, "风险助手(bot) -> you: 已处理");
        assert_eq!(rendered.match_method, "managed_bot_agent_id");
        assert!(!rendered.degraded);
    }

    #[test]
    fn tg_gst_v1_render_text_uses_chan_nickname_for_unlinked_managed_bot() {
        let mut managed = snapshot("telegram:100", "risk_bot_cn");
        managed.agent_id = Some("risk".into());
        managed.chan_user_id = Some(100);
        managed.chan_nickname = Some("风险助手中文".into());

        let rendered = tg_gst_v1_render_text(
            "已处理",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[managed],
            &[],
            Some("risk_bot_cn"),
            Some(100),
            Some("风险助手中文"),
            true,
            false,
        );

        assert_eq!(rendered.text, "风险助手中文(bot): 已处理");
        assert_eq!(rendered.match_method, "managed_bot_chan_nickname");
        assert!(!rendered.degraded);
    }

    #[test]
    fn tg_gst_v1_render_text_prefers_link_display_name_for_human_sender() {
        let rendered = tg_gst_v1_render_text(
            "hello everyone",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[],
            &[identity_link(
                "alice",
                Some("Alice Zhang"),
                Some(42),
                Some("alice"),
                Some("Alice TG"),
            )],
            Some("alice"),
            Some(42),
            Some("Alice TG"),
            false,
            false,
        );

        assert_eq!(rendered.text, "Alice Zhang: hello everyone");
        assert_eq!(rendered.match_method, "identity_link_user_id");
        assert!(!rendered.degraded);
    }

    #[test]
    fn tg_gst_v1_render_text_uses_short_id_without_internal_key() {
        let rendered = tg_gst_v1_render_text(
            "ping",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[],
            &[],
            None,
            Some(1234567890),
            None,
            true,
            false,
        );

        assert_eq!(rendered.text, "tg-bot-67890(bot): ping");
        assert_eq!(rendered.match_method, "technical_short_id");
        assert!(rendered.degraded);
        assert!(!rendered.text.contains("telegram:"));
        assert!(!rendered.text.contains("tg:1234567890"));
    }

    #[test]
    fn tg_gst_v1_render_text_disambiguates_same_display_name_bots_by_username() {
        let mut bot_a = snapshot("telegram:100", "risk_bot_cn");
        bot_a.agent_id = Some("risk-a".into());
        bot_a.chan_user_id = Some(100);
        bot_a.chan_nickname = Some("风险助手中文".into());

        let mut bot_b = snapshot("telegram:200", "risk_helper_backup");
        bot_b.agent_id = Some("risk-b".into());
        bot_b.chan_user_id = Some(200);
        bot_b.chan_nickname = Some("风险助手中文备用".into());

        let rendered = tg_gst_v1_render_text(
            "已处理",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[bot_a, bot_b],
            &[
                identity_link(
                    "risk-a",
                    Some("风险助手"),
                    Some(100),
                    Some("risk_bot_cn"),
                    Some("风险助手中文"),
                ),
                identity_link(
                    "risk-b",
                    Some("风险助手"),
                    Some(200),
                    Some("risk_helper_backup"),
                    Some("风险助手中文备用"),
                ),
            ],
            Some("risk_bot_cn"),
            Some(100),
            Some("风险助手中文"),
            true,
            false,
        );

        assert_eq!(rendered.text, "风险助手(bot)[risk_bot_cn]: 已处理");
        assert_eq!(rendered.match_method, "managed_bot_agent_id");
        assert!(!rendered.degraded);
        assert!(rendered.disambiguated);
    }

    #[test]
    fn tg_gst_v1_render_text_disambiguates_same_display_name_identity_link_bots_by_username() {
        let rendered = tg_gst_v1_render_text(
            "已处理",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[],
            &[
                identity_link(
                    "risk-a",
                    Some("风险助手"),
                    Some(100),
                    Some("risk_bot_cn"),
                    Some("风险助手中文"),
                ),
                identity_link(
                    "risk-b",
                    Some("风险助手"),
                    Some(200),
                    Some("risk_helper_backup"),
                    Some("风险助手中文备用"),
                ),
            ],
            Some("risk_bot_cn"),
            Some(100),
            Some("风险助手中文"),
            true,
            false,
        );

        assert_eq!(rendered.text, "风险助手(bot)[risk_bot_cn]: 已处理");
        assert_eq!(rendered.match_method, "identity_link_user_id");
        assert!(!rendered.degraded);
        assert!(rendered.disambiguated);
    }

    #[test]
    fn tg_gst_v1_render_text_disambiguates_same_display_name_humans_by_short_id() {
        let rendered = tg_gst_v1_render_text(
            "hello everyone",
            Some(moltis_channels::ChannelMessageKind::Text),
            &[],
            &[
                identity_link(
                    "alice-a",
                    Some("Alice Zhang"),
                    Some(12345),
                    None,
                    Some("Alice A"),
                ),
                identity_link(
                    "alice-b",
                    Some("Alice Zhang"),
                    Some(54321),
                    None,
                    Some("Alice B"),
                ),
            ],
            None,
            Some(12345),
            Some("Alice A"),
            false,
            false,
        );

        assert_eq!(rendered.text, "Alice Zhang[12345]: hello everyone");
        assert_eq!(rendered.match_method, "identity_link_user_id");
        assert!(!rendered.degraded);
        assert!(rendered.disambiguated);
    }
}

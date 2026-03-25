use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex, OnceLock},
};

use {
    teloxide::{
        payloads::SendMessageSetters,
        prelude::*,
        types::{
            CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, MediaKind, MessageEntity,
            MessageEntityKind, MessageEntityRef, MessageKind, ParseMode, UserId,
        },
    },
    tracing::{debug, info, warn},
};

use {
    moltis_channels::{
        ChannelAttachment, ChannelEvent, ChannelMessageKind, ChannelOutbound, ChannelReplyTarget,
        ChannelType, message_log::MessageLogEntry,
    },
    moltis_common::types::ChatType,
};

#[cfg(feature = "metrics")]
use moltis_metrics::{counter, histogram, telegram as tg_metrics};

use crate::{
    access::{self, AccessDenied},
    adapter::{
        TgAttachment, TgContent, TgFollowUpTarget, TgInbound, TgInboundKind, TgInboundMode,
        TgInboundRequest, TgPrivateSource, TgPrivateTarget, TgRoute, TgTranscriptFormat,
        plan_group_target_action, resolve_dm_bucket_key, resolve_group_bucket_key,
        resolve_tg_route,
    },
    config::TelegramBusAccountSnapshot,
    otp::{OtpInitResult, OtpVerifyResult},
    outbound::{TypingRequestError, send_chat_action_typing},
    state::AccountStateMap,
};

#[cfg(not(test))]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);
#[cfg(test)]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(10);

const CALLBACK_BUCKET_BINDING_LIMIT: usize = 4096;

#[derive(Default)]
struct CallbackBucketBindings {
    by_message: HashMap<String, String>,
    order: VecDeque<String>,
}

fn callback_bucket_bindings() -> &'static Mutex<CallbackBucketBindings> {
    static STORE: OnceLock<Mutex<CallbackBucketBindings>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(CallbackBucketBindings::default()))
}

fn callback_bucket_binding_key(account_handle: &str, chat_id: &str, message_id: i32) -> String {
    format!("{account_handle}:{chat_id}:{message_id}")
}

fn remember_callback_bucket_binding(target: &ChannelReplyTarget, message_id: i32) {
    let Some(bucket_key) = target.bucket_key.as_deref() else {
        return;
    };

    let key = callback_bucket_binding_key(&target.chan_account_key, &target.chat_id, message_id);
    let mut bindings = callback_bucket_bindings()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !bindings.by_message.contains_key(&key) {
        bindings.order.push_back(key.clone());
    }
    bindings.by_message.insert(key, bucket_key.to_string());

    while bindings.order.len() > CALLBACK_BUCKET_BINDING_LIMIT {
        if let Some(oldest_key) = bindings.order.pop_front() {
            bindings.by_message.remove(&oldest_key);
        }
    }
}

fn lookup_callback_bucket_binding(
    account_handle: &str,
    chat_id: &str,
    message_id: i32,
) -> Option<String> {
    callback_bucket_bindings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .by_message
        .get(&callback_bucket_binding_key(
            account_handle,
            chat_id,
            message_id,
        ))
        .cloned()
}

fn callback_sender_hint_from_target(target: &ChannelReplyTarget) -> Option<String> {
    target
        .bucket_key
        .as_deref()
        .and_then(|bucket_key| bucket_key.rsplit_once("-sender-"))
        .and_then(|(_, sender)| {
            (sender.starts_with("person.") || sender.starts_with("tguser."))
                .then(|| sender.to_string())
        })
}

fn with_callback_sender_hint(base: String, target: &ChannelReplyTarget) -> String {
    match callback_sender_hint_from_target(target) {
        Some(sender) => format!("{base}|s={sender}"),
        None => base,
    }
}

fn split_callback_sender_hint(data: &str) -> (&str, Option<&str>) {
    if let Some((base, sender_hint)) = data.rsplit_once("|s=")
        && !sender_hint.is_empty()
        && (sender_hint.starts_with("person.") || sender_hint.starts_with("tguser."))
    {
        return (base, Some(sender_hint));
    }

    (data, None)
}

#[derive(Debug, Clone, Copy)]
pub struct RetryableUpdateError {
    pub reason_code: &'static str,
}

impl std::fmt::Display for RetryableUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "retryable update failure ({})", self.reason_code)
    }
}

impl std::error::Error for RetryableUpdateError {}

fn user_facing_error_message() -> &'static str {
    "⚠️ Something went wrong. Please try again."
}

fn user_facing_unsupported_attachment_message() -> &'static str {
    "⚠️ This attachment type isn’t supported yet. Please send text, a photo, a voice message, or a location."
}

fn parse_chat_id(chat_id: &str) -> anyhow::Result<ChatId> {
    let id = chat_id
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("invalid chat_id"))?;
    Ok(ChatId(id))
}

async fn run_with_telegram_typing<T>(
    bot: teloxide::Bot,
    account_handle: &str,
    chat_id: &str,
    op: &'static str,
    operation: impl Future<Output = T>,
) -> T {
    async fn send_typing_once(
        bot: &teloxide::Bot,
        account_handle: &str,
        chat_id: &str,
        op: &'static str,
        typing_failed: &mut bool,
    ) {
        let parsed_chat_id = match parse_chat_id(chat_id) {
            Ok(chat_id) => chat_id,
            Err(_) => {
                if !*typing_failed {
                    warn!(
                        event = "telegram.typing.failed",
                        op,
                        account_handle,
                        chat_id,
                        reason_code = "invalid_chat_id",
                        "failed to start typing indicator"
                    );
                    *typing_failed = true;
                }
                return;
            },
        };

        match send_chat_action_typing(bot, parsed_chat_id, None).await {
            Ok(()) => {
                if *typing_failed {
                    info!(
                        event = "telegram.typing.recovered",
                        op, account_handle, chat_id, "typing indicator recovered"
                    );
                    *typing_failed = false;
                }
            },
            Err(error) => {
                let (reason_code, error_class) = match &error {
                    TypingRequestError::Request(teloxide::RequestError::Api(_)) => {
                        ("send_typing_failed", "api")
                    },
                    TypingRequestError::Request(teloxide::RequestError::Network(err))
                        if err.is_timeout() || err.is_connect() =>
                    {
                        ("transport_failed_before_send", "network")
                    },
                    TypingRequestError::Request(request_error) => {
                        let error_class = match request_error {
                            teloxide::RequestError::RetryAfter(_) => "retry_after",
                            teloxide::RequestError::InvalidJson { .. } => "invalid_json",
                            teloxide::RequestError::Io(_) => "io",
                            teloxide::RequestError::Network(_) => "network",
                            teloxide::RequestError::Api(_) => "api",
                            _ => "other",
                        };
                        ("send_typing_failed", error_class)
                    },
                    TypingRequestError::Timeout => ("send_typing_timeout", "timeout"),
                };
                if !*typing_failed {
                    warn!(
                        event = "telegram.typing.failed",
                        op,
                        account_handle,
                        chat_id,
                        reason_code,
                        error_class,
                        error = %error,
                        "failed to send typing indicator"
                    );
                    *typing_failed = true;
                } else {
                    debug!(
                        event = "telegram.typing.failed",
                        op,
                        account_handle,
                        chat_id,
                        reason_code,
                        error_class,
                        "typing indicator still failing"
                    );
                }
            },
        }
    }

    let mut initial_typing_failed = false;
    send_typing_once(
        &bot,
        account_handle,
        chat_id,
        op,
        &mut initial_typing_failed,
    )
    .await;

    let operation = operation;
    let typing_loop = async move {
        let mut typing_failed = initial_typing_failed;
        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            keepalive.tick().await;
            send_typing_once(&bot, account_handle, chat_id, op, &mut typing_failed).await;
        }
    };

    tokio::pin!(operation);
    tokio::pin!(typing_loop);

    tokio::select! {
        result = &mut operation => result,
        _ = &mut typing_loop => unreachable!("telegram command typing loop must stay pending until the operation completes"),
    }
}

/// Shared context injected into teloxide's dispatcher.
#[derive(Clone)]
pub struct HandlerContext {
    pub accounts: AccountStateMap,
    pub account_handle: String,
}

/// Build the teloxide update handler.
pub fn build_handler() -> Handler<
    'static,
    DependencyMap,
    Result<(), Box<dyn std::error::Error + Send + Sync>>,
    teloxide::dispatching::DpHandlerDescription,
> {
    Update::filter_message().endpoint(handle_message)
}

/// Handle a single inbound Telegram message (called from manual polling loop).
pub async fn handle_message_direct(
    msg: Message,
    bot: &Bot,
    account_handle: &str,
    accounts: &AccountStateMap,
) -> anyhow::Result<()> {
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    #[cfg(feature = "metrics")]
    counter!(tg_metrics::MESSAGES_RECEIVED_TOTAL).increment(1);

    let text = extract_text(&msg);
    if text.is_none() && !has_media(&msg) {
        debug!(account_handle, "ignoring non-text, non-media message");
        return Ok(());
    }

    let (config, bot_user_id, bot_username, outbound, message_log, event_sink, core_bridge) = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = match accts.get(account_handle) {
            Some(s) => s,
            None => {
                warn!(account_handle, "handler: account not found in state map");
                return Ok(());
            },
        };
        (
            state.config.clone(),
            state.bot_user_id,
            state.bot_username.clone(),
            Arc::clone(&state.outbound),
            state.message_log.clone(),
            state.event_sink.clone(),
            state.core_bridge.clone(),
        )
    };
    let bot_handle = bot_username.as_deref().map(|u| format!("@{u}"));

    let (chat_type, group_id) = classify_chat(&msg);
    let peer_id = msg
        .from
        .as_ref()
        .map(|u| u.id.0.to_string())
        .unwrap_or_default();
    let sender_name = msg.from.as_ref().and_then(|u| {
        let first = &u.first_name;
        let last = u.last_name.as_deref().unwrap_or("");
        let name = format!("{first} {last}").trim().to_string();
        if name.is_empty() {
            u.username.clone()
        } else {
            Some(name)
        }
    });

    let bot_mentioned = check_bot_mentioned(&msg, bot_user_id, bot_username.as_deref());

    debug!(
        account_handle,
        ?chat_type,
        peer_id,
        ?bot_mentioned,
        "checking access"
    );

    let username = msg.from.as_ref().and_then(|u| u.username.clone());
    let inbound_kind = message_kind(&msg);
    let text_len = text.as_ref().map_or(0, |body| body.len());
    info!(
        account_handle,
        chat_id = msg.chat.id.0,
        message_id = msg.id.0,
        peer_id,
        username = ?username,
        sender_name = ?sender_name,
        kind = ?inbound_kind,
        has_media = has_media(&msg),
        has_text = text.is_some(),
        text_len,
        "telegram inbound message received"
    );

    // Access control
    let access_result = access::check_access(
        &config,
        &chat_type,
        &peer_id,
        username.as_deref(),
        group_id.as_deref(),
        bot_mentioned,
    );
    let access_granted = access_result.is_ok();

    // Log every inbound message (before returning on denial).
    if let Some(ref log) = message_log {
        let chat_type_str = match chat_type {
            ChatType::Dm => "dm",
            ChatType::Group => "group",
            ChatType::Channel => "channel",
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let entry = MessageLogEntry {
            id: 0,
            account_handle: account_handle.to_string(),
            channel_type: ChannelType::Telegram.to_string(),
            peer_id: peer_id.clone(),
            username: username.clone(),
            sender_name: sender_name.clone(),
            chat_id: msg.chat.id.0.to_string(),
            chat_type: chat_type_str.into(),
            body: text.clone().unwrap_or_default(),
            access_granted,
            created_at: now,
        };
        if let Err(e) = log.log(entry).await {
            warn!(account_handle, "failed to log message: {e}");
        }
    }

    // Emit channel event for real-time UI updates.
    if let Some(ref sink) = event_sink {
        sink.emit(ChannelEvent::InboundMessage {
            chan_type: ChannelType::Telegram,
            chan_account_key: account_handle.to_string(),
            peer_id: peer_id.clone(),
            username: username.clone(),
            sender_name: sender_name.clone(),
            message_count: None,
            access_granted,
        })
        .await;
    }

    if let Err(reason) = access_result {
        warn!(account_handle, %reason, peer_id, username = ?username, "handler: access denied");
        #[cfg(feature = "metrics")]
        counter!(tg_metrics::ACCESS_CONTROL_DENIALS_TOTAL).increment(1);

        // OTP self-approval for non-allowlisted DM users.
        if reason == AccessDenied::NotOnAllowlist
            && chat_type == ChatType::Dm
            && config.otp_self_approval
        {
            handle_otp_flow(
                accounts,
                account_handle,
                &peer_id,
                username.as_deref(),
                sender_name.as_deref(),
                text.as_deref(),
                &msg,
                event_sink.as_deref(),
            )
            .await;
        }

        return Ok(());
    }

    debug!(account_handle, "handler: access granted");

    if let Some(kind) = unsupported_media_kind(&msg) {
        let reply_to = msg.id.0.to_string();
        info!(
            event = "telegram.attachment.unsupported",
            account_handle,
            chat_id = msg.chat.id.0,
            kind,
            has_caption = text.as_ref().is_some_and(|t| !t.trim().is_empty()),
            "telegram inbound unsupported attachment"
        );
        if let Err(_send_err) = outbound
            .send_text(
                account_handle,
                &msg.chat.id.0.to_string(),
                user_facing_unsupported_attachment_message(),
                Some(&reply_to),
            )
            .await
        {
            warn!(
                event = "telegram.user_feedback.failed",
                account_handle,
                reason_code = "send_text_failed",
                "failed to send unsupported attachment feedback"
            );
        }
        return Ok(());
    }

    // Check for voice/audio messages and transcribe them
    let (mut body, attachments) = if let Some(voice_file) = extract_voice_file(&msg) {
        // If STT is not configured, reply with guidance and do not dispatch to the LLM.
        if let Some(ref bridge) = core_bridge
            && !bridge.voice_transcription_available().await
        {
            if let Err(_e) = outbound
                .send_text(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "I can't understand voice, you did not configure it, please visit Settings -> Voice",
                    None,
                )
                .await
            {
                warn!(
                    event = "telegram.user_feedback.failed",
                    account_handle,
                    reason_code = "send_text_failed",
                    "failed to send STT setup hint"
                );
            }
            return Ok(());
        }

        // Try to transcribe the voice message
        if let Some(ref bridge) = core_bridge {
            match download_telegram_file(bot, &voice_file.file_id, 20 * 1024 * 1024).await {
                Ok(audio_data) => {
                    debug!(
                        account_handle,
                        file_id = %voice_file.file_id,
                        format = %voice_file.format,
                        size = audio_data.len(),
                        "downloaded voice file, transcribing"
                    );
                    match bridge
                        .request_voice_transcription(&audio_data, &voice_file.format)
                        .await
                    {
                        Ok(transcribed) => {
                            debug!(
                                account_handle,
                                text_len = transcribed.len(),
                                "voice transcription successful"
                            );
                            // Combine with any caption if present
                            let caption = text.clone().unwrap_or_default();
                            let body = if caption.is_empty() {
                                transcribed
                            } else {
                                format!("{}\n\n[Voice message]: {}", caption, transcribed)
                            };
                            (body, Vec::new())
                        },
                        Err(_e) => {
                            warn!(
                                event = "telegram.stt.failed",
                                account_handle,
                                reason_code = "stt_failed",
                                "voice transcription failed"
                            );
                            if let Err(_send_err) = outbound
                                .send_text(
                                    account_handle,
                                    &msg.chat.id.0.to_string(),
                                    "⚠️ I couldn't transcribe that voice message. Please try again or send text.",
                                    None,
                                )
                                .await
                            {
                                warn!(
                                    event = "telegram.user_feedback.failed",
                                    account_handle,
                                    reason_code = "send_text_failed",
                                    "failed to send STT failure feedback"
                                );
                            }
                            return Ok(());
                        },
                    }
                },
                Err(e) => {
                    warn!(
                        event = "telegram.download.failed",
                        account_handle,
                        reason_code = e.reason_code,
                        "failed to download voice file"
                    );
                    if e.retryable {
                        return Err(RetryableUpdateError {
                            reason_code: e.reason_code,
                        }
                        .into());
                    }
                    if let Err(_send_err) = outbound
                        .send_text(
                            account_handle,
                            &msg.chat.id.0.to_string(),
                            "⚠️ I couldn't download that voice message. Please try again.",
                            None,
                        )
                        .await
                    {
                        warn!(
                            event = "telegram.user_feedback.failed",
                            account_handle,
                            reason_code = "send_text_failed",
                            "failed to send download failure feedback"
                        );
                    }
                    return Ok(());
                },
            }
        } else {
            // No core bridge, can't transcribe
            (
                text.clone()
                    .unwrap_or_else(|| "[Voice message]".to_string()),
                Vec::new(),
            )
        }
    } else if let Some(photo_file) = extract_photo_file(&msg) {
        // Handle photo messages - download and send as multimodal content
        match download_telegram_file(bot, &photo_file.file_id, 25 * 1024 * 1024).await {
            Ok(image_data) => {
                debug!(
                    account_handle,
                    file_id = %photo_file.file_id,
                    size = image_data.len(),
                    "downloaded photo"
                );

                // Optimize image for LLM consumption (resize if needed, compress)
                let (final_data, media_type) = match moltis_media::image_ops::optimize_for_llm(
                    &image_data,
                    None,
                ) {
                    Ok(optimized) => {
                        if optimized.was_resized {
                            info!(
                                account_handle,
                                original_size = image_data.len(),
                                final_size = optimized.data.len(),
                                original_dims = %format!("{}x{}", optimized.original_width, optimized.original_height),
                                final_dims = %format!("{}x{}", optimized.final_width, optimized.final_height),
                                "resized image for LLM"
                            );
                        }
                        (optimized.data, optimized.media_type)
                    },
                    Err(e) => {
                        warn!(account_handle, error = %e, "failed to optimize image, using original");
                        (image_data, photo_file.media_type)
                    },
                };

                let attachment = ChannelAttachment {
                    media_type,
                    data: final_data,
                };
                // Use caption as text, or empty string if no caption
                let caption = text.clone().unwrap_or_default();
                (caption, vec![attachment])
            },
            Err(e) => {
                warn!(
                    event = "telegram.download.failed",
                    account_handle,
                    reason_code = e.reason_code,
                    "failed to download photo"
                );
                if e.retryable {
                    return Err(RetryableUpdateError {
                        reason_code: e.reason_code,
                    }
                    .into());
                }
                if let Err(_send_err) = outbound
                    .send_text(
                        account_handle,
                        &msg.chat.id.0.to_string(),
                        "⚠️ I couldn't download that photo. Please try again.",
                        None,
                    )
                    .await
                {
                    warn!(
                        event = "telegram.user_feedback.failed",
                        account_handle,
                        reason_code = "send_text_failed",
                        "failed to send photo download failure feedback"
                    );
                }
                return Ok(());
            },
        }
    } else if let Some(loc_info) = extract_location(&msg) {
        let lat = loc_info.latitude;
        let lon = loc_info.longitude;

        // Handle location sharing: update stored location and resolve any pending tool request.
        let resolved = if let Some(ref bridge) = core_bridge {
            let identity_links = crate::state::telegram_identity_links_snapshot(accounts);
            let inbound = build_tg_inbound(
                &config,
                account_handle,
                &chat_type,
                TgInboundMode::Dispatch,
                "",
                false,
                true,
                &msg,
                bot_mentioned,
                bot_user_id.map(|user_id| user_id.0 as u64),
                &identity_links,
            );
            let route = resolve_tg_route(&config, &inbound);
            bridge
                .update_location(
                    TgFollowUpTarget {
                        route,
                        private_target: build_tg_private_target(account_handle, &msg),
                    },
                    lat,
                    lon,
                )
                .await
        } else {
            false
        };

        info!(
            account_handle,
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            lat,
            lon,
            is_live = loc_info.is_live,
            resolved_pending_request = resolved,
            "telegram location received"
        );

        if resolved {
            // Pending tool request was resolved — the LLM will respond via the tool flow.
            if let Err(_e) = outbound
                .send_text_silent(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "Location updated.",
                    None,
                )
                .await
            {
                warn!(
                    event = "telegram.user_feedback.failed",
                    account_handle,
                    reason_code = "send_text_failed",
                    "failed to send location confirmation"
                );
            }
            return Ok(());
        }

        if loc_info.is_live {
            // Live location share — acknowledge silently, subsequent updates arrive
            // as EditedMessage and are handled by handle_edited_location().
            if let Err(_e) = outbound
                .send_text_silent(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "Live location tracking started. Your location will be updated automatically.",
                    None,
                )
                .await
            {
                warn!(
                    event = "telegram.user_feedback.failed",
                    account_handle,
                    reason_code = "send_text_failed",
                    "failed to send live location ack"
                );
            }
            return Ok(());
        }

        // Static location share — dispatch to LLM so it can acknowledge.
        (format!("I'm sharing my location: {lat}, {lon}"), Vec::new())
    } else {
        // Log unhandled media types so we know when users are sending attachments we don't process
        if let Some(media_type) = describe_media_kind(&msg) {
            info!(
                account_handle,
                peer_id, media_type, "received unhandled attachment type"
            );
        }
        (text.unwrap_or_default(), Vec::new())
    };

    let has_content = !body.is_empty() || !attachments.is_empty();
    if has_content && core_bridge.is_none() {
        info!(
            event = "telegram.inbound.ignored",
            account_handle,
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            reason_code = "core_bridge_missing",
            "telegram inbound ignored (core bridge missing)"
        );
        if chat_type == ChatType::Dm {
            let reply_to = msg.id.0.to_string();
            if let Err(_e) = outbound
                .send_text(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    user_facing_error_message(),
                    Some(&reply_to),
                )
                .await
            {
                warn!(
                    event = "telegram.user_feedback.failed",
                    account_handle,
                    reason_code = "send_text_failed",
                    "failed to send event sink missing feedback"
                );
            }
        }
        return Ok(());
    }
    if !has_content {
        info!(
            event = "telegram.inbound.ignored",
            account_handle,
            chat_id = msg.chat.id.0,
            message_id = msg.id.0,
            reason_code = "empty_content",
            "telegram inbound ignored (empty content)"
        );
        return Ok(());
    }

    // Dispatch to the chat session (per-channel session key derived by the core bridge).
    if let Some(ref bridge) = core_bridge {
        let sender_id = msg.from.as_ref().map(|u| u.id.0 as u64);
        let sender_is_bot = msg.from.as_ref().is_some_and(|u| u.is_bot);
        let managed_snapshots = managed_account_snapshots(accounts);
        let identity_links = crate::state::telegram_identity_links_snapshot(accounts);
        let planned_action = match chat_type {
            ChatType::Dm => crate::adapter::TgGroupTargetAction {
                mode: TgInboundMode::Dispatch,
                body: body.clone(),
                addressed: true,
                reason_code: "tg_dispatch_dm",
            },
            ChatType::Group | ChatType::Channel => {
                let managed_sender_account_handle =
                    register_group_runtime_participants(accounts, account_handle, &msg);
                let reply_to_target_account_handle =
                    resolve_reply_to_target_account_handle(accounts, &msg);
                let Some(action) = plan_group_target_action(
                    &body,
                    &managed_snapshots,
                    account_handle,
                    bot_username.as_deref(),
                    reply_to_target_account_handle.as_deref(),
                    config.group_line_start_mention_dispatch,
                    config.group_reply_to_dispatch,
                    !attachments.is_empty()
                        || inbound_kind.is_some_and(|kind| kind != ChannelMessageKind::Text),
                ) else {
                    info!(
                        event = "telegram.group.plan",
                        account_handle,
                        chat_id = msg.chat.id.0,
                        message_id = msg.id.0,
                        reason_code = "tg_noise_drop",
                        decision = "drop",
                        policy = "group_record_dispatch_v3",
                        "telegram group event dropped before gateway handoff"
                    );
                    return Ok(());
                };

                let dedupe_key = group_action_dedupe_key(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    &msg.id.0.to_string(),
                );
                let is_dup = crate::state::shared_group_runtime(accounts)
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .check_and_insert_action(&dedupe_key);
                if is_dup {
                    info!(
                        event = "telegram.group.plan",
                        account_handle,
                        chat_id = msg.chat.id.0,
                        message_id = msg.id.0,
                        reason_code = "tg_dedup_hit",
                        decision = "drop",
                        policy = "group_record_dispatch_v3",
                        "telegram group event deduped before gateway handoff"
                    );
                    return Ok(());
                }

                apply_group_dispatch_fuse(
                    accounts,
                    account_handle,
                    &msg,
                    managed_sender_account_handle.as_deref(),
                    action,
                )
            },
        };
        let inbound = build_tg_inbound(
            &config,
            account_handle,
            &chat_type,
            planned_action.mode,
            &planned_action.body,
            !attachments.is_empty(),
            false,
            &msg,
            planned_action.addressed,
            bot_user_id.map(|user_id| user_id.0 as u64),
            &identity_links,
        );
        let route = resolve_tg_route(&config, &inbound);
        let private_target = build_tg_private_target(account_handle, &msg);
        let reply_target =
            build_channel_reply_target(account_handle, bot_handle.clone(), &msg, &route);
        let follow_up_target = TgFollowUpTarget {
            route: route.clone(),
            private_target: private_target.clone(),
        };

        info!(
            account_handle,
            chat_id = %reply_target.chat_id,
            message_id = ?reply_target.message_id,
            bucket_key = %route.bucket_key,
            addressed = route.addressed,
            body_len = planned_action.body.len(),
            attachment_count = attachments.len(),
            message_kind = ?inbound_kind,
            reason_code = planned_action.reason_code,
            "telegram inbound dispatched to chat"
        );

        // Intercept slash commands before dispatching to the LLM.
        //
        // NOTE: Telegram supports addressed commands in groups: `/context@MyBot`.
        // We treat unaddressed commands in Group/Channel as ambiguous and ignore them
        // (avoid multi-bot spam), even if the account is configured with a permissive
        // `mention_mode`.
        if planned_action.body.trim_start().starts_with('/') {
            let body_trim = planned_action.body.trim_start();
            let cmd_text_full = body_trim.trim_start_matches('/').trim();
            let first = cmd_text_full.split_whitespace().next().unwrap_or("");
            let (cmd_name_raw, addressed_raw) = first
                .split_once('@')
                .map_or((first, None), |(a, b)| (a, Some(b)));

            let cmd_name = cmd_name_raw.to_lowercase();
            let bot_username_norm = bot_username.as_deref().map(normalize_username);
            let addressed_norm = addressed_raw.map(normalize_username);

            // Group/Channel: only handle addressed commands (`/cmd@this_bot`).
            if chat_type != ChatType::Dm {
                match (&addressed_norm, &bot_username_norm) {
                    (Some(target), Some(me)) if target == me => {},
                    (Some(_target), _) => {
                        info!(
                            event = "telegram.command.ignored",
                            account_handle,
                            chat_id = msg.chat.id.0,
                            message_id = msg.id.0,
                            reason_code = "addressed_to_other_bot",
                            command = cmd_name_raw,
                            "telegram command ignored (addressed to another bot)"
                        );
                        return Ok(());
                    },
                    (None, _) => {
                        info!(
                            event = "telegram.command.ignored",
                            account_handle,
                            chat_id = msg.chat.id.0,
                            message_id = msg.id.0,
                            reason_code = "unaddressed_in_group",
                            command = cmd_name_raw,
                            "telegram command ignored (unaddressed in group)"
                        );
                        return Ok(());
                    },
                }
            } else if let (Some(target), Some(me)) = (&addressed_norm, &bot_username_norm) {
                // DM: if the user explicitly addressed another bot, ignore.
                if target != me {
                    info!(
                        event = "telegram.command.ignored",
                        account_handle,
                        chat_id = msg.chat.id.0,
                        message_id = msg.id.0,
                        reason_code = "addressed_to_other_bot",
                        command = cmd_name_raw,
                        "telegram command ignored (addressed to another bot)"
                    );
                    return Ok(());
                }
            }

            let args = cmd_text_full[first.len()..].trim();
            let cmd_text = if args.is_empty() {
                cmd_name.clone()
            } else {
                format!("{cmd_name} {args}")
            };

            if matches!(
                cmd_name.as_str(),
                "new" | "clear" | "compact" | "context" | "model" | "sandbox" | "sessions" | "help"
            ) {
                // For /context, send a formatted card with inline keyboard.
                if cmd_name == "context" {
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        let chat_id = reply_target.chat_id.clone();
                        let config = config.clone();
                        let reply_target = reply_target.clone();
                        run_with_telegram_typing(
                            bot.clone(),
                            account_handle,
                            &chat_id,
                            "slash_command_context",
                            async move {
                                let context_result = bridge
                                    .dispatch_command("context", follow_up_target.clone())
                                    .await;
                                match context_result {
                                    Ok(text) => {
                                        if let Err(_e) = send_context_card(
                                            &bot,
                                            &reply_target.chat_id,
                                            &text,
                                            &config,
                                            chat_type,
                                        )
                                        .await
                                        {
                                            warn!(
                                                event = "telegram.helper_send.failed",
                                                account_handle,
                                                reason_code = "send_context_card_failed",
                                                "failed to send context card"
                                            );
                                        }
                                    },
                                    Err(_e) => {
                                        warn!(
                                            event = "telegram.command.failed",
                                            account_handle,
                                            reason_code = "dispatch_command_failed",
                                            "context command failed"
                                        );
                                        if let Err(_e) = outbound
                                            .send_text_silent(
                                                account_handle,
                                                &reply_target.chat_id,
                                                user_facing_error_message(),
                                                reply_target.message_id.as_deref(),
                                            )
                                            .await
                                        {
                                            warn!(
                                                event = "telegram.user_feedback.failed",
                                                account_handle,
                                                reason_code = "send_text_failed",
                                                "failed to send context command failure feedback"
                                            );
                                        }
                                    },
                                }
                            },
                        )
                        .await;
                    }
                    return Ok(());
                }

                // For /model without args, send an inline keyboard to pick a model.
                if cmd_name == "model" && args.is_empty() {
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        let chat_id = reply_target.chat_id.clone();
                        let reply_target = reply_target.clone();
                        run_with_telegram_typing(
                            bot.clone(),
                            account_handle,
                            &chat_id,
                            "slash_command_model",
                            async move {
                                let list_result = bridge
                                    .dispatch_command("model", follow_up_target.clone())
                                    .await;
                                match list_result {
                                    Ok(text) => {
                                        if let Err(_e) =
                                            send_model_keyboard(&bot, &reply_target, &text).await
                                        {
                                            warn!(
                                                event = "telegram.helper_send.failed",
                                                account_handle,
                                                reason_code = "send_model_keyboard_failed",
                                                "failed to send model keyboard"
                                            );
                                        }
                                    },
                                    Err(_e) => {
                                        warn!(
                                            event = "telegram.command.failed",
                                            account_handle,
                                            reason_code = "dispatch_command_failed",
                                            "model command failed"
                                        );
                                        if let Err(_e) = outbound
                                            .send_text_silent(
                                                account_handle,
                                                &reply_target.chat_id,
                                                user_facing_error_message(),
                                                reply_target.message_id.as_deref(),
                                            )
                                            .await
                                        {
                                            warn!(
                                                event = "telegram.user_feedback.failed",
                                                account_handle,
                                                reason_code = "send_text_failed",
                                                "failed to send model command failure feedback"
                                            );
                                        }
                                    },
                                }
                            },
                        )
                        .await;
                    }
                    return Ok(());
                }

                // For /sandbox without args, send toggle + image keyboard.
                if cmd_name == "sandbox" && args.is_empty() {
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        let chat_id = reply_target.chat_id.clone();
                        let reply_target = reply_target.clone();
                        run_with_telegram_typing(
                            bot.clone(),
                            account_handle,
                            &chat_id,
                            "slash_command_sandbox",
                            async move {
                                let list_result = bridge
                                    .dispatch_command("sandbox", follow_up_target.clone())
                                    .await;
                                match list_result {
                                    Ok(text) => {
                                        if let Err(_e) =
                                            send_sandbox_keyboard(&bot, &reply_target, &text).await
                                        {
                                            warn!(
                                                event = "telegram.helper_send.failed",
                                                account_handle,
                                                reason_code = "send_sandbox_keyboard_failed",
                                                "failed to send sandbox keyboard"
                                            );
                                        }
                                    },
                                    Err(_e) => {
                                        warn!(
                                            event = "telegram.command.failed",
                                            account_handle,
                                            reason_code = "dispatch_command_failed",
                                            "sandbox command failed"
                                        );
                                        if let Err(_e) = outbound
                                            .send_text_silent(
                                                account_handle,
                                                &reply_target.chat_id,
                                                user_facing_error_message(),
                                                reply_target.message_id.as_deref(),
                                            )
                                            .await
                                        {
                                            warn!(
                                                event = "telegram.user_feedback.failed",
                                                account_handle,
                                                reason_code = "send_text_failed",
                                                "failed to send sandbox command failure feedback"
                                            );
                                        }
                                    },
                                }
                            },
                        )
                        .await;
                    }
                    return Ok(());
                }

                // For /sessions without args, send an inline keyboard instead of plain text.
                if cmd_name == "sessions" && args.is_empty() {
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        let chat_id = reply_target.chat_id.clone();
                        let reply_target = reply_target.clone();
                        run_with_telegram_typing(
                            bot.clone(),
                            account_handle,
                            &chat_id,
                            "slash_command_sessions",
                            async move {
                                let list_result = bridge
                                    .dispatch_command("sessions", follow_up_target.clone())
                                    .await;
                                match list_result {
                                    Ok(text) => {
                                        if let Err(_e) =
                                            send_sessions_keyboard(&bot, &reply_target, &text).await
                                        {
                                            warn!(
                                                event = "telegram.helper_send.failed",
                                                account_handle,
                                                reason_code = "send_sessions_keyboard_failed",
                                                "failed to send sessions keyboard"
                                            );
                                        }
                                    },
                                    Err(_e) => {
                                        warn!(
                                            event = "telegram.command.failed",
                                            account_handle,
                                            reason_code = "dispatch_command_failed",
                                            "sessions command failed"
                                        );
                                        if let Err(_e) = outbound
                                            .send_text_silent(
                                                account_handle,
                                                &reply_target.chat_id,
                                                user_facing_error_message(),
                                                reply_target.message_id.as_deref(),
                                            )
                                            .await
                                        {
                                            warn!(
                                                event = "telegram.user_feedback.failed",
                                                account_handle,
                                                reason_code = "send_text_failed",
                                                "failed to send sessions command failure feedback"
                                            );
                                        }
                                    },
                                }
                            },
                        )
                        .await;
                    }
                    return Ok(());
                }

                // Get the outbound Arc before awaiting (avoid holding RwLockReadGuard across await).
                let outbound = {
                    let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                    accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
                };
                let bot = {
                    let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                    accts.get(account_handle).map(|s| s.bot.clone())
                };
                if let (Some(outbound), Some(bot)) = (outbound, bot) {
                    let chat_id = reply_target.chat_id.clone();
                    let reply_target = reply_target.clone();
                    run_with_telegram_typing(
                        bot,
                        account_handle,
                        &chat_id,
                        "slash_command",
                        async move {
                            let response = if cmd_name == "help" {
                                "Available commands:\n/new — Start a new session\n/sessions — List and switch sessions\n/model — Switch provider/model\n/sandbox — Toggle sandbox and choose image\n/clear — Clear session history\n/compact — Compact session (summarize)\n/context — Show session context info\n/help — Show this help".to_string()
                            } else {
                                match bridge
                                    .dispatch_command(&cmd_text, follow_up_target.clone())
                                    .await
                                {
                                    Ok(msg) => msg,
                                    Err(_e) => {
                                        warn!(
                                            event = "telegram.command.failed",
                                            account_handle,
                                            reason_code = "dispatch_command_failed",
                                            command = cmd_name,
                                            "slash command dispatch_command failed"
                                        );
                                        user_facing_error_message().to_string()
                                    },
                                }
                            };
                            if let Err(_e) = outbound
                                .send_text_silent(
                                    account_handle,
                                    &reply_target.chat_id,
                                    &response,
                                    reply_target.message_id.as_deref(),
                                )
                                .await
                            {
                                warn!(
                                    event = "telegram.user_feedback.failed",
                                    account_handle,
                                    reason_code = "send_text_failed",
                                    "failed to send command response"
                                );
                            }
                        },
                    )
                    .await;
                }
                return Ok(());
            }

            // Unknown addressed command: reply with help (DM or addressed group).
            let help_hint = if let Some(me) = bot_username.as_deref().filter(|s| !s.is_empty()) {
                format!("Use /help@{me}.")
            } else {
                "Use /help.".to_string()
            };
            let response = format!("Unknown command: /{cmd_name_raw}. {help_hint}");
            let outbound = {
                let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
            };
            if let Some(outbound) = outbound
                && let Err(_e) = outbound
                    .send_text(
                        account_handle,
                        &reply_target.chat_id,
                        &response,
                        reply_target.message_id.as_deref(),
                    )
                    .await
            {
                warn!(
                    event = "telegram.user_feedback.failed",
                    account_handle,
                    reason_code = "send_text_failed",
                    "failed to send command response"
                );
            }
            return Ok(());
        }

        let inbound_message_kind = message_kind(&msg);
        let tg_gst_v1 = chat_type == ChatType::Group;
        let mut final_body = planned_action.body.clone();

        if tg_gst_v1 {
            if let Some(ref bot_username) = bot_username
                && attachments.is_empty()
                && bot_mentioned
                && tg_gst_v1_is_self_mention_only(&msg, &body, bot_user_id, bot_username)
            {
                let presence_inbound = TgInbound {
                    mode: TgInboundMode::RecordOnly,
                    body: TgContent {
                        text: body.clone(),
                        has_attachments: false,
                        has_location: false,
                    },
                    ..inbound.clone()
                };
                let request = build_tg_inbound_request(
                    &presence_inbound,
                    route.clone(),
                    private_target.clone(),
                    &managed_snapshots,
                    &identity_links,
                    sender_name.clone(),
                    username.clone(),
                    sender_id,
                    sender_is_bot,
                    inbound_message_kind,
                    config.model.clone(),
                    Vec::new(),
                );
                bridge.handle_inbound(request).await;

                let outbound = {
                    let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                    accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
                };
                if let Some(outbound) = outbound
                    && let Err(_e) = outbound
                        .send_text(
                            account_handle,
                            &reply_target.chat_id,
                            "我在。",
                            reply_target.message_id.as_deref(),
                        )
                        .await
                {
                    warn!(
                        event = "telegram.user_feedback.failed",
                        account_handle,
                        reason_code = "send_text_failed",
                        "failed to send presence reply"
                    );
                }
                return Ok(());
            }
        } else {
            // Strip self-mentions from the user text before it is persisted / sent to the LLM.
            if let Some(ref bot_username) = bot_username {
                let (rewritten, stripped) =
                    strip_self_mention_from_message(&msg, &body, bot_user_id, bot_username);
                if stripped {
                    debug!(
                        account_handle,
                        before_len = body.len(),
                        after_len = rewritten.len(),
                        "telegram: stripped self-mention from user text"
                    );
                    body = rewritten;

                    // User only sent "@this_bot" (after stripping): reply fixed short phrase, no LLM.
                    if body.trim().is_empty() && attachments.is_empty() {
                        let outbound = {
                            let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                            accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
                        };
                        if let Some(outbound) = outbound
                            && let Err(_e) = outbound
                                .send_text(
                                    account_handle,
                                    &reply_target.chat_id,
                                    "我在。",
                                    reply_target.message_id.as_deref(),
                                )
                                .await
                        {
                            warn!(
                                event = "telegram.user_feedback.failed",
                                account_handle,
                                reason_code = "send_text_failed",
                                "failed to send presence reply"
                            );
                        }
                        return Ok(());
                    }
                    final_body = body.clone();
                }
            }
        }

        let attachments = attachments
            .into_iter()
            .map(|attachment| TgAttachment {
                media_type: attachment.media_type,
                data: attachment.data,
            })
            .collect::<Vec<_>>();
        let final_inbound = TgInbound {
            mode: planned_action.mode,
            body: TgContent {
                text: final_body,
                has_attachments: !attachments.is_empty(),
                has_location: false,
            },
            ..inbound
        };
        let request = build_tg_inbound_request(
            &final_inbound,
            route,
            private_target,
            &managed_snapshots,
            &identity_links,
            sender_name.clone(),
            username.clone(),
            sender_id,
            sender_is_bot,
            inbound_message_kind,
            config.model.clone(),
            attachments,
        );
        bridge.handle_inbound(request).await;
    }

    #[cfg(feature = "metrics")]
    histogram!(tg_metrics::POLLING_DURATION_SECONDS).record(start.elapsed().as_secs_f64());

    Ok(())
}

/// OTP challenge message sent to the Telegram user.
///
/// **Security invariant:** this message must NEVER contain the actual
/// verification code.  The code is only visible to the bot owner in the
/// web UI (Channels → Senders).  Leaking it here would let any
/// unauthenticated user self-approve without admin awareness.
pub(crate) const OTP_CHALLENGE_MSG: &str = "To use this bot, please enter the verification code.\n\nAsk the bot owner for the code \u{2014} it is visible in the web UI under <b>Channels \u{2192} Senders</b>.\n\nThe code expires in 5 minutes.";

/// Handle OTP challenge/verification flow for a non-allowlisted DM user.
///
/// Called when `dm_policy = Allowlist`, the peer is not on the allowlist, and
/// `otp_self_approval` is enabled. Manages the full lifecycle:
/// - First message: issue a 6-digit OTP challenge
/// - Code reply: verify and auto-approve on match
/// - Non-code messages while pending: silently ignored (flood protection)
#[allow(clippy::too_many_arguments)]
async fn handle_otp_flow(
    accounts: &AccountStateMap,
    account_handle: &str,
    peer_id: &str,
    username: Option<&str>,
    sender_name: Option<&str>,
    text: Option<&str>,
    msg: &Message,
    event_sink: Option<&dyn moltis_channels::ChannelEventSink>,
) {
    let chat_id = msg.chat.id;

    // Resolve bot early (needed for sending messages).
    let bot = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        accts.get(account_handle).map(|s| s.bot.clone())
    };
    let bot = match bot {
        Some(b) => b,
        None => return,
    };

    // Check current OTP state.
    let has_pending = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        accts
            .get(account_handle)
            .map(|s| {
                let otp = s.otp.lock().unwrap_or_else(|e| e.into_inner());
                otp.has_pending(peer_id)
            })
            .unwrap_or(false)
    };

    if has_pending {
        // A challenge is already pending. Check if the user sent a 6-digit code.
        let body = text.unwrap_or("").trim();
        let is_code = body.len() == 6 && body.chars().all(|c| c.is_ascii_digit());

        if !is_code {
            // Silent ignore — flood protection.
            debug!(
                event = "telegram.otp.ignored",
                account_handle,
                peer_id,
                reason_code = "non_code",
                "otp pending: ignored non-code message"
            );
            return;
        }

        // Verify the code.
        let result = {
            let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
            match accts.get(account_handle) {
                Some(s) => {
                    let mut otp = s.otp.lock().unwrap_or_else(|e| e.into_inner());
                    otp.verify(peer_id, body)
                },
                None => return,
            }
        };

        match result {
            OtpVerifyResult::Approved => {
                // Auto-approve: add to allowlist via the event sink.
                let identifier = username.unwrap_or(peer_id);
                if let Some(sink) = event_sink {
                    sink.request_sender_approval("telegram", account_handle, identifier)
                        .await;
                }

                if let Err(_e) = bot
                    .send_message(chat_id, "Verified! You now have access to this bot.")
                    .await
                {
                    warn!(
                        event = "telegram.helper_send.failed",
                        account_handle,
                        peer_id,
                        reason_code = "otp_verified_send_failed",
                        "failed to send OTP approved message"
                    );
                }

                // Emit resolved event.
                if let Some(sink) = event_sink {
                    sink.emit(ChannelEvent::OtpResolved {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: account_handle.to_string(),
                        peer_id: peer_id.to_string(),
                        username: username.map(String::from),
                        resolution: "approved".into(),
                    })
                    .await;
                }

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_VERIFICATIONS_TOTAL, "result" => "approved").increment(1);
            },
            OtpVerifyResult::WrongCode { attempts_left } => {
                if let Err(_e) = bot
                    .send_message(
                        chat_id,
                        format!(
                            "Incorrect code. {attempts_left} attempt{} remaining.",
                            if attempts_left == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ),
                    )
                    .await
                {
                    warn!(
                        event = "telegram.helper_send.failed",
                        account_handle,
                        peer_id,
                        reason_code = "otp_wrong_code_send_failed",
                        "failed to send OTP wrong-code message"
                    );
                }

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_VERIFICATIONS_TOTAL, "result" => "wrong_code")
                    .increment(1);
            },
            OtpVerifyResult::LockedOut => {
                if let Err(_e) = bot
                    .send_message(chat_id, "Too many failed attempts. Please try again later.")
                    .await
                {
                    warn!(
                        event = "telegram.helper_send.failed",
                        account_handle,
                        peer_id,
                        reason_code = "otp_locked_out_send_failed",
                        "failed to send OTP locked-out message"
                    );
                }

                if let Some(sink) = event_sink {
                    sink.emit(ChannelEvent::OtpResolved {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: account_handle.to_string(),
                        peer_id: peer_id.to_string(),
                        username: username.map(String::from),
                        resolution: "locked_out".into(),
                    })
                    .await;
                }

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_VERIFICATIONS_TOTAL, "result" => "locked_out")
                    .increment(1);
            },
            OtpVerifyResult::Expired => {
                if let Err(_e) = bot
                    .send_message(
                        chat_id,
                        "Your code has expired. Send any message to get a new one.",
                    )
                    .await
                {
                    warn!(
                        event = "telegram.helper_send.failed",
                        account_handle,
                        peer_id,
                        reason_code = "otp_expired_send_failed",
                        "failed to send OTP expired message"
                    );
                }

                if let Some(sink) = event_sink {
                    sink.emit(ChannelEvent::OtpResolved {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: account_handle.to_string(),
                        peer_id: peer_id.to_string(),
                        username: username.map(String::from),
                        resolution: "expired".into(),
                    })
                    .await;
                }

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_VERIFICATIONS_TOTAL, "result" => "expired").increment(1);
            },
            OtpVerifyResult::NoPending => {
                // Shouldn't happen since we checked has_pending, but handle gracefully.
            },
        }
    } else {
        // No pending challenge — initiate one.
        let init_result = {
            let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
            match accts.get(account_handle) {
                Some(s) => {
                    let mut otp = s.otp.lock().unwrap_or_else(|e| e.into_inner());
                    otp.initiate(
                        peer_id,
                        username.map(String::from),
                        sender_name.map(String::from),
                    )
                },
                None => return,
            }
        };

        match init_result {
            OtpInitResult::Created(code) => {
                if let Err(_e) = bot
                    .send_message(chat_id, OTP_CHALLENGE_MSG)
                    .parse_mode(ParseMode::Html)
                    .await
                {
                    warn!(
                        event = "telegram.helper_send.failed",
                        account_handle,
                        peer_id,
                        reason_code = "otp_challenge_send_failed",
                        "failed to send OTP challenge message"
                    );
                }

                // Emit OTP challenge event for the admin UI.
                if let Some(sink) = event_sink {
                    // Compute expires_at epoch.
                    let expires_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                        + 300;

                    sink.emit(ChannelEvent::OtpChallenge {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: account_handle.to_string(),
                        peer_id: peer_id.to_string(),
                        username: username.map(String::from),
                        sender_name: sender_name.map(String::from),
                        code,
                        expires_at,
                    })
                    .await;
                }

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_CHALLENGES_TOTAL).increment(1);
            },
            OtpInitResult::AlreadyPending => {
                // Silent ignore.
                debug!(
                    event = "telegram.otp.ignored",
                    account_handle,
                    peer_id,
                    reason_code = "already_pending",
                    "otp ignored (already pending)"
                );
            },
            OtpInitResult::LockedOut => {
                // Silent ignore.
                debug!(
                    event = "telegram.otp.ignored",
                    account_handle,
                    peer_id,
                    reason_code = "locked_out",
                    "otp ignored (locked out)"
                );
            },
        }
    }
}

/// Handle an edited message — only processes live location updates.
///
/// Telegram sends live location updates as `EditedMessage` with `MediaKind::Location`.
/// We silently update the cached location without dispatching to the LLM or
/// re-checking access (the user was already approved on the initial share).
pub async fn handle_edited_location(
    msg: Message,
    account_handle: &str,
    accounts: &AccountStateMap,
) -> anyhow::Result<()> {
    let Some(loc_info) = extract_location(&msg) else {
        // Not a location edit — ignore (could be a text edit, etc.).
        return Ok(());
    };
    let lat = loc_info.latitude;
    let lon = loc_info.longitude;

    debug!(
        account_handle,
        lat,
        lon,
        chat_id = msg.chat.id.0,
        "live location update"
    );
    info!(
        account_handle,
        chat_id = msg.chat.id.0,
        message_id = msg.id.0,
        lat,
        lon,
        "telegram live location update received"
    );

    let (core_bridge, config) = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        (
            accts
                .get(account_handle)
                .and_then(|s| s.core_bridge.clone()),
            accts.get(account_handle).map(|s| s.config.clone()),
        )
    };

    if let Some(ref bridge) = core_bridge {
        let (chat_type, _) = classify_chat(&msg);
        let identity_links = crate::state::telegram_identity_links_snapshot(accounts);
        let sender = tg_sender_for_message(&msg, &identity_links);
        let branch = crate::adapter::resolve_branch_key(
            message_thread_id_text(&msg).as_deref(),
            reply_to_message_id_text(&msg).as_deref(),
        );
        let peer = tg_peer_for_message(&chat_type, &msg, &identity_links).unwrap_or_default();
        if let Some(config) = config.as_ref() {
            log_follow_up_route_degrade(
                config,
                account_handle,
                &msg.chat.id.0.to_string(),
                Some(msg.id.0),
                &chat_type,
                sender.as_deref(),
                branch.as_deref(),
                "edited_location",
            );
        }
        let bucket_key = config.as_ref().map(|config| {
            tg_bucket_key_for_route(
                config,
                crate::adapter::account_key_from_config(config).as_deref(),
                &chat_type,
                &peer,
                sender.as_deref(),
                branch.as_deref(),
            )
        });
        let route = TgRoute {
            peer,
            sender,
            bucket_key: bucket_key.unwrap_or_default(),
            addressed: false,
        };
        bridge
            .update_location(
                TgFollowUpTarget {
                    route,
                    private_target: TgPrivateTarget {
                        account_handle: account_handle.to_string(),
                        chat_id: msg.chat.id.0.to_string(),
                        message_id: Some(msg.id.0.to_string()),
                        thread_id: message_thread_id_text(&msg),
                    },
                },
                lat,
                lon,
            )
            .await;
    }

    Ok(())
}

/// Handle a single inbound Telegram message (teloxide dispatcher endpoint).
async fn handle_message(
    msg: Message,
    bot: Bot,
    ctx: Arc<HandlerContext>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    handle_message_direct(msg, &bot, &ctx.account_handle, &ctx.accounts)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Send a sessions list as an inline keyboard.
///
/// Parses the text response from `dispatch_command("sessions")` to extract
/// session labels, then sends an inline keyboard with one button per session.
async fn send_sessions_keyboard(
    bot: &Bot,
    reply_target: &ChannelReplyTarget,
    sessions_text: &str,
) -> anyhow::Result<()> {
    let chat = parse_chat_id(&reply_target.chat_id)?;

    // Parse numbered lines like "1. Session label (5 msgs) *"
    let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for line in sessions_text.lines() {
        let trimmed = line.trim();
        // Match lines starting with a number followed by ". "
        if let Some(dot_pos) = trimmed.find(". ")
            && let Ok(n) = trimmed[..dot_pos].parse::<usize>()
        {
            let label_part = &trimmed[dot_pos + 2..];
            let is_active = label_part.ends_with('*');
            let display = if is_active {
                format!("● {}", label_part.trim_end_matches('*').trim())
            } else {
                format!("○ {label_part}")
            };
            buttons.push(vec![InlineKeyboardButton::callback(
                display,
                with_callback_sender_hint(format!("sessions_switch:{n}"), reply_target),
            )]);
        }
    }

    if buttons.is_empty() {
        bot.send_message(chat, sessions_text).await?;
        return Ok(());
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let sent = bot
        .send_message(chat, "Select a session:")
        .reply_markup(keyboard)
        .await?;
    remember_callback_bucket_binding(reply_target, sent.id.0);
    Ok(())
}

/// Send context info as a formatted HTML card with blockquote sections.
///
/// Parses the markdown context response from `dispatch_command("context")`
/// and renders it as a structured Telegram HTML message.
async fn send_context_card(
    bot: &Bot,
    chat_id: &str,
    context_text: &str,
    config: &crate::config::TelegramAccountConfig,
    chat_type: ChatType,
) -> anyhow::Result<()> {
    let chat = parse_chat_id(chat_id)?;

    // Preferred path: context.v1 JSON contract emitted by the gateway.
    if let Some(payload) = parse_context_v1_payload(context_text) {
        let html = render_context_card_v1(&payload, config, chat_type);
        bot.send_message(chat, html)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    } else if context_text.trim_start().starts_with('{') {
        warn!(
            len = context_text.len(),
            "telegram /context: failed to parse context.v1 JSON, falling back to markdown"
        );
    }

    let html = render_context_card_markdown_fallback(context_text);

    bot.send_message(chat, html)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

fn render_context_card_markdown_fallback(context_text: &str) -> String {
    // Parse "**Key:** value" lines from the markdown response into a map.
    let mut fields: Vec<(&str, String)> = Vec::new();
    for line in context_text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("**")
            && let Some(end) = rest.find("**")
        {
            let label = &rest[..end];
            let raw_value = rest[end + 2..].trim();
            // Strip markdown backticks from value
            let value = raw_value.replace('`', "");
            fields.push((label, escape_html_simple(&value)));
        }
    }

    let get = |key: &str| -> String {
        fields
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    let session = get("Session:");
    let messages = get("Messages:");
    let provider = get("Provider:");
    let model = get("Model:");
    let sandbox = get("Sandbox:");
    let plugins_raw = get("Plugins:");
    let tokens = {
        let explicit = get("Tokens:");
        if !explicit.is_empty() {
            explicit
        } else {
            let last = get("Last:");
            let next = get("Next (est):");
            let mut parts = Vec::new();
            if !last.is_empty() {
                parts.push(format!("Last {last}"));
            }
            if !next.is_empty() {
                parts.push(format!("Next {next}"));
            }
            if parts.is_empty() {
                "".to_string()
            } else {
                parts.join(" · ")
            }
        }
    };

    // Format plugins as individual lines
    let plugins_section = if plugins_raw == "none" || plugins_raw.is_empty() {
        "  <i>none</i>".to_string()
    } else {
        plugins_raw
            .split(", ")
            .map(|p| format!("  ▸ {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Sandbox indicator
    let sandbox_icon = if sandbox.starts_with("on") {
        "🟢"
    } else {
        "⚫"
    };

    format!(
        "\
<b>📋 Session Context</b>

<blockquote><b>🤖 Model</b>
{provider} · <code>{model}</code>

<b>{sandbox_icon} Sandbox</b>
{sandbox}

<b>🧩 Plugins</b>
{plugins_section}</blockquote>

<code>Session   {session}
Messages  {messages}
Tokens    {tokens}</code>"
    )
}

fn parse_context_v1_payload(context_text: &str) -> Option<serde_json::Value> {
    let trimmed = context_text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let root: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    if root.get("format")?.as_str()? != "context.v1" {
        return None;
    }
    Some(root.get("payload")?.clone())
}

fn truncate_middle(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if max_chars == 0 || total <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return "…".to_string();
    }
    let keep_head = (max_chars - 1) / 2;
    let keep_tail = max_chars - 1 - keep_head;
    let head: String = s.chars().take(keep_head).collect();
    let tail: String = s.chars().skip(total.saturating_sub(keep_tail)).collect();
    format!("{head}…{tail}")
}

fn format_bool(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

fn format_opt_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string())
}

fn clamp_html_len(html: String, max_chars: usize, fallback_html: String) -> String {
    if html.chars().count() <= max_chars {
        return html;
    }
    fallback_html
}

fn render_context_card_v1(
    payload: &serde_json::Value,
    config: &crate::config::TelegramAccountConfig,
    chat_type: ChatType,
) -> String {
    // Telegram message cap is 4096 chars; leave margin for HTML tags.
    const MAX_HTML_LEN: usize = 3600;
    const MAX_LIST_LINES: usize = 8;

    let session = payload.get("session").cloned().unwrap_or_default();
    let llm = payload.get("llm").cloned().unwrap_or_default();
    let sandbox = payload.get("sandbox").cloned().unwrap_or_default();
    let compaction = payload.get("compaction").cloned().unwrap_or_default();
    let token_debug = payload.get("tokenDebug").cloned().unwrap_or_default();

    let session_key = session
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let msg_count = session.get("messageCount").and_then(|v| v.as_u64());

    let provider = llm
        .get("provider")
        .and_then(|v| v.as_str())
        .or_else(|| session.get("provider").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let model = llm
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| session.get("model").and_then(|v| v.as_str()))
        .unwrap_or("default");

    // LLM overrides (best-effort, may be provider-specific).
    let overrides = llm.get("overrides").cloned().unwrap_or_default();
    let prompt_cache_key = overrides
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .map(|s| truncate_middle(s, 64));
    let generation = overrides.get("generation").cloned().unwrap_or_default();
    let max_out_effective = generation
        .get("max_output_tokens")
        .and_then(|v| v.get("effective"))
        .and_then(|v| v.as_u64());
    let max_out_configured = generation
        .get("max_output_tokens")
        .and_then(|v| v.get("configured"))
        .and_then(|v| v.as_u64());
    let reasoning_effort = generation
        .get("reasoning_effort")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let text_verbosity = generation
        .get("text_verbosity")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let temperature = generation.get("temperature").and_then(|v| v.as_f64());

    let overrides_lines: Vec<String> = {
        let mut lines = Vec::new();
        if let Some(k) = prompt_cache_key {
            lines.push(format!(
                "prompt_cache_key: <code>{}</code>",
                escape_html_simple(&k)
            ));
        }
        if max_out_effective.is_some() || max_out_configured.is_some() {
            lines.push(format!(
                "max_output_tokens: {}{}",
                escape_html_simple(&format_opt_u64(max_out_effective)),
                max_out_configured
                    .map(|c| format!(" (configured {})", c))
                    .unwrap_or_default()
            ));
        }
        if let Some(e) = reasoning_effort {
            lines.push(format!(
                "reasoning_effort: <code>{}</code>",
                escape_html_simple(&e)
            ));
        }
        if let Some(v) = text_verbosity {
            lines.push(format!(
                "text_verbosity: <code>{}</code>",
                escape_html_simple(&v)
            ));
        }
        if let Some(t) = temperature {
            lines.push(format!("temperature: <code>{:.2}</code>", t));
        }
        if lines.is_empty() && !overrides.is_null() {
            let raw = serde_json::to_string(&overrides).unwrap_or_default();
            lines.push(format!(
                "overrides: <code>{}</code>",
                escape_html_simple(&truncate_middle(&raw, 220))
            ));
        }
        lines
    };

    // Compaction.
    let is_compacted = compaction
        .get("isCompacted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let summary_len = compaction.get("summaryLen").and_then(|v| v.as_u64());
    let kept_count = compaction.get("keptMessageCount").and_then(|v| v.as_u64());
    let keep_rounds = compaction
        .get("keepLastUserRounds")
        .and_then(|v| v.as_u64());

    // Sandbox / mounts.
    let sandbox_enabled = sandbox
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let sandbox_backend = sandbox
        .get("backend")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let sandbox_image = sandbox.get("image").and_then(|v| v.as_str()).unwrap_or("");
    let external_mounts_status = sandbox
        .get("externalMountsStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mounts = sandbox
        .get("mounts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let allowlist = sandbox
        .get("mountAllowlist")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut mount_lines: Vec<String> = Vec::new();
    for m in mounts.iter().take(MAX_LIST_LINES) {
        let host = m.get("hostDir").and_then(|v| v.as_str()).unwrap_or("");
        let guest = m.get("guestDir").and_then(|v| v.as_str()).unwrap_or("");
        let mode = m.get("mode").and_then(|v| v.as_str()).unwrap_or("");
        let lhs = truncate_middle(host, 42);
        let rhs = truncate_middle(guest, 36);
        let line = if mode.is_empty() {
            format!(
                "▸ <code>{}</code> → <code>{}</code>",
                escape_html_simple(&lhs),
                escape_html_simple(&rhs)
            )
        } else {
            format!(
                "▸ <code>{}</code> → <code>{}</code> <i>({})</i>",
                escape_html_simple(&lhs),
                escape_html_simple(&rhs),
                escape_html_simple(mode)
            )
        };
        mount_lines.push(line);
    }
    if mounts.len() > MAX_LIST_LINES {
        mount_lines.push(format!(
            "<i>… (+{} more)</i>",
            mounts.len() - MAX_LIST_LINES
        ));
    }
    if mount_lines.is_empty() {
        mount_lines.push("<i>none</i>".to_string());
    }

    // Tokens.
    let last = token_debug.get("lastRequest").cloned().unwrap_or_default();
    let next = token_debug.get("nextRequest").cloned().unwrap_or_default();

    let last_in = last.get("inputTokens").and_then(|v| v.as_u64());
    let last_out = last.get("outputTokens").and_then(|v| v.as_u64());
    let last_cached = last.get("cachedTokens").and_then(|v| v.as_u64());

    let cw = next.get("contextWindow").and_then(|v| v.as_u64());
    let planned_out = next.get("plannedMaxOutputToks").and_then(|v| v.as_u64());
    let max_in = next.get("maxInputToks").and_then(|v| v.as_u64());
    let compact_thred = next.get("autoCompactToksThred").and_then(|v| v.as_u64());
    let prompt_est = next.get("promptInputToksEst").and_then(|v| v.as_u64());
    let compact_progress = next.get("compactProgress").and_then(|v| v.as_f64());
    let method = next
        .get("details")
        .and_then(|v| v.get("method"))
        .and_then(|v| v.as_str())
        .unwrap_or("heuristic");

    let pct = compact_progress
        .map(|p| (p * 100.0).round() as i64)
        .filter(|p| *p >= 0);

    let tokens_html = format!(
        "\
<b>🧮 Tokens</b>
Last (authoritative): in={} out={} cached={}
Next (estimate, method={}): prompt={} · threshold={} · progress={}%
<i>Note: Telegram has no draftText, so pending user tokens are assumed 0.</i>",
        escape_html_simple(&format_opt_u64(last_in)),
        escape_html_simple(&format_opt_u64(last_out)),
        escape_html_simple(&format_opt_u64(last_cached)),
        escape_html_simple(method),
        escape_html_simple(&format_opt_u64(prompt_est)),
        escape_html_simple(&format_opt_u64(compact_thred)),
        pct.map(|v| v.to_string())
            .unwrap_or_else(|| "—".to_string()),
    );

    let limits_line = format!(
        "<i>Limits:</i> cw={} planned_max_output={} max_input={}",
        escape_html_simple(&format_opt_u64(cw)),
        escape_html_simple(&format_opt_u64(planned_out)),
        escape_html_simple(&format_opt_u64(max_in)),
    );

    // Skills/plugins: list only names (avoid huge payloads).
    let skills = payload
        .get("skills")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut skill_names: Vec<String> = skills
        .iter()
        .filter_map(|s| {
            s.get("name")
                .and_then(|v| v.as_str())
                .map(|n| n.to_string())
        })
        .collect();
    skill_names.sort();
    let skills_line = if skill_names.is_empty() {
        "<i>none</i>".to_string()
    } else {
        let shown: Vec<String> = skill_names
            .iter()
            .take(MAX_LIST_LINES)
            .map(|s| escape_html_simple(s))
            .collect();
        let mut out = shown.join(", ");
        if skill_names.len() > MAX_LIST_LINES {
            out.push_str(&format!(
                " <i>(+{} more)</i>",
                skill_names.len() - MAX_LIST_LINES
            ));
        }
        out
    };

    let overrides_block = if overrides_lines.is_empty() {
        "<i>none</i>".to_string()
    } else {
        overrides_lines
            .into_iter()
            .map(|l| format!("▸ {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let sandbox_icon = if sandbox_enabled {
        "🟢"
    } else {
        "⚫"
    };
    let sandbox_details = if sandbox_enabled {
        let mut parts = Vec::new();
        if !sandbox_backend.is_empty() {
            parts.push(escape_html_simple(sandbox_backend));
        }
        if !sandbox_image.is_empty() {
            parts.push(format!(
                "<code>{}</code>",
                escape_html_simple(&truncate_middle(sandbox_image, 48))
            ));
        }
        if !external_mounts_status.is_empty() {
            parts.push(format!(
                "externalMounts=<code>{}</code>",
                escape_html_simple(external_mounts_status)
            ));
        }
        if parts.is_empty() {
            "on".to_string()
        } else {
            format!("on · {}", parts.join(" · "))
        }
    } else {
        "off".to_string()
    };

    let group_scope_note = match chat_type {
        ChatType::Group => format!(
            "line_start_dispatch=<code>{}</code> · reply_dispatch=<code>{}</code> · record=<code>on</code>",
            if config.group_line_start_mention_dispatch {
                "on"
            } else {
                "off"
            },
            if config.group_reply_to_dispatch {
                "on"
            } else {
                "off"
            },
        ),
        _ => "<i>(n/a)</i>".to_string(),
    };

    let html_full = format!(
        "\
<b>📋 Session Context</b>

<blockquote><b>Session</b>
<code>{}</code>
messages: {}

<b>🤖 Model</b>
{} · <code>{}</code>

<b>👥 Group</b>
{}

<b>🧩 Plugins</b>
{}

<b>🧠 LLM overrides</b>
{}

<b>🧱 Compaction</b>
compacted: {} · summary_len: {} · kept_msgs: {} · keep_last_user_rounds: {}

<b>{} Sandbox</b>
{}
allowlist: {} · mounts: {}
{}
</blockquote>

<blockquote>{}
{}</blockquote>",
        escape_html_simple(session_key),
        escape_html_simple(&format_opt_u64(msg_count)),
        escape_html_simple(provider),
        escape_html_simple(model),
        group_scope_note,
        skills_line,
        overrides_block,
        escape_html_simple(format_bool(is_compacted)),
        escape_html_simple(&format_opt_u64(summary_len)),
        escape_html_simple(&format_opt_u64(kept_count)),
        escape_html_simple(&format_opt_u64(keep_rounds)),
        sandbox_icon,
        sandbox_details,
        escape_html_simple(&allowlist.len().to_string()),
        escape_html_simple(&mounts.len().to_string()),
        mount_lines.join("\n"),
        tokens_html,
        limits_line,
    );

    let html_fallback = format!(
        "\
<b>📋 Session Context</b>

<code>{}</code>
{} · <code>{}</code>

<i>Output too long for Telegram. Use the Web UI /context for full details.</i>",
        escape_html_simple(session_key),
        escape_html_simple(provider),
        escape_html_simple(model),
    );

    clamp_html_len(html_full, MAX_HTML_LEN, html_fallback)
}

/// Send model selection as an inline keyboard.
///
/// If the response starts with `providers:`, show a provider picker first.
/// Otherwise show the model list directly.
async fn send_model_keyboard(
    bot: &Bot,
    reply_target: &ChannelReplyTarget,
    text: &str,
) -> anyhow::Result<()> {
    let chat = parse_chat_id(&reply_target.chat_id)?;

    let is_provider_list = text.starts_with("providers:");

    let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "providers:" {
            continue;
        }
        if let Some(dot_pos) = trimmed.find(". ")
            && let Ok(n) = trimmed[..dot_pos].parse::<usize>()
        {
            let label_part = &trimmed[dot_pos + 2..];
            let is_active = label_part.ends_with('*');
            let clean = label_part.trim_end_matches('*').trim();
            let display = if is_active {
                format!("● {clean}")
            } else {
                format!("○ {clean}")
            };

            if is_provider_list {
                // Extract provider name (before the parenthesized count).
                let provider_name = clean.rfind(" (").map(|i| &clean[..i]).unwrap_or(clean);
                buttons.push(vec![InlineKeyboardButton::callback(
                    display,
                    with_callback_sender_hint(
                        format!("model_provider:{provider_name}"),
                        reply_target,
                    ),
                )]);
            } else {
                buttons.push(vec![InlineKeyboardButton::callback(
                    display,
                    with_callback_sender_hint(format!("model_switch:{n}"), reply_target),
                )]);
            }
        }
    }

    if buttons.is_empty() {
        bot.send_message(chat, "No models available.").await?;
        return Ok(());
    }

    let heading = if is_provider_list {
        "🤖 Select a provider:"
    } else {
        "🤖 Select a model:"
    };

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let sent = bot
        .send_message(chat, heading)
        .reply_markup(keyboard)
        .await?;
    remember_callback_bucket_binding(reply_target, sent.id.0);
    Ok(())
}

/// Send sandbox status with toggle button and image picker.
///
/// First line is `status:on` or `status:off`. Remaining lines are numbered
/// images, with `*` marking the current one.
async fn send_sandbox_keyboard(
    bot: &Bot,
    reply_target: &ChannelReplyTarget,
    text: &str,
) -> anyhow::Result<()> {
    let chat = parse_chat_id(&reply_target.chat_id)?;

    let mut is_on = false;
    let mut image_buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(status) = trimmed.strip_prefix("status:") {
            is_on = status == "on";
            continue;
        }
        if let Some(dot_pos) = trimmed.find(". ")
            && let Ok(n) = trimmed[..dot_pos].parse::<usize>()
        {
            let label_part = &trimmed[dot_pos + 2..];
            let is_active = label_part.ends_with('*');
            let clean = label_part.trim_end_matches('*').trim();
            let display = if is_active {
                format!("● {clean}")
            } else {
                format!("○ {clean}")
            };
            image_buttons.push(vec![InlineKeyboardButton::callback(
                display,
                with_callback_sender_hint(format!("sandbox_image:{n}"), reply_target),
            )]);
        }
    }

    // Toggle button at the top.
    let toggle_label = if is_on {
        "🟢 Sandbox ON — tap to disable"
    } else {
        "⚫ Sandbox OFF — tap to enable"
    };
    let toggle_action = if is_on {
        with_callback_sender_hint("sandbox_toggle:off".to_string(), reply_target)
    } else {
        with_callback_sender_hint("sandbox_toggle:on".to_string(), reply_target)
    };

    let mut buttons = vec![vec![InlineKeyboardButton::callback(
        toggle_label.to_string(),
        toggle_action,
    )]];
    buttons.extend(image_buttons);

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let sent = bot
        .send_message(chat, "⚙️ Sandbox settings:")
        .reply_markup(keyboard)
        .await?;
    remember_callback_bucket_binding(reply_target, sent.id.0);
    Ok(())
}

fn escape_html_simple(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Handle a Telegram callback query (inline keyboard button press).
pub async fn handle_callback_query(
    query: CallbackQuery,
    bot: &Bot,
    account_handle: &str,
    accounts: &AccountStateMap,
) -> anyhow::Result<()> {
    let callback_id = query.id.clone();
    let mut answer_retryable_reason_code = None;
    if let Err(e) = bot.answer_callback_query(&callback_id).await {
        let reason_code = match &e {
            teloxide::RequestError::Network(err) if err.is_timeout() || err.is_connect() => {
                "transport_failed_before_send"
            },
            teloxide::RequestError::Api(teloxide::ApiError::InvalidQueryId) => "invalid_query_id",
            teloxide::RequestError::Network(_) => "unknown_outcome",
            _ => "answer_failed",
        };
        warn!(
            event = "telegram.callback.answer_failed",
            account_handle,
            callback_id = %callback_id,
            reason_code,
            "failed to answer callback query"
        );
        if reason_code == "transport_failed_before_send" {
            answer_retryable_reason_code = Some(reason_code);
        }
    }

    if let Some(reason_code) = answer_retryable_reason_code {
        return Err(RetryableUpdateError { reason_code }.into());
    }

    let raw_data = match query.data {
        Some(ref d) => d.as_str(),
        None => {
            info!(
                event = "telegram.callback.ignored",
                account_handle,
                callback_id = %callback_id,
                reason_code = "no_data",
                "telegram callback ignored (no data)"
            );
            return Ok(());
        },
    };
    let (data, sender_override) = split_callback_sender_hint(raw_data);

    // Determine which command this callback is for.
    let cmd_text = if let Some(n_str) = data.strip_prefix("sessions_switch:") {
        Some(format!("sessions {n_str}"))
    } else if let Some(n_str) = data.strip_prefix("model_switch:") {
        Some(format!("model {n_str}"))
    } else if let Some(val) = data.strip_prefix("sandbox_toggle:") {
        Some(format!("sandbox {val}"))
    } else if let Some(n_str) = data.strip_prefix("sandbox_image:") {
        Some(format!("sandbox image {n_str}"))
    } else if data.starts_with("model_provider:") {
        // Handled separately below — no simple cmd_text.
        None
    } else {
        info!(
            event = "telegram.callback.ignored",
            account_handle,
            callback_id = %callback_id,
            reason_code = "unknown_data",
            "telegram callback ignored (unknown data)"
        );
        return Ok(());
    };

    let chat_id = query
        .message
        .as_ref()
        .map(|m| m.chat().id.0.to_string())
        .unwrap_or_default();

    if chat_id.is_empty() {
        info!(
            event = "telegram.callback.ignored",
            account_handle,
            callback_id = %callback_id,
            reason_code = "chat_id_missing",
            "telegram callback ignored (missing chat_id)"
        );
        return Ok(());
    }

    let (core_bridge, outbound, bot_handle, config) = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = match accts.get(account_handle) {
            Some(s) => s,
            None => {
                info!(
                    event = "telegram.callback.ignored",
                    account_handle,
                    callback_id = %callback_id,
                    reason_code = "account_missing",
                    "telegram callback ignored (account missing)"
                );
                return Ok(());
            },
        };
        (
            state.core_bridge.clone(),
            Arc::clone(&state.outbound),
            state.bot_username.as_deref().map(|u| format!("@{u}")),
            state.config.clone(),
        )
    };

    let reply_target = moltis_channels::ChannelReplyTarget {
        chan_type: ChannelType::Telegram,
        chan_account_key: account_handle.to_string(),
        chan_user_name: bot_handle,
        chat_id: chat_id.clone(),
        message_id: query.message.as_ref().map(|m| m.id().0.to_string()),
        thread_id: query.message.as_ref().and_then(|message| {
            message
                .regular_message()
                .and_then(|message| message.thread_id.map(|thread_id| thread_id.to_string()))
        }),
        bucket_key: resolve_callback_bucket_key(&config, account_handle, &query, sender_override),
    };
    let follow_up_target = TgFollowUpTarget {
        route: TgRoute {
            peer: chat_id.clone(),
            sender: sender_override
                .map(str::to_string)
                .or_else(|| Some(query.from.id.0.to_string())),
            bucket_key: reply_target.bucket_key.clone().unwrap_or_default(),
            addressed: true,
        },
        private_target: TgPrivateTarget {
            account_handle: account_handle.to_string(),
            chat_id: chat_id.clone(),
            message_id: reply_target.message_id.clone(),
            thread_id: reply_target.thread_id.clone(),
        },
    };

    // Provider selection → fetch models for that provider and show a new keyboard.
    if let Some(provider_name) = data.strip_prefix("model_provider:") {
        if let Some(ref bridge) = core_bridge {
            let cmd = format!("model provider:{provider_name}");
            let typing_chat_id = chat_id.clone();
            let followup_chat_id = chat_id.clone();
            let followup_target = reply_target.clone();
            run_with_telegram_typing(
                bot.clone(),
                account_handle,
                &typing_chat_id,
                "callback_command_model_provider",
                async move {
                    match bridge.dispatch_command(&cmd, follow_up_target).await {
                        Ok(text) => {
                            if let Err(_e) =
                                send_model_keyboard(&bot, &followup_target, &text).await
                            {
                                warn!(
                                    event = "telegram.helper_send.failed",
                                    account_handle,
                                    reason_code = "send_model_keyboard_failed",
                                    "failed to send model keyboard"
                                );
                            }
                        },
                        Err(_e) => {
                            warn!(
                                event = "telegram.command.failed",
                                account_handle,
                                reason_code = "dispatch_command_failed",
                                "callback dispatch_command failed"
                            );
                            if let Err(_e) = outbound
                                .send_text_silent(
                                    account_handle,
                                    &followup_chat_id,
                                    user_facing_error_message(),
                                    None,
                                )
                                .await
                            {
                                warn!(
                                    event = "telegram.user_feedback.failed",
                                    account_handle,
                                    reason_code = "send_text_failed",
                                    "failed to send callback dispatch_command failure feedback"
                                );
                            }
                        },
                    }
                },
            )
            .await;
        } else {
            info!(
                event = "telegram.callback.ignored",
                account_handle,
                callback_id = %callback_id,
                reason_code = "core_bridge_missing",
                "telegram callback ignored (core bridge missing)"
            );
        }
        return Ok(());
    }

    let Some(cmd_text) = cmd_text else {
        return Ok(());
    };

    if let Some(ref bridge) = core_bridge {
        let reply_to = reply_target.message_id.clone();
        let typing_chat_id = chat_id.clone();
        let followup_chat_id = chat_id.clone();
        run_with_telegram_typing(
            bot.clone(),
            account_handle,
            &typing_chat_id,
            "callback_command",
            async move {
                let response = match bridge.dispatch_command(&cmd_text, follow_up_target).await {
                    Ok(msg) => msg,
                    Err(_e) => {
                        warn!(
                            event = "telegram.command.failed",
                            account_handle,
                            reason_code = "dispatch_command_failed",
                            command = cmd_text,
                            "callback dispatch_command failed"
                        );
                        user_facing_error_message().to_string()
                    },
                };
                if let Err(_e) = outbound
                    .send_text_silent(
                        account_handle,
                        &followup_chat_id,
                        &response,
                        reply_to.as_deref(),
                    )
                    .await
                {
                    warn!(
                        event = "telegram.user_feedback.failed",
                        account_handle,
                        reason_code = "send_text_failed",
                        "failed to send callback response"
                    );
                }
            },
        )
        .await;
    } else {
        info!(
            event = "telegram.callback.ignored",
            account_handle,
            callback_id = %callback_id,
            reason_code = "core_bridge_missing",
            "telegram callback ignored (core bridge missing)"
        );
    }

    Ok(())
}

/// Extract text content from a message.
fn extract_text(msg: &Message) -> Option<String> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(t) => Some(t.text.clone()),
            MediaKind::Photo(p) => p.caption.clone(),
            MediaKind::Document(d) => d.caption.clone(),
            MediaKind::Audio(a) => a.caption.clone(),
            MediaKind::Voice(v) => v.caption.clone(),
            MediaKind::Video(vid) => vid.caption.clone(),
            MediaKind::Animation(a) => a.caption.clone(),
            _ => None,
        },
        _ => None,
    }
}

/// Check if the message contains media (photo, document, etc.).
fn has_media(msg: &Message) -> bool {
    match &msg.kind {
        MessageKind::Common(common) => !matches!(common.media_kind, MediaKind::Text(_)),
        _ => false,
    }
}

fn unsupported_media_kind(msg: &Message) -> Option<&'static str> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Document(_) => Some("document"),
            MediaKind::Video(_) => Some("video"),
            MediaKind::VideoNote(_) => Some("video_note"),
            MediaKind::Sticker(_) => Some("sticker"),
            MediaKind::Animation(_) => Some("animation"),
            MediaKind::Poll(_) => Some("poll"),
            MediaKind::Contact(_) => Some("contact"),
            MediaKind::Game(_) => Some("game"),
            MediaKind::Venue(_) => Some("venue"),
            _ => None,
        },
        _ => None,
    }
}

/// Extract a file ID reference from a message for later download.
#[allow(dead_code)]
fn extract_media_url(msg: &Message) -> Option<String> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Photo(p) => p.photo.last().map(|ps| format!("tg://file/{}", ps.file.id)),
            MediaKind::Document(d) => Some(format!("tg://file/{}", d.document.file.id)),
            MediaKind::Audio(a) => Some(format!("tg://file/{}", a.audio.file.id)),
            MediaKind::Voice(v) => Some(format!("tg://file/{}", v.voice.file.id)),
            MediaKind::Sticker(s) => Some(format!("tg://file/{}", s.sticker.file.id)),
            _ => None,
        },
        _ => None,
    }
}

/// Voice/audio file info for transcription.
struct VoiceFileInfo {
    file_id: String,
    /// Format hint: "ogg" for voice messages, "mp3"/"m4a" for audio files
    format: String,
}

/// Extract voice or audio file info from a message.
fn extract_voice_file(msg: &Message) -> Option<VoiceFileInfo> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Voice(v) => Some(VoiceFileInfo {
                file_id: v.voice.file.id.clone(),
                format: "ogg".to_string(), // Telegram voice messages are OGG Opus
            }),
            MediaKind::Audio(a) => {
                // Audio files can be various formats, try to detect from mime_type
                let format = a
                    .audio
                    .mime_type
                    .as_ref()
                    .map(|m| {
                        match m.as_ref() {
                            "audio/mpeg" | "audio/mp3" => "mp3",
                            "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
                            "audio/ogg" | "audio/opus" => "ogg",
                            "audio/wav" | "audio/x-wav" => "wav",
                            "audio/webm" => "webm",
                            _ => "mp3", // Default fallback
                        }
                    })
                    .unwrap_or("mp3")
                    .to_string();
                Some(VoiceFileInfo {
                    file_id: a.audio.file.id.clone(),
                    format,
                })
            },
            _ => None,
        },
        _ => None,
    }
}

/// Photo file info for vision.
struct PhotoFileInfo {
    file_id: String,
    /// MIME type for the image (e.g., "image/jpeg").
    media_type: String,
}

/// Extract photo file info from a message.
/// Returns the largest photo size for best quality.
fn extract_photo_file(msg: &Message) -> Option<PhotoFileInfo> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Photo(p) => {
                // Get the largest photo size (last in the array)
                p.photo.last().map(|ps| PhotoFileInfo {
                    file_id: ps.file.id.clone(),
                    media_type: "image/jpeg".to_string(), // Telegram photos are JPEG
                })
            },
            _ => None,
        },
        _ => None,
    }
}

/// Extracted location info from a Telegram message.
struct LocationInfo {
    latitude: f64,
    longitude: f64,
    /// Whether this is a live location share (has `live_period` set).
    is_live: bool,
}

/// Extract location coordinates from a message.
fn extract_location(msg: &Message) -> Option<LocationInfo> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Location(loc) => Some(LocationInfo {
                latitude: loc.location.latitude,
                longitude: loc.location.longitude,
                is_live: loc.location.live_period.is_some(),
            }),
            _ => None,
        },
        _ => None,
    }
}

/// Describe a media kind for logging purposes.
fn describe_media_kind(msg: &Message) -> Option<&'static str> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(_) => None,
            MediaKind::Animation(_) => Some("animation/GIF"),
            MediaKind::Audio(_) => Some("audio"),
            MediaKind::Contact(_) => Some("contact"),
            MediaKind::Document(_) => Some("document"),
            MediaKind::Game(_) => Some("game"),
            MediaKind::Location(_) => Some("location"),
            MediaKind::Photo(_) => Some("photo"),
            MediaKind::Poll(_) => Some("poll"),
            MediaKind::Sticker(_) => Some("sticker"),
            MediaKind::Venue(_) => Some("venue"),
            MediaKind::Video(_) => Some("video"),
            MediaKind::VideoNote(_) => Some("video note"),
            MediaKind::Voice(_) => Some("voice"),
            _ => Some("unknown media"),
        },
        _ => None,
    }
}

fn message_kind(msg: &Message) -> Option<ChannelMessageKind> {
    match &msg.kind {
        MessageKind::Common(common) => Some(common.media_kind.to_channel_message_kind()),
        _ => None,
    }
}

trait ToChannelMessageKind {
    fn to_channel_message_kind(&self) -> ChannelMessageKind;
}

impl ToChannelMessageKind for MediaKind {
    fn to_channel_message_kind(&self) -> ChannelMessageKind {
        match self {
            MediaKind::Text(_) => ChannelMessageKind::Text,
            MediaKind::Voice(_) => ChannelMessageKind::Voice,
            MediaKind::Audio(_) => ChannelMessageKind::Audio,
            MediaKind::Photo(_) => ChannelMessageKind::Photo,
            MediaKind::Document(_) => ChannelMessageKind::Document,
            MediaKind::Video(_) | MediaKind::VideoNote(_) => ChannelMessageKind::Video,
            MediaKind::Location(_) => ChannelMessageKind::Location,
            _ => ChannelMessageKind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TelegramDownloadError {
    reason_code: &'static str,
    retryable: bool,
}

impl std::fmt::Display for TelegramDownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "telegram download failed ({})", self.reason_code)
    }
}

impl std::error::Error for TelegramDownloadError {}

/// Download a file from Telegram by file ID.
///
/// Notes:
/// - Must inherit the bot's configured API URL / client (supports `set_api_url` tests/self-hosted).
/// - Must not construct or log raw tokenized file URLs.
/// - Must enforce basic timeout and size limits to avoid unbounded memory growth.
async fn download_telegram_file(
    bot: &Bot,
    file_id: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, TelegramDownloadError> {
    use futures::StreamExt as _;
    use teloxide::net::Download as _;

    let file = bot.get_file(file_id).await.map_err(|e| match &e {
        teloxide::RequestError::Api(_) => TelegramDownloadError {
            reason_code: "get_file_api_failed",
            retryable: false,
        },
        teloxide::RequestError::Network(err) if err.is_timeout() => TelegramDownloadError {
            reason_code: "timeout",
            retryable: true,
        },
        teloxide::RequestError::Network(err) if err.is_connect() => TelegramDownloadError {
            reason_code: "network",
            retryable: true,
        },
        teloxide::RequestError::Network(_) => TelegramDownloadError {
            reason_code: "network",
            retryable: true,
        },
        _ => TelegramDownloadError {
            reason_code: "get_file_failed",
            retryable: false,
        },
    })?;

    if (file.size as usize) > max_bytes {
        return Err(TelegramDownloadError {
            reason_code: "file_too_large",
            retryable: false,
        });
    }

    let mut stream = bot.download_file_stream(&file.path);
    let mut data: Vec<u8> = Vec::new();
    let download = async {
        while let Some(next) = stream.next().await {
            let chunk = next.map_err(|e| TelegramDownloadError {
                reason_code: if e.is_timeout() {
                    "timeout"
                } else {
                    "network"
                },
                retryable: e.is_timeout() || e.is_connect(),
            })?;
            if data.len().saturating_add(chunk.len()) > max_bytes {
                return Err(TelegramDownloadError {
                    reason_code: "file_too_large",
                    retryable: false,
                });
            }
            data.extend_from_slice(&chunk);
        }
        Ok::<(), TelegramDownloadError>(())
    };

    tokio::time::timeout(std::time::Duration::from_secs(45), download)
        .await
        .map_err(|_| TelegramDownloadError {
            reason_code: "timeout",
            retryable: true,
        })??;

    Ok(data)
}

/// Classify the chat type.
fn classify_chat(msg: &Message) -> (ChatType, Option<String>) {
    match msg.chat.kind {
        teloxide::types::ChatKind::Private(_) => (ChatType::Dm, None),
        teloxide::types::ChatKind::Public(ref p) => {
            let group_id = msg.chat.id.0.to_string();
            match p.kind {
                teloxide::types::PublicChatKind::Channel(_) => (ChatType::Channel, Some(group_id)),
                _ => (ChatType::Group, Some(group_id)),
            }
        },
    }
}

fn message_thread_id_text(msg: &Message) -> Option<String> {
    msg.thread_id.map(|thread_id| thread_id.to_string())
}

fn reply_to_message_id_text(msg: &Message) -> Option<String> {
    msg.reply_to_message().map(|reply| reply.id.0.to_string())
}

fn tg_bucket_key_for_route(
    config: &crate::config::TelegramAccountConfig,
    account_key: Option<&str>,
    chat_type: &ChatType,
    peer: &str,
    sender: Option<&str>,
    branch: Option<&str>,
) -> String {
    match chat_type {
        ChatType::Dm => resolve_dm_bucket_key(&config.dm_scope, account_key, peer),
        ChatType::Group | ChatType::Channel => {
            resolve_group_bucket_key(&config.group_scope, account_key, peer, sender, branch)
        },
    }
}

fn log_follow_up_route_degrade(
    config: &crate::config::TelegramAccountConfig,
    account_handle: &str,
    chat_id: &str,
    message_id: Option<i32>,
    chat_type: &ChatType,
    sender: Option<&str>,
    thread_id: Option<&str>,
    source: &'static str,
) {
    use crate::config::GroupScope;

    if !matches!(chat_type, ChatType::Group | ChatType::Channel) {
        return;
    }

    let needs_sender = matches!(
        config.group_scope,
        GroupScope::PerSender | GroupScope::PerBranchSender
    );
    let needs_branch = matches!(
        config.group_scope,
        GroupScope::PerBranch | GroupScope::PerBranchSender
    );

    if needs_sender && sender.is_none() {
        info!(
            event = "telegram.route.degraded",
            source,
            account_handle,
            chat_id,
            message_id,
            reason_code = "sender_missing",
            "telegram follow-up route degraded because sender was missing"
        );
    }

    if needs_branch && thread_id.is_none() {
        info!(
            event = "telegram.route.degraded",
            source,
            account_handle,
            chat_id,
            message_id,
            reason_code = "branch_missing",
            "telegram follow-up route degraded because branch was missing"
        );
    }
}

fn tg_peer_for_message(
    chat_type: &ChatType,
    msg: &Message,
    identity_links: &[crate::config::TelegramIdentityLink],
) -> Option<String> {
    match chat_type {
        ChatType::Dm => crate::adapter::resolve_person_or_tguser_key(
            identity_links,
            msg.from.as_ref().map(|user| user.id.0 as u64),
            msg.from.as_ref().and_then(|user| user.username.as_deref()),
        ),
        ChatType::Group | ChatType::Channel => crate::adapter::tgchat_key(&msg.chat.id.0.to_string()),
    }
}

fn tg_sender_for_message(
    msg: &Message,
    identity_links: &[crate::config::TelegramIdentityLink],
) -> Option<String> {
    crate::adapter::resolve_person_or_tguser_key(
        identity_links,
        msg.from.as_ref().map(|user| user.id.0 as u64),
        msg.from.as_ref().and_then(|user| user.username.as_deref()),
    )
}

#[cfg(test)]
fn tg_bucket_key_for_callback_query(
    config: &crate::config::TelegramAccountConfig,
    _account_handle: &str,
    query: &CallbackQuery,
) -> Option<String> {
    let message = query.message.as_ref()?;
    let chat = message.chat();
    let (chat_type, peer) = match &chat.kind {
        teloxide::types::ChatKind::Private(_) => {
            (ChatType::Dm, crate::adapter::tguser_key(query.from.id.0 as u64))
        },
        teloxide::types::ChatKind::Public(_) => (
            ChatType::Group,
            crate::adapter::tgchat_key(&chat.id.0.to_string())?,
        ),
    };
    let branch = crate::adapter::resolve_branch_key(
        message
            .regular_message()
            .and_then(|message| message.thread_id.map(|thread_id| thread_id.to_string()))
            .as_deref(),
        None,
    );
    let sender = crate::adapter::tguser_key(query.from.id.0 as u64);
    Some(tg_bucket_key_for_route(
        config,
        crate::adapter::account_key_from_config(config).as_deref(),
        &chat_type,
        &peer,
        Some(sender.as_str()),
        branch.as_deref(),
    ))
}

fn resolve_callback_bucket_key(
    config: &crate::config::TelegramAccountConfig,
    account_handle: &str,
    query: &CallbackQuery,
    sender_override: Option<&str>,
) -> Option<String> {
    use crate::config::GroupScope;

    let message = query.message.as_ref()?;
    let chat = message.chat();
    let chat_id = chat.id.0.to_string();
    let message_id = message.id().0;
    if let Some(bucket_key) = lookup_callback_bucket_binding(account_handle, &chat_id, message_id) {
        return Some(bucket_key);
    }

    if matches!(chat.kind, teloxide::types::ChatKind::Public(_))
        && !matches!(config.group_scope, GroupScope::Group)
        && sender_override.is_none()
    {
        warn!(
            event = "telegram.callback.bucket_binding.missing",
            account_handle,
            chat_id,
            message_id,
            reason_code = "callback_bucket_binding_missing",
            "telegram callback bucket binding missing; falling back to route-derived bucket"
        );
    }

    let (chat_type, sender, branch) = match &chat.kind {
        teloxide::types::ChatKind::Private(_) => (
            ChatType::Dm,
            Some(crate::adapter::tguser_key(query.from.id.0 as u64)),
            None,
        ),
        teloxide::types::ChatKind::Public(_) => (
            ChatType::Group,
            Some(
                sender_override
                    .unwrap_or(crate::adapter::tguser_key(query.from.id.0 as u64).as_str())
                    .to_string(),
            ),
            crate::adapter::resolve_branch_key(
                message
                    .regular_message()
                    .and_then(|message| message.thread_id.map(|thread_id| thread_id.to_string()))
                    .as_deref(),
                None,
            ),
        ),
    };
    log_follow_up_route_degrade(
        config,
        account_handle,
        &chat_id,
        Some(message_id),
        &chat_type,
        sender.as_deref(),
        branch.as_deref(),
        "callback_query",
    );

    let sender = sender.as_deref().unwrap_or_default();
    Some(tg_bucket_key_for_route(
        config,
        crate::adapter::account_key_from_config(config).as_deref(),
        &chat_type,
        &match chat_type {
            ChatType::Dm => crate::adapter::tguser_key(query.from.id.0 as u64),
            ChatType::Group | ChatType::Channel => crate::adapter::tgchat_key(&chat_id)?,
        },
        Some(sender),
        branch.as_deref(),
    ))
}

fn build_tg_inbound(
    config: &crate::config::TelegramAccountConfig,
    account_handle: &str,
    chat_type: &ChatType,
    mode: TgInboundMode,
    body: &str,
    has_attachments: bool,
    has_location: bool,
    msg: &Message,
    addressed: bool,
    bot_user_id: Option<u64>,
    identity_links: &[crate::config::TelegramIdentityLink],
) -> TgInbound {
    let kind = match chat_type {
        ChatType::Dm => TgInboundKind::Dm,
        ChatType::Group | ChatType::Channel => TgInboundKind::Group,
    };
    let inbound = TgInbound {
        kind,
        mode,
        body: TgContent {
            text: body.to_string(),
            has_attachments,
            has_location,
        },
        private_source: TgPrivateSource {
            account_handle: account_handle.to_string(),
            account_key: bot_user_id.map(crate::adapter::tguser_key),
            chat_id: msg.chat.id.0.to_string(),
            message_id: Some(msg.id.0.to_string()),
            thread_id: message_thread_id_text(msg),
            reply_to_message_id: reply_to_message_id_text(msg),
            peer: tg_peer_for_message(chat_type, msg, identity_links).unwrap_or_default(),
            sender: tg_sender_for_message(msg, identity_links),
            addressed,
        },
    };
    log_route_degrade(config, &inbound);
    inbound
}

fn build_tg_private_target(account_handle: &str, msg: &Message) -> TgPrivateTarget {
    TgPrivateTarget {
        account_handle: account_handle.to_string(),
        chat_id: msg.chat.id.0.to_string(),
        message_id: Some(msg.id.0.to_string()),
        thread_id: message_thread_id_text(msg),
    }
}

fn build_tg_inbound_request(
    inbound: &TgInbound,
    route: TgRoute,
    private_target: TgPrivateTarget,
    managed_snapshots: &[TelegramBusAccountSnapshot],
    identity_links: &[crate::config::TelegramIdentityLink],
    sender_name: Option<String>,
    username: Option<String>,
    sender_id: Option<u64>,
    sender_is_bot: bool,
    message_kind: Option<ChannelMessageKind>,
    model: Option<String>,
    attachments: Vec<TgAttachment>,
) -> TgInboundRequest {
    let transcript_format = TgTranscriptFormat::TgGstV1;
    let addressed = route.addressed;
    let mut inbound = inbound.clone();
    if inbound.kind == TgInboundKind::Group && transcript_format == TgTranscriptFormat::TgGstV1 {
        let rendered = crate::adapter::tg_gst_v1_render_text(
            inbound.body.text.as_str(),
            message_kind,
            managed_snapshots,
            identity_links,
            username.as_deref(),
            sender_id,
            sender_name.as_deref(),
            sender_is_bot,
            addressed,
        );
        if rendered.degraded || rendered.disambiguated {
            info!(
                event = "telegram.speaker_resolution",
                account_handle = inbound.private_source.account_handle,
                reason_code = rendered.reason_code,
                decision = if rendered.degraded {
                    "degraded"
                } else {
                    "disambiguated"
                },
                policy = "tg_gst_v1_speaker",
                match_method = rendered.match_method,
                collision = rendered.disambiguated,
                sender_short_id = sender_id.map(|id| id % 100000),
                "telegram speaker rendering required fallback or disambiguation"
            );
        }
        inbound.body.text = rendered.text;
    }

    TgInboundRequest {
        inbound,
        route,
        private_target,
        transcript_format,
        sender_name,
        username,
        sender_id,
        sender_is_bot,
        model,
        message_kind,
        attachments,
    }
}

fn build_channel_reply_target(
    account_handle: &str,
    bot_handle: Option<String>,
    msg: &Message,
    route: &crate::adapter::TgRoute,
) -> ChannelReplyTarget {
    ChannelReplyTarget {
        chan_type: ChannelType::Telegram,
        chan_account_key: account_handle.to_string(),
        chan_user_name: bot_handle,
        chat_id: msg.chat.id.0.to_string(),
        message_id: Some(msg.id.0.to_string()),
        thread_id: message_thread_id_text(msg),
        bucket_key: Some(route.bucket_key.clone()),
    }
}

fn log_route_degrade(config: &crate::config::TelegramAccountConfig, inbound: &TgInbound) {
    use crate::config::GroupScope;

    if inbound.kind != TgInboundKind::Group {
        return;
    }

    let needs_sender = matches!(
        config.group_scope,
        GroupScope::PerSender | GroupScope::PerBranchSender
    );
    let needs_branch = matches!(
        config.group_scope,
        GroupScope::PerBranch | GroupScope::PerBranchSender
    );

    if needs_sender && inbound.private_source.sender.is_none() {
        info!(
            event = "telegram.route.degraded",
            account_handle = inbound.private_source.account_handle,
            chat_id = inbound.private_source.chat_id,
            message_id = ?inbound.private_source.message_id,
            reason_code = "sender_missing",
            "telegram route degraded because sender was missing"
        );
    }

    if needs_branch && inbound.private_source.thread_id.is_none() {
        info!(
            event = "telegram.route.degraded",
            account_handle = inbound.private_source.account_handle,
            chat_id = inbound.private_source.chat_id,
            message_id = ?inbound.private_source.message_id,
            reason_code = "branch_missing",
            "telegram route degraded because branch was missing"
        );
    }
}

/// Check if the bot was @mentioned in the message.
fn normalize_username(username: &str) -> String {
    username.trim().trim_start_matches('@').to_lowercase()
}

fn is_tg_username_byte(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

fn is_addressed_command_to_bot(command_entity_text: &str, bot_username_norm: &str) -> bool {
    // Examples:
    // - "/context" -> not addressed
    // - "/context@MyBot" -> addressed to MyBot
    // - "/context@MyBot123" -> addressed, but not to MyBot
    let text = command_entity_text.trim();
    if !text.starts_with('/') {
        return false;
    }
    let Some((_, suffix)) = text.split_once('@') else {
        return false;
    };
    let target = normalize_username(suffix);
    !target.is_empty() && target == bot_username_norm
}

fn entities_trigger_wakeup(
    text: &str,
    entities: &[MessageEntity],
    bot_user_id: Option<UserId>,
    bot_username_norm: &str,
) -> bool {
    if entities.is_empty() {
        return false;
    }
    for ent in MessageEntityRef::parse(text, entities) {
        match ent.kind() {
            MessageEntityKind::Mention => {
                let mention = ent.text();
                if let Some(stripped) = mention.strip_prefix('@') {
                    if normalize_username(stripped) == bot_username_norm {
                        return true;
                    }
                }
            },
            MessageEntityKind::TextMention { user } => {
                if bot_user_id.is_some_and(|id| user.id == id) {
                    return true;
                }
                if user
                    .username
                    .as_deref()
                    .is_some_and(|u| normalize_username(u) == bot_username_norm)
                {
                    return true;
                }
            },
            MessageEntityKind::BotCommand => {
                if is_addressed_command_to_bot(ent.text(), bot_username_norm) {
                    return true;
                }
            },
            _ => {},
        }
    }
    false
}

fn fallback_contains_at_username(text: &str, bot_username_norm: &str) -> bool {
    let needle = bot_username_norm.as_bytes();
    if needle.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && bytes.len() >= i + 1 + needle.len() {
            let start = i + 1;
            let end = start + needle.len();
            if bytes[start..end]
                .iter()
                .zip(needle.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
            {
                if end == bytes.len() || !is_tg_username_byte(bytes[end]) {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn managed_account_snapshots(accounts: &AccountStateMap) -> Vec<TelegramBusAccountSnapshot> {
    let map = accounts.read().unwrap_or_else(|e| e.into_inner());
    map.iter()
        .map(|(account_handle, state)| TelegramBusAccountSnapshot {
            account_handle: account_handle.clone(),
            agent_id: state.config.agent_id.clone(),
            chan_user_id: crate::state::effective_bot_user_id(state),
            chan_user_name: crate::state::effective_bot_username(state),
            chan_nickname: state.config.chan_nickname.clone(),
            dm_scope: state.config.dm_scope.clone(),
            group_scope: state.config.group_scope.clone(),
        })
        .collect()
}

fn resolve_managed_account_handle_for_user(
    accounts: &AccountStateMap,
    user_id: Option<u64>,
    username: Option<&str>,
) -> Option<String> {
    let username_norm = username.map(normalize_username);
    let map = accounts.read().unwrap_or_else(|e| e.into_inner());
    map.iter().find_map(|(account_handle, state)| {
        let id_matches =
            user_id.is_some_and(|value| crate::state::effective_bot_user_id(state) == Some(value));
        let username_matches = username_norm.as_deref().is_some_and(|value| {
            crate::state::effective_bot_username(state)
                .as_deref()
                .map(normalize_username)
                .as_deref()
                == Some(value)
        });
        (id_matches || username_matches).then(|| account_handle.clone())
    })
}

fn resolve_reply_to_target_account_handle(
    accounts: &AccountStateMap,
    msg: &Message,
) -> Option<String> {
    let reply = msg.reply_to_message()?;
    let from_user = reply.from.as_ref();
    let resolved = resolve_managed_account_handle_for_user(
        accounts,
        from_user.map(|user| user.id.0 as u64),
        from_user.and_then(|user| user.username.as_deref()),
    );
    if resolved.is_some() {
        return resolved;
    }
    let binding = crate::state::shared_group_runtime(accounts);
    let mut runtime = binding.lock().unwrap_or_else(|e| e.into_inner());
    runtime.message_author(&msg.chat.id.0.to_string(), &reply.id.0.to_string())
}

fn register_group_runtime_participants(
    accounts: &AccountStateMap,
    account_handle: &str,
    msg: &Message,
) -> Option<String> {
    let chat_id = msg.chat.id.0.to_string();
    let sender_account_handle = resolve_managed_account_handle_for_user(
        accounts,
        msg.from.as_ref().map(|user| user.id.0 as u64),
        msg.from.as_ref().and_then(|user| user.username.as_deref()),
    );
    let binding = crate::state::shared_group_runtime(accounts);
    let mut runtime = binding.lock().unwrap_or_else(|e| e.into_inner());
    runtime.register_participant(&chat_id, account_handle);
    if let Some(ref sender_account_handle) = sender_account_handle {
        runtime.register_participant(&chat_id, &sender_account_handle);
    }
    sender_account_handle
}

fn apply_group_dispatch_fuse(
    accounts: &AccountStateMap,
    target_account_handle: &str,
    msg: &Message,
    managed_sender_account_handle: Option<&str>,
    action: crate::adapter::TgGroupTargetAction,
) -> crate::adapter::TgGroupTargetAction {
    if !matches!(action.mode, TgInboundMode::Dispatch) {
        return action;
    }

    let chat_id = msg.chat.id.0.to_string();
    let message_id = msg.id.0.to_string();
    let thread_id = message_thread_id_text(msg);
    let runtime = crate::state::shared_group_runtime(accounts);
    let mut runtime = runtime.lock().unwrap_or_else(|e| e.into_inner());

    if managed_sender_account_handle.is_none() {
        runtime.ensure_external_root_dispatch(&chat_id, &message_id);
        return action;
    }

    match runtime.admit_managed_dispatch(&chat_id, &message_id) {
        Some(admission) if admission.allowed => action,
        Some(admission) => {
            let source_account_handle = managed_sender_account_handle.unwrap_or_default();
            if admission.first_budget_exceeded {
                warn!(
                    event = "telegram.group.dispatch_fuse",
                    reason_code = "root_dispatch_budget_exceeded",
                    decision = "downgrade_to_record",
                    policy = "group_record_dispatch_v3",
                    root_message_id = admission.root_message_id,
                    used = admission.used,
                    budget = admission.budget,
                    chat_id,
                    thread_id = thread_id.as_deref(),
                    source_account_handle,
                    target_account_handle,
                    message_id,
                    "telegram group inbound dispatch downgraded by root dispatch fuse"
                );
            } else {
                info!(
                    event = "telegram.group.dispatch_fuse",
                    reason_code = "root_dispatch_budget_exceeded",
                    decision = "downgrade_to_record",
                    policy = "group_record_dispatch_v3",
                    root_message_id = admission.root_message_id,
                    used = admission.used,
                    budget = admission.budget,
                    chat_id,
                    thread_id = thread_id.as_deref(),
                    source_account_handle,
                    target_account_handle,
                    message_id,
                    "telegram group inbound dispatch downgraded by root dispatch fuse"
                );
            }
            crate::adapter::TgGroupTargetAction {
                mode: TgInboundMode::RecordOnly,
                ..action
            }
        },
        None => {
            warn!(
                event = "telegram.group.dispatch_fuse",
                reason_code = "root_dispatch_context_missing",
                decision = "downgrade_to_record",
                policy = "group_record_dispatch_v3",
                root_message_id = Option::<&str>::None,
                used = Option::<u32>::None,
                budget = Option::<u32>::None,
                chat_id,
                thread_id = thread_id.as_deref(),
                source_account_handle = managed_sender_account_handle,
                target_account_handle,
                message_id,
                "telegram group inbound dispatch downgraded because root context is missing"
            );
            crate::adapter::TgGroupTargetAction {
                mode: TgInboundMode::RecordOnly,
                ..action
            }
        },
    }
}

fn group_action_dedupe_key(account_handle: &str, chat_id: &str, message_id: &str) -> String {
    format!("telegram.group.action|account:{account_handle}|chat:{chat_id}|message:{message_id}")
}

/// Check if the bot was @mentioned (or otherwise explicitly activated) in the message.
fn check_bot_mentioned(
    msg: &Message,
    bot_user_id: Option<UserId>,
    bot_username: Option<&str>,
) -> bool {
    let Some(bot_username) = bot_username else {
        return false;
    };
    let bot_username_norm = normalize_username(bot_username);
    if bot_username_norm.is_empty() {
        return false;
    }

    // Reply-to-bot activation (optional but recommended for group usability).
    if let Some(bot_id) = bot_user_id
        && msg
            .reply_to_message()
            .and_then(|m| m.from.as_ref())
            .is_some_and(|u| u.id == bot_id)
    {
        return true;
    }

    // Prefer structured entities (text + caption).
    if let MessageKind::Common(common) = &msg.kind {
        match &common.media_kind {
            MediaKind::Text(t) => {
                if entities_trigger_wakeup(&t.text, &t.entities, bot_user_id, &bot_username_norm) {
                    return true;
                }
                if t.entities.is_empty() {
                    return fallback_contains_at_username(&t.text, &bot_username_norm);
                }
                return false;
            },
            MediaKind::Animation(a) => {
                if let Some(caption) = a.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &a.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if a.caption_entities.is_empty() {
                    return a
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            MediaKind::Audio(a) => {
                if let Some(caption) = a.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &a.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if a.caption_entities.is_empty() {
                    return a
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            MediaKind::Document(d) => {
                if let Some(caption) = d.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &d.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if d.caption_entities.is_empty() {
                    return d
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            MediaKind::Photo(p) => {
                if let Some(caption) = p.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &p.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if p.caption_entities.is_empty() {
                    return p
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            MediaKind::Video(v) => {
                if let Some(caption) = v.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &v.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if v.caption_entities.is_empty() {
                    return v
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            MediaKind::Voice(v) => {
                if let Some(caption) = v.caption.as_deref()
                    && entities_trigger_wakeup(
                        caption,
                        &v.caption_entities,
                        bot_user_id,
                        &bot_username_norm,
                    )
                {
                    return true;
                }
                if v.caption_entities.is_empty() {
                    return v
                        .caption
                        .as_deref()
                        .is_some_and(|c| fallback_contains_at_username(c, &bot_username_norm));
                }
                return false;
            },
            _ => {
                // Fallback for kinds we don't parse entities for.
                let text = extract_text(msg).unwrap_or_default();
                return fallback_contains_at_username(&text, &bot_username_norm);
            },
        }
    }

    false
}

fn strip_self_mentions_from_text(
    text: &str,
    entities: &[MessageEntity],
    bot_user_id: Option<UserId>,
    bot_username_norm: &str,
) -> (String, bool) {
    if text.is_empty() || bot_username_norm.is_empty() {
        return (text.to_string(), false);
    }

    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();

    if !entities.is_empty() {
        for ent in MessageEntityRef::parse(text, entities) {
            match ent.kind() {
                MessageEntityKind::Mention => {
                    let mention = ent.text();
                    if let Some(stripped) = mention.strip_prefix('@') {
                        if normalize_username(stripped) == bot_username_norm {
                            ranges.push(ent.range());
                        }
                    }
                },
                MessageEntityKind::TextMention { user } => {
                    if bot_user_id.is_some_and(|id| user.id == id)
                        || user
                            .username
                            .as_deref()
                            .is_some_and(|u| normalize_username(u) == bot_username_norm)
                    {
                        ranges.push(ent.range());
                    }
                },
                MessageEntityKind::BotCommand => {
                    let cmd = ent.text();
                    if is_addressed_command_to_bot(cmd, bot_username_norm)
                        && let Some(at_idx) = cmd.find('@')
                    {
                        let r = ent.range();
                        let start = r.start.saturating_add(at_idx);
                        if start < r.end {
                            ranges.push(start..r.end);
                        }
                    }
                },
                _ => {},
            }
        }
    } else {
        // Fallback only when Telegram provided no entities: boundary-safe match with a
        // stricter prefix check to avoid stripping email addresses.
        let needle = bot_username_norm.as_bytes();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'@' && bytes.len() >= i + 1 + needle.len() {
                if i > 0 && is_tg_username_byte(bytes[i - 1]) {
                    i += 1;
                    continue;
                }
                let start = i + 1;
                let end = start + needle.len();
                if bytes[start..end]
                    .iter()
                    .zip(needle.iter())
                    .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
                {
                    if end == bytes.len() || !is_tg_username_byte(bytes[end]) {
                        ranges.push(i..end);
                        i = end;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }

    if ranges.is_empty() {
        return (text.to_string(), false);
    }

    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.start <= last.end {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        merged.push(r);
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for r in merged {
        if r.start > cursor {
            out.push_str(&text[cursor..r.start]);
        }
        cursor = r.end.min(text.len());
    }
    if cursor < text.len() {
        out.push_str(&text[cursor..]);
    }

    let normalized = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (normalized, true)
}

fn strip_self_mention_from_message(
    msg: &Message,
    body: &str,
    bot_user_id: Option<UserId>,
    bot_username: &str,
) -> (String, bool) {
    let bot_username_norm = normalize_username(bot_username);
    if bot_username_norm.is_empty() || body.is_empty() {
        return (body.to_string(), false);
    }

    let (source_text, source_entities): (&str, &[MessageEntity]) = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(t) => (&t.text, &t.entities),
            MediaKind::Animation(a) => (
                a.caption.as_deref().unwrap_or_default(),
                &a.caption_entities,
            ),
            MediaKind::Audio(a) => (
                a.caption.as_deref().unwrap_or_default(),
                &a.caption_entities,
            ),
            MediaKind::Document(d) => (
                d.caption.as_deref().unwrap_or_default(),
                &d.caption_entities,
            ),
            MediaKind::Photo(p) => (
                p.caption.as_deref().unwrap_or_default(),
                &p.caption_entities,
            ),
            MediaKind::Video(v) => (
                v.caption.as_deref().unwrap_or_default(),
                &v.caption_entities,
            ),
            MediaKind::Voice(v) => (
                v.caption.as_deref().unwrap_or_default(),
                &v.caption_entities,
            ),
            _ => ("", &[]),
        },
        _ => ("", &[]),
    };

    if source_text.is_empty() || !body.starts_with(source_text) {
        return (body.to_string(), false);
    }

    let (stripped_prefix, stripped_any) = strip_self_mentions_from_text(
        source_text,
        source_entities,
        bot_user_id,
        &bot_username_norm,
    );
    if !stripped_any {
        return (body.to_string(), false);
    }

    let rest = &body[source_text.len()..];
    if stripped_prefix.is_empty() {
        return (rest.trim_start().to_string(), true);
    }
    (format!("{stripped_prefix}{rest}"), true)
}

fn tg_gst_v1_is_self_mention_only(
    msg: &Message,
    body: &str,
    bot_user_id: Option<UserId>,
    bot_username: &str,
) -> bool {
    let bot_username_norm = normalize_username(bot_username);
    if bot_username_norm.is_empty() || body.trim().is_empty() {
        return false;
    }

    let (source_text, source_entities): (&str, &[MessageEntity]) = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(t) => (&t.text, &t.entities),
            MediaKind::Animation(a) => (
                a.caption.as_deref().unwrap_or_default(),
                &a.caption_entities,
            ),
            MediaKind::Audio(a) => (
                a.caption.as_deref().unwrap_or_default(),
                &a.caption_entities,
            ),
            MediaKind::Document(d) => (
                d.caption.as_deref().unwrap_or_default(),
                &d.caption_entities,
            ),
            MediaKind::Photo(p) => (
                p.caption.as_deref().unwrap_or_default(),
                &p.caption_entities,
            ),
            MediaKind::Video(v) => (
                v.caption.as_deref().unwrap_or_default(),
                &v.caption_entities,
            ),
            MediaKind::Voice(v) => (
                v.caption.as_deref().unwrap_or_default(),
                &v.caption_entities,
            ),
            _ => ("", &[]),
        },
        _ => ("", &[]),
    };

    if source_text.is_empty() || !body.starts_with(source_text) {
        return false;
    }

    let (without_mentions, stripped) = tg_gst_v1_strip_mentions_for_presence_check(
        source_text,
        source_entities,
        bot_user_id,
        &bot_username_norm,
    );
    if !stripped {
        return false;
    }

    without_mentions.chars().all(|c| c.is_whitespace())
}

fn tg_gst_v1_strip_mentions_for_presence_check(
    text: &str,
    entities: &[MessageEntity],
    bot_user_id: Option<UserId>,
    bot_username_norm: &str,
) -> (String, bool) {
    if text.is_empty() || bot_username_norm.is_empty() {
        return (text.to_string(), false);
    }

    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    if !entities.is_empty() {
        for ent in MessageEntityRef::parse(text, entities) {
            match ent.kind() {
                MessageEntityKind::Mention => {
                    let mention = ent.text();
                    if let Some(stripped) = mention.strip_prefix('@') {
                        if normalize_username(stripped) == bot_username_norm {
                            ranges.push(ent.range());
                        }
                    }
                },
                MessageEntityKind::TextMention { user } => {
                    if bot_user_id.is_some_and(|id| user.id == id)
                        || user
                            .username
                            .as_deref()
                            .is_some_and(|u| normalize_username(u) == bot_username_norm)
                    {
                        ranges.push(ent.range());
                    }
                },
                MessageEntityKind::BotCommand => {
                    let cmd = ent.text();
                    if is_addressed_command_to_bot(cmd, bot_username_norm)
                        && let Some(at_idx) = cmd.find('@')
                    {
                        let r = ent.range();
                        let start = r.start.saturating_add(at_idx);
                        if start < r.end {
                            ranges.push(start..r.end);
                        }
                    }
                },
                _ => {},
            }
        }
    } else {
        // Conservative fallback: if Telegram gave no entities, do not attempt
        // presence detection (avoid email/URL false positives).
        return (text.to_string(), false);
    }

    if ranges.is_empty() {
        return (text.to_string(), false);
    }

    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.start <= last.end {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        merged.push(r);
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for r in merged {
        if r.start > cursor {
            out.push_str(&text[cursor..r.start]);
        }
        cursor = r.end.min(text.len());
    }
    if cursor < text.len() {
        out.push_str(&text[cursor..]);
    }

    (out, true)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{
            collections::HashMap,
            sync::{Arc, Mutex, atomic::Ordering},
        },
    };

    use {
        anyhow::Result,
        async_trait::async_trait,
        axum::{Json, Router, body::Bytes, extract::State, http::Uri, routing::post},
        moltis_channels::{
            ChannelEvent, ChannelEventSink, ChannelInboundContext, ChannelMessageMeta,
            ChannelReplyTarget,
        },
        secrecy::Secret,
        serde::{Deserialize, Serialize},
        serde_json::json,
        tokio::sync::oneshot,
        tokio_util::sync::CancellationToken,
        tracing_subscriber::fmt::MakeWriter,
    };

    use crate::{
        config::{TelegramAccountConfig, TelegramIdentityLink},
        otp::OtpState,
        outbound::TelegramOutbound,
        state::{AccountState, AccountStateMap},
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TelegramApiMethod {
        SendMessage,
        SendChatAction,
        GetFile,
        AnswerCallbackQuery,
        Other(String),
    }

    impl TelegramApiMethod {
        fn from_path(path: &str) -> Self {
            let method = path.rsplit('/').next().unwrap_or_default();
            match method {
                "SendMessage" => Self::SendMessage,
                "SendChatAction" => Self::SendChatAction,
                "GetFile" => Self::GetFile,
                "AnswerCallbackQuery" => Self::AnswerCallbackQuery,
                _ => Self::Other(method.to_string()),
            }
        }
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    enum CapturedTelegramRequest {
        SendMessage(SendMessageRequest),
        SendChatAction(SendChatActionRequest),
        GetFile(GetFileRequest),
        AnswerCallbackQuery(AnswerCallbackQueryRequest),
        FileDownload {
            path: String,
        },
        Other {
            method: TelegramApiMethod,
            raw_body: String,
        },
    }

    #[derive(Debug, Clone, Deserialize)]
    struct SendMessageRequest {
        chat_id: i64,
        text: String,
        #[serde(default)]
        parse_mode: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize)]
    struct SendChatActionRequest {
        chat_id: i64,
        action: String,
    }

    #[derive(Clone, Default)]
    struct SharedLogBuffer {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    struct SharedLogWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.buffer.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter {
                buffer: Arc::clone(&self.buffer),
            }
        }
    }

    fn capture_json_logs<T>(operation: impl FnOnce() -> T) -> (Vec<serde_json::Value>, T) {
        let writer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .without_time()
            .json()
            .finish();
        let result = tracing::subscriber::with_default(subscriber, operation);
        let raw = String::from_utf8(writer.buffer.lock().unwrap().clone()).unwrap();
        let logs = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect();
        (logs, result)
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    struct GetFileRequest {
        file_id: String,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    struct AnswerCallbackQueryRequest {
        callback_query_id: String,
        #[serde(default)]
        text: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct TelegramApiResponse {
        ok: bool,
        result: TelegramApiResult,
    }

    #[derive(Debug, Serialize)]
    #[serde(untagged)]
    enum TelegramApiResult {
        Message(TelegramMessageResult),
        File(TelegramFileResult),
        Bool(bool),
    }

    #[derive(Debug, Serialize)]
    struct TelegramChat {
        id: i64,
        #[serde(rename = "type")]
        chat_type: String,
    }

    #[derive(Debug, Serialize)]
    struct TelegramMessageResult {
        message_id: i64,
        date: i64,
        chat: TelegramChat,
        text: String,
    }

    #[derive(Debug, Serialize)]
    struct TelegramFileResult {
        file_id: String,
        file_unique_id: String,
        file_size: u32,
        file_path: String,
    }

    #[derive(Clone)]
    struct MockTelegramApi {
        requests: Arc<Mutex<Vec<CapturedTelegramRequest>>>,
    }

    async fn telegram_api_handler(
        State(state): State<MockTelegramApi>,
        uri: Uri,
        body: Bytes,
    ) -> Json<TelegramApiResponse> {
        let method = TelegramApiMethod::from_path(uri.path());
        let raw_body = String::from_utf8_lossy(&body).to_string();

        let captured = match method.clone() {
            TelegramApiMethod::SendMessage => {
                match serde_json::from_slice::<SendMessageRequest>(&body) {
                    Ok(req) => CapturedTelegramRequest::SendMessage(req),
                    Err(_) => CapturedTelegramRequest::Other { method, raw_body },
                }
            },
            TelegramApiMethod::SendChatAction => {
                match serde_json::from_slice::<SendChatActionRequest>(&body) {
                    Ok(req) => CapturedTelegramRequest::SendChatAction(req),
                    Err(_) => CapturedTelegramRequest::Other { method, raw_body },
                }
            },
            TelegramApiMethod::GetFile => match serde_json::from_slice::<GetFileRequest>(&body) {
                Ok(req) => CapturedTelegramRequest::GetFile(req),
                Err(_) => CapturedTelegramRequest::Other { method, raw_body },
            },
            TelegramApiMethod::AnswerCallbackQuery => {
                match serde_json::from_slice::<AnswerCallbackQueryRequest>(&body) {
                    Ok(req) => CapturedTelegramRequest::AnswerCallbackQuery(req),
                    Err(_) => CapturedTelegramRequest::Other { method, raw_body },
                }
            },
            TelegramApiMethod::Other(_) => CapturedTelegramRequest::Other { method, raw_body },
        };

        state.requests.lock().expect("lock requests").push(captured);

        match TelegramApiMethod::from_path(uri.path()) {
            TelegramApiMethod::SendMessage => Json(TelegramApiResponse {
                ok: true,
                result: TelegramApiResult::Message(TelegramMessageResult {
                    message_id: 1,
                    date: 0,
                    chat: TelegramChat {
                        id: 42,
                        chat_type: "private".to_string(),
                    },
                    text: "ok".to_string(),
                }),
            }),
            TelegramApiMethod::GetFile => Json(TelegramApiResponse {
                ok: true,
                result: TelegramApiResult::File(TelegramFileResult {
                    file_id: "file-id".to_string(),
                    file_unique_id: "file-unique".to_string(),
                    file_size: 4,
                    file_path: "test.bin".to_string(),
                }),
            }),
            TelegramApiMethod::SendChatAction
            | TelegramApiMethod::AnswerCallbackQuery
            | TelegramApiMethod::Other(_) => Json(TelegramApiResponse {
                ok: true,
                result: TelegramApiResult::Bool(true),
            }),
        }
    }

    async fn telegram_file_handler(
        State(state): State<MockTelegramApi>,
        uri: Uri,
    ) -> axum::body::Bytes {
        state
            .requests
            .lock()
            .expect("lock requests")
            .push(CapturedTelegramRequest::FileDownload {
                path: uri.path().to_string(),
            });
        axum::body::Bytes::from_static(b"abcd")
    }

    fn mock_sink_format_text(text: &str, _meta: &ChannelMessageMeta) -> String {
        text.to_string()
    }

    #[derive(Default)]
    struct MockSink {
        dispatch_calls: std::sync::atomic::AtomicUsize,
        ingest_calls: std::sync::atomic::AtomicUsize,
        command_calls: std::sync::atomic::AtomicUsize,
        fail_command: std::sync::atomic::AtomicBool,
        resolve_location: std::sync::atomic::AtomicBool,
        last_dispatch_text: Mutex<Option<String>>,
        last_ingest_text: Mutex<Option<String>>,
        last_command: Mutex<Option<String>>,
        last_command_ctx: Mutex<Option<ChannelInboundContext>>,
        last_location_ctx: Mutex<Option<ChannelInboundContext>>,
        command_response: Mutex<Option<String>>,
    }

    #[async_trait]
    impl ChannelEventSink for MockSink {
        async fn emit(&self, _event: ChannelEvent) {}

        async fn dispatch_to_chat(
            &self,
            text: &str,
            _ctx: ChannelInboundContext,
            meta: ChannelMessageMeta,
        ) {
            let mut last = self.last_dispatch_text.lock().unwrap();
            *last = Some(mock_sink_format_text(text, &meta));
            self.dispatch_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        async fn ingest_only(
            &self,
            text: &str,
            _ctx: ChannelInboundContext,
            meta: ChannelMessageMeta,
        ) {
            let mut last = self.last_ingest_text.lock().unwrap();
            *last = Some(mock_sink_format_text(text, &meta));
            self.ingest_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        async fn dispatch_command(
            &self,
            command: &str,
            ctx: ChannelInboundContext,
        ) -> anyhow::Result<String> {
            {
                let mut last = self.last_command.lock().unwrap();
                *last = Some(command.to_string());
            }
            {
                let mut last = self.last_command_ctx.lock().unwrap();
                *last = Some(ctx);
            }
            self.command_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.fail_command.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("boom")
            }
            Ok(self
                .command_response
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default())
        }

        async fn request_disable_account(
            &self,
            _channel_type: &str,
            _account_handle: &str,
            _reason: &str,
        ) {
        }

        async fn transcribe_voice(&self, _audio_data: &[u8], _format: &str) -> Result<String> {
            Err(anyhow::anyhow!(
                "transcribe should not be called when STT unavailable"
            ))
        }

        async fn voice_stt_available(&self) -> bool {
            false
        }

        async fn update_location(
            &self,
            ctx: ChannelInboundContext,
            _latitude: f64,
            _longitude: f64,
        ) -> bool {
            let mut last = self.last_location_ctx.lock().unwrap();
            *last = Some(ctx);
            self.resolve_location
                .load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl crate::adapter::TelegramCoreBridge for MockSink {
        async fn handle_inbound(&self, request: TgInboundRequest) {
            let bucket_key = request.route.bucket_key.clone();
            let reply_target_ref = crate::adapter::reply_target_ref_for_target(
                request.private_target.account_handle.as_str(),
                request.private_target.chat_id.as_str(),
                request.private_target.thread_id.as_deref(),
                request.private_target.message_id.as_deref(),
            )
            .unwrap_or_else(|| "{}".to_string());
            let channel_binding = crate::adapter::telegram_binding_json_for_bucket(
                request.private_target.account_handle.as_str(),
                request.private_target.chat_id.as_str(),
                request.private_target.thread_id.as_deref(),
                Some(bucket_key.as_str()),
            );
            let ctx = ChannelInboundContext {
                chan_type: ChannelType::Telegram,
                bucket_key,
                reply_target_ref,
                channel_binding,
            };
            let meta = ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: request.sender_name,
                username: request.username,
                message_kind: request.message_kind,
                model: request.model,
            };
            let text = request.inbound.body.text;
            match request.inbound.mode {
                TgInboundMode::Dispatch => {
                    <Self as ChannelEventSink>::dispatch_to_chat(self, &text, ctx, meta).await;
                },
                TgInboundMode::RecordOnly => {
                    <Self as ChannelEventSink>::ingest_only(self, &text, ctx, meta).await;
                },
            }
        }

        async fn dispatch_command(
            &self,
            command: &str,
            target: TgFollowUpTarget,
        ) -> anyhow::Result<String> {
            let bucket_key = target.route.bucket_key.clone();
            let reply_target_ref = crate::adapter::reply_target_ref_for_target(
                target.private_target.account_handle.as_str(),
                target.private_target.chat_id.as_str(),
                target.private_target.thread_id.as_deref(),
                target.private_target.message_id.as_deref(),
            )
            .ok_or_else(|| anyhow::anyhow!("missing reply_target_ref"))?;
            let channel_binding = crate::adapter::telegram_binding_json_for_bucket(
                target.private_target.account_handle.as_str(),
                target.private_target.chat_id.as_str(),
                target.private_target.thread_id.as_deref(),
                Some(bucket_key.as_str()),
            );
            <Self as ChannelEventSink>::dispatch_command(
                self,
                command,
                ChannelInboundContext {
                    chan_type: ChannelType::Telegram,
                    bucket_key,
                    reply_target_ref,
                    channel_binding,
                },
            )
            .await
        }

        async fn request_voice_transcription(
            &self,
            audio_data: &[u8],
            format: &str,
        ) -> Result<String> {
            <Self as ChannelEventSink>::transcribe_voice(self, audio_data, format).await
        }

        async fn voice_transcription_available(&self) -> bool {
            <Self as ChannelEventSink>::voice_stt_available(self).await
        }

        async fn update_location(
            &self,
            target: TgFollowUpTarget,
            latitude: f64,
            longitude: f64,
        ) -> bool {
            let bucket_key = target.route.bucket_key.clone();
            let Some(reply_target_ref) = crate::adapter::reply_target_ref_for_target(
                target.private_target.account_handle.as_str(),
                target.private_target.chat_id.as_str(),
                target.private_target.thread_id.as_deref(),
                target.private_target.message_id.as_deref(),
            ) else {
                return false;
            };
            let channel_binding = crate::adapter::telegram_binding_json_for_bucket(
                target.private_target.account_handle.as_str(),
                target.private_target.chat_id.as_str(),
                target.private_target.thread_id.as_deref(),
                Some(bucket_key.as_str()),
            );
            <Self as ChannelEventSink>::update_location(
                self,
                ChannelInboundContext {
                    chan_type: ChannelType::Telegram,
                    bucket_key,
                    reply_target_ref,
                    channel_binding,
                },
                latitude,
                longitude,
            )
            .await
        }
    }

    #[derive(Default)]
    struct LegacyTrapSink {
        old_path_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl ChannelEventSink for LegacyTrapSink {
        async fn emit(&self, _event: ChannelEvent) {}

        async fn dispatch_to_chat(
            &self,
            _text: &str,
            _ctx: ChannelInboundContext,
            _meta: ChannelMessageMeta,
        ) {
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
        }

        async fn ingest_only(
            &self,
            _text: &str,
            _ctx: ChannelInboundContext,
            _meta: ChannelMessageMeta,
        ) {
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
        }

        async fn dispatch_command(
            &self,
            _command: &str,
            _ctx: ChannelInboundContext,
        ) -> anyhow::Result<String> {
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
            Ok(String::new())
        }

        async fn request_disable_account(
            &self,
            _channel_type: &str,
            _account_handle: &str,
            _reason: &str,
        ) {
        }

        async fn transcribe_voice(&self, _audio_data: &[u8], _format: &str) -> Result<String> {
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
            Ok(String::new())
        }

        async fn voice_stt_available(&self) -> bool {
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
            false
        }

        async fn update_location(
            &self,
            _ctx: ChannelInboundContext,
            _latitude: f64,
            _longitude: f64,
        ) -> bool {
            let _ = _ctx;
            self.old_path_calls.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    #[derive(Default)]
    struct BridgeRecorder {
        inbound_calls: std::sync::atomic::AtomicUsize,
        last_inbound: Mutex<Option<TgInboundRequest>>,
        command_calls: std::sync::atomic::AtomicUsize,
        last_command: Mutex<Option<(String, TgFollowUpTarget)>>,
        location_calls: std::sync::atomic::AtomicUsize,
        last_location: Mutex<Option<(TgFollowUpTarget, f64, f64)>>,
    }

    #[async_trait]
    impl crate::adapter::TelegramCoreBridge for BridgeRecorder {
        async fn handle_inbound(&self, request: TgInboundRequest) {
            self.inbound_calls.fetch_add(1, Ordering::Relaxed);
            *self.last_inbound.lock().unwrap() = Some(request);
        }

        async fn dispatch_command(
            &self,
            command: &str,
            target: TgFollowUpTarget,
        ) -> anyhow::Result<String> {
            self.command_calls.fetch_add(1, Ordering::Relaxed);
            *self.last_command.lock().unwrap() = Some((command.to_string(), target));
            Ok("done".to_string())
        }

        async fn request_voice_transcription(
            &self,
            _audio_data: &[u8],
            _format: &str,
        ) -> Result<String> {
            Ok(String::new())
        }

        async fn voice_transcription_available(&self) -> bool {
            true
        }

        async fn update_location(
            &self,
            target: TgFollowUpTarget,
            latitude: f64,
            longitude: f64,
        ) -> bool {
            self.location_calls.fetch_add(1, Ordering::Relaxed);
            *self.last_location.lock().unwrap() = Some((target, latitude, longitude));
            true
        }
    }

    #[tokio::test]
    async fn inbound_text_uses_tg_core_bridge_instead_of_legacy_event_sink() {
        let bot = teloxide::Bot::new("test-token");
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(crate::outbound::TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let legacy_sink = Arc::new(LegacyTrapSink::default());
        let bridge = Arc::new(BridgeRecorder::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound,
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(legacy_sink.clone() as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        bridge.clone() as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 7,
            "date": 1,
            "chat": { "id": -100123, "type": "supergroup", "title": "Test Group" },
            "message_thread_id": 9,
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot hello"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(legacy_sink.old_path_calls.load(Ordering::Relaxed), 0);
        assert_eq!(bridge.inbound_calls.load(Ordering::Relaxed), 1);
        let request = bridge
            .last_inbound
            .lock()
            .unwrap()
            .clone()
            .expect("recorded request");
        assert_eq!(
            request.route.bucket_key,
            "group-peer-tgchat.n100123"
        );
        assert_eq!(request.private_target.chat_id, "-100123");
        assert_eq!(request.private_target.thread_id.as_deref(), Some("9"));
        assert_eq!(request.inbound.body.text, "Alice -> you: @test_bot hello");
    }

    #[test]
    fn group_record_only_does_not_create_external_root_state() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 77,
            "date": 0,
            "chat": { "id": -1001, "type": "supergroup", "title": "ops" },
            "from": {
                "id": 9001,
                "is_bot": false,
                "first_name": "Alice"
            },
            "text": "just context"
        }))
        .unwrap();

        let action = apply_group_dispatch_fuse(
            &accounts,
            "telegram:200",
            &msg,
            None,
            crate::adapter::TgGroupTargetAction {
                mode: TgInboundMode::RecordOnly,
                body: "just context".into(),
                addressed: false,
                reason_code: "tg_record_context",
            },
        );

        assert_eq!(action.mode, TgInboundMode::RecordOnly);

        let binding = crate::state::shared_group_runtime(&accounts);
        let mut runtime = binding.lock().unwrap();
        assert!(runtime.message_context("-1001", "77").is_none());
        assert!(runtime.root_budget_snapshot("-1001", "77").is_none());
    }

    #[test]
    fn same_external_group_message_reuses_same_root_across_targets() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 88,
            "date": 0,
            "chat": { "id": -1001, "type": "supergroup", "title": "ops" },
            "from": {
                "id": 9002,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "@alpha_bot @beta_bot do it"
        }))
        .unwrap();

        let action = crate::adapter::TgGroupTargetAction {
            mode: TgInboundMode::Dispatch,
            body: "@alpha_bot @beta_bot do it".into(),
            addressed: true,
            reason_code: "tg_dispatch_line_start_mention",
        };

        let first =
            apply_group_dispatch_fuse(&accounts, "telegram:200", &msg, None, action.clone());
        let second = apply_group_dispatch_fuse(&accounts, "telegram:300", &msg, None, action);

        assert_eq!(first.mode, TgInboundMode::Dispatch);
        assert_eq!(second.mode, TgInboundMode::Dispatch);

        let binding = crate::state::shared_group_runtime(&accounts);
        let mut runtime = binding.lock().unwrap();
        let context = runtime.message_context("-1001", "88").unwrap();
        let budget = runtime.root_budget_snapshot("-1001", "88").unwrap();
        assert_eq!(context.root_message_id, "88");
        assert_eq!(budget.used, 0);
    }

    #[test]
    fn managed_dispatch_without_root_context_emits_warn_log_and_record_only() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 99,
            "date": 0,
            "chat": { "id": -1001, "type": "supergroup", "title": "ops" },
            "from": {
                "id": 100,
                "is_bot": true,
                "first_name": "Source",
                "username": "source_bot"
            },
            "text": "@alpha_bot continue"
        }))
        .unwrap();

        let (logs, action) = capture_json_logs(|| {
            apply_group_dispatch_fuse(
                &accounts,
                "telegram:200",
                &msg,
                Some("telegram:100"),
                crate::adapter::TgGroupTargetAction {
                    mode: TgInboundMode::Dispatch,
                    body: "@alpha_bot continue".into(),
                    addressed: true,
                    reason_code: "tg_dispatch_line_start_mention",
                },
            )
        });

        assert_eq!(action.mode, TgInboundMode::RecordOnly);
        let fuse_log = logs
            .iter()
            .find(|entry| entry["fields"]["event"] == "telegram.group.dispatch_fuse")
            .unwrap();
        assert_eq!(fuse_log["level"], "WARN");
        assert_eq!(
            fuse_log["fields"]["reason_code"],
            "root_dispatch_context_missing"
        );
    }

    #[tokio::test]
    async fn callback_query_uses_tg_core_bridge_instead_of_legacy_event_sink() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(crate::outbound::TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let legacy_sink = Arc::new(LegacyTrapSink::default());
        let bridge = Arc::new(BridgeRecorder::default());
        let account_handle = "telegram:test-account";
        let expected_bucket_key = crate::adapter::resolve_group_bucket_key(
            &crate::config::GroupScope::PerSender,
            None,
            "tgchat.n100999",
            Some("tguser.2002"),
            None,
        );

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        group_scope: crate::config::GroupScope::PerSender,
                        ..Default::default()
                    },
                    outbound,
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(legacy_sink.clone() as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        bridge.clone() as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        remember_callback_bucket_binding(
            &ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: account_handle.to_string(),
                chan_user_name: Some("@test_bot".to_string()),
                chat_id: "-100999".to_string(),
                message_id: Some("10".to_string()),
                thread_id: None,
                bucket_key: Some(expected_bucket_key.clone()),
            },
            10,
        );

        let query: CallbackQuery = serde_json::from_value(serde_json::json!({
            "id": "cb-bridge",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": -100999, "type": "supergroup", "title": "Team" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("handle callback");

        assert_eq!(legacy_sink.old_path_calls.load(Ordering::Relaxed), 0);
        assert_eq!(bridge.command_calls.load(Ordering::Relaxed), 1);
        let (command, target) = bridge
            .last_command
            .lock()
            .unwrap()
            .clone()
            .expect("recorded callback target");
        assert_eq!(command, "sessions 1");
        assert_eq!(target.private_target.chat_id, "-100999");
        assert_eq!(target.route.bucket_key, expected_bucket_key);

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn edited_live_location_uses_tg_core_bridge_instead_of_legacy_event_sink() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(crate::outbound::TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let legacy_sink = Arc::new(LegacyTrapSink::default());
        let bridge = Arc::new(BridgeRecorder::default());
        let account_handle = "telegram:test-account";
        let expected_bucket_key = crate::adapter::resolve_group_bucket_key(
            &crate::config::GroupScope::PerBranchSender,
            None,
            "tgchat.n100888",
            Some("tguser.2002"),
            Some("topic.77"),
        );

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: teloxide::Bot::new("test-token"),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        group_scope: crate::config::GroupScope::PerBranchSender,
                        ..Default::default()
                    },
                    outbound,
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(legacy_sink.clone() as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        bridge.clone() as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 99,
            "date": 1,
            "edit_date": 2,
            "chat": { "id": -100888, "type": "supergroup", "title": "Team" },
            "message_thread_id": 77,
            "from": {
                "id": 2002,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "location": {
                "latitude": 30.1,
                "longitude": 120.2,
                "live_period": 300
            }
        }))
        .expect("deserialize live location edit");

        handle_edited_location(msg, account_handle, &accounts)
            .await
            .expect("handle edited location");

        assert_eq!(legacy_sink.old_path_calls.load(Ordering::Relaxed), 0);
        assert_eq!(bridge.location_calls.load(Ordering::Relaxed), 1);
        let (target, latitude, longitude) = bridge
            .last_location
            .lock()
            .unwrap()
            .clone()
            .expect("recorded live location target");
        assert_eq!(target.private_target.chat_id, "-100888");
        assert_eq!(target.private_target.thread_id.as_deref(), Some("77"));
        assert_eq!(target.route.bucket_key, expected_bucket_key);
        assert_eq!(latitude, 30.1);
        assert_eq!(longitude, 120.2);
    }

    /// Security: the OTP challenge message sent to the Telegram user must
    /// NEVER contain the verification code.  The code should only be visible
    /// to the admin in the web UI.  If this test fails, unauthenticated users
    /// can self-approve without admin involvement.
    #[test]
    fn security_otp_challenge_message_does_not_contain_code() {
        let msg = OTP_CHALLENGE_MSG;

        // Must not contain any 6-digit numeric sequences (OTP codes are 6 digits).
        let has_six_digits = msg
            .as_bytes()
            .windows(6)
            .any(|w| w.iter().all(|b| b.is_ascii_digit()));
        assert!(
            !has_six_digits,
            "OTP challenge message must not contain a 6-digit code: {msg}"
        );

        // Must not contain format placeholders that could interpolate a code.
        assert!(
            !msg.contains("{code}") && !msg.contains("{0}"),
            "OTP challenge message must not contain format placeholders: {msg}"
        );

        // Must contain instructions pointing to the web UI.
        assert!(
            msg.contains("Channels") && msg.contains("Senders"),
            "OTP challenge message must tell the user where to find the code"
        );
    }

    #[test]
    fn voice_messages_are_marked_with_voice_message_kind() {
        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "voice": {
                "file_id": "voice-file-id",
                "file_unique_id": "voice-unique-id",
                "duration": 1,
                "mime_type": "audio/ogg",
                "file_size": 123
            }
        }))
        .expect("deserialize voice message");

        assert!(matches!(
            message_kind(&msg),
            Some(ChannelMessageKind::Voice)
        ));
    }

    #[tokio::test]
    async fn voice_not_configured_replies_with_setup_hint_and_skips_dispatch() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                // Some sandboxed CI environments disallow binding sockets.
                // Skipping keeps the test suite runnable while still exercising
                // the logic in environments where local binds are permitted.
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "voice": {
                "file_id": "voice-file-id",
                "file_unique_id": "voice-unique-id",
                "duration": 1,
                "mime_type": "audio/ogg",
                "file_size": 123
            }
        }))
        .expect("deserialize voice message");
        assert!(
            extract_voice_file(&msg).is_some(),
            "message should contain voice media"
        );

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        {
            let requests = recorded_requests.lock().expect("requests lock");
            assert!(
                requests.iter().any(|request| {
                    if let CapturedTelegramRequest::SendMessage(body) = request {
                        body.chat_id == 42
                            && body.parse_mode.as_deref() == Some("HTML")
                            && body
                                .text
                                .contains("I can't understand voice, you did not configure it")
                    } else {
                        false
                    }
                }),
                "expected voice setup hint to be sent, requests={requests:?}"
            );
            assert!(
                requests.iter().any(|request| {
                    if let CapturedTelegramRequest::SendChatAction(action) = request {
                        action.chat_id == 42 && action.action == "typing"
                    } else {
                        false
                    }
                }),
                "expected typing action before reply, requests={requests:?}"
            );
            assert!(
                requests.iter().all(|request| {
                    if let CapturedTelegramRequest::Other { method, raw_body } = request {
                        !matches!(
                            method,
                            TelegramApiMethod::SendMessage | TelegramApiMethod::SendChatAction
                        ) || raw_body.is_empty()
                    } else {
                        true
                    }
                }),
                "unexpected untyped request capture for known method, requests={requests:?}"
            );
        }
        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "voice message should not be dispatched to chat when STT is unavailable"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn self_mention_is_stripped_before_dispatch_to_chat() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot hello",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let last = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn self_mention_only_replies_presence_and_skips_dispatch() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );

        {
            let requests = recorded_requests.lock().expect("requests lock");
            assert!(
                requests.iter().any(|request| {
                    if let CapturedTelegramRequest::SendMessage(body) = request {
                        body.chat_id == 42 && body.text.contains("我在")
                    } else {
                        false
                    }
                }),
                "expected presence reply to be sent"
            );
        }

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn group_not_mentioned_always_ingests_listen_only() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "hello everyone"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "listen-only ingest must not dispatch to chat/LLM"
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let last = sink.last_ingest_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice: hello everyone"));
    }

    #[tokio::test]
    async fn group_not_mentioned_tg_gst_v1_ingests_with_speaker_header() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "hello everyone"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "listen-only ingest must not dispatch to chat/LLM"
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let last = sink.last_ingest_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice: hello everyone"));
    }

    #[tokio::test]
    async fn group_not_mentioned_tg_gst_v1_prefers_people_display_name() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        crate::state::replace_telegram_identity_links(
            &accounts,
            vec![TelegramIdentityLink {
                agent_id: "alice".into(),
                display_name: Some("Alice Zhang".into()),
                telegram_user_id: Some(1001),
                telegram_user_name: Some("alice".into()),
                telegram_display_name: Some("Alice TG".into()),
            }],
        );
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "hello everyone"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let last = sink.last_ingest_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice Zhang: hello everyone"));
    }

    #[tokio::test]
    async fn group_mentioned_dispatches_and_strips_self_mention() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot hi",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        let last = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice -> you: @test_bot hi"));
    }

    #[tokio::test]
    async fn group_mentioned_tg_gst_v1_dispatches_without_stripping() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot hi",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        let last = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice -> you: @test_bot hi"));
    }

    #[tokio::test]
    async fn group_multi_mention_tg_gst_v1_preserves_mentions_and_newlines() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@a @test_bot @c do X",
            "entities": [
                { "type": "mention", "offset": 0, "length": 2 },
                { "type": "mention", "offset": 3, "length": 9 },
                { "type": "mention", "offset": 13, "length": 2 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        let last = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice -> you: @a @test_bot @c do X"));

        let msg2: Message = serde_json::from_value(json!({
            "message_id": 2,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot\n\n你处理下X",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg2, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        let last2 = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(
            last2.as_deref(),
            Some("Alice -> you: @test_bot\n\n你处理下X")
        );
    }

    #[tokio::test]
    async fn group_self_mention_only_tg_gst_v1_ingests_and_presence_reply() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "@test_bot",
            "entities": [
                { "type": "mention", "offset": 0, "length": 9 }
            ]
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "presence reply path must not dispatch to chat/LLM"
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "presence reply path must ingest transcript context"
        );
        let last = sink.last_ingest_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice -> you: @test_bot"));

        {
            let requests = recorded_requests.lock().expect("requests lock");
            assert!(
                requests.iter().any(|request| {
                    if let CapturedTelegramRequest::SendMessage(body) = request {
                        body.chat_id == -1001 && body.text.contains("我在")
                    } else {
                        false
                    }
                }),
                "expected presence reply to be sent"
            );
        }

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn group_unaddressed_tg_gst_v1_records_without_you_flag() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound::new(Arc::clone(&accounts)));
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let bot = teloxide::Bot::new("test-token");

        {
            let mut map = accounts.write().expect("accounts write lock");
            let cfg = TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                ..Default::default()
            };
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: cfg,
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "hello everyone"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            sink.ingest_calls.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let last = sink.last_ingest_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("Alice: hello everyone"));
    }

    #[tokio::test]
    async fn addressed_slash_command_in_dm_is_intercepted() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "/help@test_bot"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );

        {
            let requests = recorded_requests.lock().expect("requests lock");
            assert!(
                requests.iter().any(|request| {
                    if let CapturedTelegramRequest::SendMessage(body) = request {
                        body.chat_id == 42 && body.text.contains("Available commands:")
                    } else {
                        false
                    }
                }),
                "expected /help response to be sent"
            );
        }

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn addressed_slash_command_failure_sends_sanitized_message() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        sink.fail_command
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "/model@test_bot"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        let requests = recorded_requests.lock().expect("requests lock");
        let sent = requests
            .iter()
            .find_map(|request| match request {
                CapturedTelegramRequest::SendMessage(body) if body.chat_id == 42 => {
                    Some(body.text.clone())
                },
                _ => None,
            })
            .expect("expected user-facing error message");
        assert!(
            sent.contains("Something went wrong"),
            "expected sanitized failure text, got {sent}"
        );
        assert!(!sent.contains("boom"), "internal error text must not leak");
        assert!(
            requests.iter().any(|request| {
                matches!(
                    request,
                    CapturedTelegramRequest::SendChatAction(action)
                        if action.chat_id == 42 && action.action == "typing"
                )
            }),
            "expected typing action before slash command feedback, requests={requests:?}"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| matches!(
                    request,
                    CapturedTelegramRequest::SendChatAction(action)
                        if action.chat_id == 42 && action.action == "typing"
                ))
                .count(),
            1,
            "slash command feedback must have a single typing owner, requests={requests:?}"
        );

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn addressed_slash_command_to_other_bot_in_dm_is_ignored() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "/help@other_bot"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert!(
            recorded_requests.lock().expect("requests lock").is_empty(),
            "expected no outbound send for other bot command"
        );

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[test]
    fn extract_location_from_message() {
        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "location": {
                "latitude": 48.8566,
                "longitude": 2.3522
            }
        }))
        .expect("deserialize location message");

        let loc = extract_location(&msg);
        assert!(loc.is_some(), "should extract location from message");
        let info = loc.unwrap();
        assert!((info.latitude - 48.8566).abs() < 1e-4);
        assert!((info.longitude - 2.3522).abs() < 1e-4);
        assert!(!info.is_live, "static location should not be live");
    }

    #[test]
    fn extract_location_returns_none_for_text() {
        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice"
            },
            "text": "hello"
        }))
        .expect("deserialize text message");

        assert!(extract_location(&msg).is_none());
    }

    #[tokio::test]
    async fn send_sessions_keyboard_records_bucket_binding_for_sent_message() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);
        let reply_target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:test-account".to_string(),
            chan_user_name: Some("@test_bot".to_string()),
            chat_id: "-100999".to_string(),
            message_id: Some("10".to_string()),
            thread_id: None,
            bucket_key: Some(
                "group-peer-tgchat.n100999-sender-tguser.2002".to_string(),
            ),
        };

        send_sessions_keyboard(&bot, &reply_target, "1. Session A *\n2. Session B")
            .await
            .expect("send sessions keyboard");

        assert_eq!(
            lookup_callback_bucket_binding("telegram:test-account", "-100999", 1).as_deref(),
            reply_target.bucket_key.as_deref(),
            "keyboard send must remember the originating bucket for later callback routing"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn edited_live_location_preserves_bucket_key_for_group_scope() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        let expected_bucket_key = crate::adapter::resolve_group_bucket_key(
            &crate::config::GroupScope::PerSender,
            None,
            "tgchat.n100999",
            Some("tguser.1001"),
            None,
        );

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: teloxide::Bot::new("test-token"),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        group_scope: crate::config::GroupScope::PerSender,
                        ..Default::default()
                    },
                    outbound,
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 22,
            "date": 1,
            "chat": { "id": -100999, "type": "supergroup", "title": "Team" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "location": {
                "latitude": 48.8566,
                "longitude": 2.3522,
                "live_period": 300
            }
        }))
        .expect("deserialize edited location message");

        handle_edited_location(msg, account_handle, &accounts)
            .await
            .expect("handle edited location");

        let ctx = sink
            .last_location_ctx
            .lock()
            .unwrap()
            .clone()
            .expect("expected update_location context");
        assert_eq!(ctx.bucket_key, expected_bucket_key);
        let decoded = crate::adapter::inbound_target_from_reply_target_ref(&ctx.reply_target_ref)
            .expect("decode reply_target_ref");
        assert_eq!(decoded.chat_id, "-100999");
    }

    #[test]
    fn location_messages_are_marked_with_location_message_kind() {
        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice"
            },
            "location": {
                "latitude": 48.8566,
                "longitude": 2.3522
            }
        }))
        .expect("deserialize location message");

        assert!(matches!(
            message_kind(&msg),
            Some(ChannelMessageKind::Location)
        ));
    }

    #[test]
    fn extract_location_detects_live_period() {
        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice"
            },
            "location": {
                "latitude": 48.8566,
                "longitude": 2.3522,
                "live_period": 3600
            }
        }))
        .expect("deserialize live location message");

        let info = extract_location(&msg).expect("should extract live location");
        assert!(info.is_live, "location with live_period should be live");
        assert!((info.latitude - 48.8566).abs() < 1e-4);
    }

    #[test]
    fn parse_context_v1_payload_extracts_payload() {
        let payload = json!({ "session": { "key": "session:1" } });
        let wrapped = json!({ "format": "context.v1", "payload": payload });
        let text = wrapped.to_string();
        let out = parse_context_v1_payload(&text).expect("should parse context.v1");
        assert_eq!(
            out.get("session")
                .and_then(|v| v.get("key"))
                .and_then(|v| v.as_str()),
            Some("session:1")
        );
    }

    #[test]
    fn render_context_card_markdown_fallback_uses_last_next_when_tokens_missing() {
        let markdown = "\
**Session:** `telegram:bot:123`\n\
**Messages:** `3`\n\
**Provider:** `openai-responses`\n\
**Model:** `openai-responses::gpt-5.2`\n\
**Sandbox:** `on (docker)`\n\
**Plugins:** `none`\n\
**Last:** `in=10 out=20 cached=0`\n\
**Next (est):** `prompt=6500`\n";

        let html = render_context_card_markdown_fallback(markdown);
        assert!(html.contains("📋 Session Context"));
        assert!(html.contains("telegram:bot:123"));
        assert!(
            html.contains("Tokens    Last in=10 out=20 cached=0 · Next prompt=6500"),
            "should synthesize Tokens from Last/Next when Tokens is missing"
        );
    }

    #[test]
    fn truncate_middle_is_unicode_safe() {
        let s = "你好，世界，这是一个很长的字符串";
        let t = truncate_middle(s, 6);
        assert!(t.contains('…'));
        assert!(t.chars().count() <= 6);
    }

    #[test]
    fn render_context_card_v1_includes_key_fields_and_truncates_lists() {
        let mounts: Vec<serde_json::Value> = (0..12)
            .map(|i| {
                json!({
                    "hostDir": format!("/very/long/host/path/that/should/be/truncated/{i}/subdir"),
                    "guestDir": format!("/mnt/{i}/subdir"),
                    "mode": "ro",
                })
            })
            .collect();
        let skills: Vec<serde_json::Value> = (0..10)
            .map(|i| json!({"name": format!("skill_{i}")}))
            .collect();

        let payload = json!({
            "session": {
                "key": "agent:zhuzhu:group-peer-tgchat.n123",
                "messageCount": 42,
                "provider": "openai-responses",
                "model": "openai-responses::gpt-5.2"
            },
            "llm": {
                "provider": "openai-responses",
                "model": "openai-responses::gpt-5.2",
                "overrides": {
                    "prompt_cache_key": "agent:zhuzhu:group-peer-tgchat.n123-branch-topic.42-sender-person.alice-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "generation": {
                        "max_output_tokens": { "configured": 2048, "effective": 2048, "limit": 8192, "clamped": false },
                        "reasoning_effort": "medium",
                        "text_verbosity": "low",
                        "temperature": 0.0
                    }
                }
            },
            "compaction": {
                "isCompacted": true,
                "summaryLen": 123,
                "keptMessageCount": 9,
                "keepLastUserRounds": 4
            },
            "sandbox": {
                "enabled": true,
                "backend": "docker",
                "image": "ubuntu:22.04",
                "externalMountsStatus": "configured",
                "mountAllowlist": ["/a", "/b", "/c"],
                "mounts": mounts
            },
            "skills": skills,
            "tokenDebug": {
                "lastRequest": { "inputTokens": 100, "outputTokens": 50, "cachedTokens": 25 },
                "nextRequest": {
                    "contextWindow": 128000,
                    "plannedMaxOutputToks": 4096,
                    "maxInputToks": 96000,
                    "autoCompactToksThred": 81600,
                    "promptInputToksEst": 6500,
                    "compactProgress": 0.079656,
                    "details": { "method": "heuristic" }
                }
            }
        });

        let cfg = crate::config::TelegramAccountConfig::default();
        let html = render_context_card_v1(&payload, &cfg, ChatType::Group);
        assert!(html.contains("Session Context"));
        assert!(html.contains("agent:zhuzhu:group-peer-tgchat.n123"));
        if html.contains("Output too long for Telegram") {
            // Fallback path should still provide a minimal, valid summary.
            assert!(html.contains("openai-responses"));
            assert!(html.contains("gpt-5.2"));
        } else {
            assert!(html.contains("prompt_cache_key"));
            assert!(html.contains("max_output_tokens"));
            assert!(html.contains("Compaction"));
            assert!(html.contains("Sandbox"));
            assert!(
                html.contains("(+"),
                "should indicate truncated lists when large"
            );
            assert!(html.contains("Telegram has no draftText"));
        }
        assert!(
            html.chars().count() <= 3700,
            "should stay within a safe size for Telegram"
        );
    }

    #[test]
    fn normalize_username_strips_at_and_lowercases() {
        assert_eq!(normalize_username("@MyBot "), "mybot");
        assert_eq!(normalize_username("MyBot"), "mybot");
        assert_eq!(normalize_username("@mybot"), "mybot");
    }

    #[test]
    fn fallback_contains_at_username_is_boundary_safe() {
        let bot = "mybot";
        assert!(fallback_contains_at_username("@MyBot hi", bot));
        assert!(fallback_contains_at_username("hi @mybot", bot));
        assert!(!fallback_contains_at_username("@MyBot123 hi", bot));
    }

    #[test]
    fn entities_trigger_wakeup_recognizes_mention_case_insensitive() {
        let bot = "mybot";
        let text = "@MyBot hello";
        let entities = vec![MessageEntity::new(MessageEntityKind::Mention, 0, 6)];
        assert!(entities_trigger_wakeup(text, &entities, None, bot));
    }

    #[test]
    fn entities_trigger_wakeup_recognizes_only_addressed_bot_command() {
        let bot = "mybot";
        let addressed = "/context@MyBot";
        let entities = vec![MessageEntity::new(
            MessageEntityKind::BotCommand,
            0,
            addressed.len(),
        )];
        assert!(entities_trigger_wakeup(addressed, &entities, None, bot));

        let plain = "/context";
        let entities = vec![MessageEntity::new(
            MessageEntityKind::BotCommand,
            0,
            plain.len(),
        )];
        assert!(!entities_trigger_wakeup(plain, &entities, None, bot));
    }

    #[test]
    fn strip_self_mentions_strips_addressed_bot_command_suffix() {
        let bot = "mybot";
        let cmd = "/context@MyBot";
        let text = format!("{cmd} hi");
        let entities = vec![MessageEntity::new(
            MessageEntityKind::BotCommand,
            0,
            cmd.len(),
        )];
        let (out, stripped) = strip_self_mentions_from_text(&text, &entities, None, bot);
        assert!(stripped);
        assert_eq!(out, "/context hi");

        let cmd_other = "/context@OtherBot";
        let text_other = format!("{cmd_other} hi");
        let entities = vec![MessageEntity::new(
            MessageEntityKind::BotCommand,
            0,
            cmd_other.len(),
        )];
        let (out, stripped) = strip_self_mentions_from_text(&text_other, &entities, None, bot);
        assert!(!stripped);
        assert_eq!(out, text_other);
    }

    #[test]
    fn entities_trigger_wakeup_recognizes_text_mention_by_user_id() {
        let bot_username_norm = "mybot";
        let bot_id = UserId(4242);
        let user = teloxide::types::User {
            id: bot_id,
            is_bot: true,
            first_name: "MyBot".into(),
            last_name: None,
            username: Some("MyBot".into()),
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        };
        let text = "MyBot";
        let entities = vec![MessageEntity::new(
            MessageEntityKind::TextMention { user },
            0,
            5,
        )];
        assert!(entities_trigger_wakeup(
            text,
            &entities,
            Some(bot_id),
            bot_username_norm
        ));
    }

    #[test]
    fn check_bot_mentioned_is_true_when_replying_to_bot_message() {
        let bot_id = 4242;
        let msg: Message = serde_json::from_value(json!({
            "message_id": 2,
            "date": 1,
            "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "ok",
            "reply_to_message": {
                "message_id": 1,
                "date": 1,
                "chat": { "id": -1001, "type": "supergroup", "title": "Test Group" },
                "from": {
                    "id": bot_id,
                    "is_bot": true,
                    "first_name": "MyBot",
                    "username": "MyBot"
                },
                "text": "previous"
            }
        }))
        .expect("deserialize reply message");

        assert!(check_bot_mentioned(
            &msg,
            Some(UserId(bot_id)),
            Some("MyBot")
        ));
    }

    #[tokio::test]
    async fn callback_without_data_still_answers_callback_query() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb1",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": null
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, "telegram:test-account", &accounts)
            .await
            .expect("handle callback");

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|r| {
                matches!(r, CapturedTelegramRequest::AnswerCallbackQuery(_))
                    || matches!(
                        r,
                        CapturedTelegramRequest::Other {
                            method: TelegramApiMethod::AnswerCallbackQuery,
                            ..
                        }
                    )
            }),
            "expected answerCallbackQuery request, got {requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_with_message_missing_account_still_answers_callback_query() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        // Intentionally empty account map: triggers account_missing branch after early answer.
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb2",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": 42, "type": "private", "first_name": "Alice" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, "telegram:missing-account", &accounts)
            .await
            .expect("handle callback");

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|r| {
                matches!(r, CapturedTelegramRequest::AnswerCallbackQuery(_))
                    || matches!(
                        r,
                        CapturedTelegramRequest::Other {
                            method: TelegramApiMethod::AnswerCallbackQuery,
                            ..
                        }
                    )
            }),
            "expected answerCallbackQuery request, got {requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_with_data_answers_first_then_sends_followup_message() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        *sink.command_response.lock().unwrap() = Some("done".to_string());
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb-followup",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": 42, "type": "private", "first_name": "Alice" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("handle callback");

        let requests = recorded_requests.lock().expect("requests lock");
        let answer_idx = requests
            .iter()
            .position(|r| matches!(r, CapturedTelegramRequest::AnswerCallbackQuery(_)))
            .expect("expected answerCallbackQuery");
        let send_idx = requests
            .iter()
            .position(|r| {
                matches!(
                    r,
                    CapturedTelegramRequest::SendMessage(body) if body.chat_id == 42 && body.text == "done"
                )
            })
            .expect("expected follow-up sendMessage");
        let typing_idx = requests
            .iter()
            .position(|r| {
                matches!(
                    r,
                    CapturedTelegramRequest::SendChatAction(action)
                        if action.chat_id == 42 && action.action == "typing"
                )
            })
            .expect("expected typing action before follow-up send");
        assert!(
            answer_idx < send_idx,
            "callback must answer before sending follow-up, requests={requests:?}"
        );
        assert!(
            answer_idx < typing_idx && typing_idx < send_idx,
            "callback typing must remain after callback answer and before follow-up send, requests={requests:?}"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| matches!(
                    r,
                    CapturedTelegramRequest::SendChatAction(action)
                        if action.chat_id == 42 && action.action == "typing"
                ))
                .count(),
            1,
            "callback follow-up must have a single typing owner, requests={requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_query_preserves_bucket_key_for_group_scope() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        *sink.command_response.lock().unwrap() = Some("done".to_string());
        let account_handle = "telegram:test-account";
        let expected_bucket_key = crate::adapter::resolve_group_bucket_key(
            &crate::config::GroupScope::PerSender,
            None,
            "tgchat.n100999",
            Some("tguser.2002"),
            None,
        );

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        group_scope: crate::config::GroupScope::PerSender,
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb-bucket",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": -100999, "type": "supergroup", "title": "Team" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");
        let fallback_bucket_key = tg_bucket_key_for_callback_query(
            &TelegramAccountConfig {
                token: Secret::new("test-token".to_string()),
                group_scope: crate::config::GroupScope::PerSender,
                ..Default::default()
            },
            account_handle,
            &query,
        )
        .expect("fallback bucket");
        assert_ne!(
            fallback_bucket_key, expected_bucket_key,
            "stored callback binding must differ from clicker-derived fallback to prove preservation"
        );

        remember_callback_bucket_binding(
            &ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: account_handle.to_string(),
                chan_user_name: Some("@test_bot".to_string()),
                chat_id: "-100999".to_string(),
                message_id: Some("10".to_string()),
                thread_id: None,
                bucket_key: Some(expected_bucket_key.clone()),
            },
            10,
        );

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("handle callback");

        let ctx = sink
            .last_command_ctx
            .lock()
            .unwrap()
            .clone()
            .expect("expected callback dispatch context");
        assert_eq!(ctx.bucket_key, expected_bucket_key);
        let decoded = crate::adapter::inbound_target_from_reply_target_ref(&ctx.reply_target_ref)
            .expect("decode reply_target_ref");
        assert_eq!(decoded.chat_id, "-100999");

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_query_sender_hint_preserves_bucket_key_without_runtime_binding() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        *sink.command_response.lock().unwrap() = Some("done".to_string());
        let account_handle = "telegram:test-account";
        let expected_bucket_key = crate::adapter::resolve_group_bucket_key(
            &crate::config::GroupScope::PerSender,
            None,
            "tgchat.n100999",
            Some("tguser.2002"),
            None,
        );

        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        group_scope: crate::config::GroupScope::PerSender,
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb-sender-hint",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1|s=tguser.2002",
            "message": {
                "message_id": 11,
                "date": 1,
                "chat": { "id": -100999, "type": "supergroup", "title": "Team" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("handle callback");

        let ctx = sink
            .last_command_ctx
            .lock()
            .unwrap()
            .clone()
            .expect("expected callback dispatch context");
        assert_eq!(ctx.bucket_key, expected_bucket_key);
        let decoded = crate::adapter::inbound_target_from_reply_target_ref(&ctx.reply_target_ref)
            .expect("decode reply_target_ref");
        assert_eq!(decoded.chat_id, "-100999");

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_answer_transport_failure_is_retryable() {
        let bot =
            teloxide::Bot::new("test-token").set_api_url("http://127.0.0.1:1/".parse().unwrap());
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb3",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1"
        }))
        .expect("deserialize callback query");

        let err = handle_callback_query(query, &bot, "telegram:test-account", &accounts)
            .await
            .expect_err("expected retryable failure");
        assert!(
            err.downcast_ref::<RetryableUpdateError>().is_some(),
            "expected RetryableUpdateError"
        );
    }

    #[tokio::test]
    async fn callback_invalid_query_id_still_dispatches_followup_and_is_not_retryable() {
        async fn invalid_query_id_handler(
            State(state): State<MockTelegramApi>,
            uri: Uri,
            body: Bytes,
        ) -> axum::response::Response {
            let method = TelegramApiMethod::from_path(uri.path());
            let raw_body = String::from_utf8_lossy(&body).to_string();
            state
                .requests
                .lock()
                .expect("lock requests")
                .push(CapturedTelegramRequest::Other {
                    method: method.clone(),
                    raw_body,
                });
            match method {
                TelegramApiMethod::AnswerCallbackQuery => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "ok": false,
                        "error_code": 400,
                        "description": "Bad Request: query is too old and response timeout expired or query id is invalid"
                    })),
                )
                    .into_response(),
                _ => telegram_api_handler(State(state), uri, body).await.into_response(),
            }
        }

        use axum::response::IntoResponse as _;

        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(invalid_query_id_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        *sink.command_response.lock().unwrap() = Some("done after invalid ack".to_string());
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb-invalid",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": 42, "type": "private", "first_name": "Alice" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("invalid query id should be terminal");

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|r| {
                matches!(
                    r,
                    CapturedTelegramRequest::SendMessage(body)
                        if body.chat_id == 42 && body.text == "done after invalid ack"
                )
            }),
            "expected callback action to continue after invalid query id ack failure, requests={requests:?}"
        );
        assert_eq!(
            sink.command_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "callback action must still dispatch even when answerCallbackQuery is terminally rejected"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn callback_command_failure_sends_sanitized_message() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        sink.fail_command
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let query: CallbackQuery = serde_json::from_value(json!({
            "id": "cb-fail",
            "from": { "id": 1001, "is_bot": false, "first_name": "Alice", "username": "alice" },
            "chat_instance": "ci",
            "data": "sessions_switch:1",
            "message": {
                "message_id": 10,
                "date": 1,
                "chat": { "id": 42, "type": "private", "first_name": "Alice" },
                "text": "tap"
            }
        }))
        .expect("deserialize callback query");

        handle_callback_query(query, &bot, account_handle, &accounts)
            .await
            .expect("handle callback");

        let requests = recorded_requests.lock().expect("requests lock");
        let sent = requests
            .iter()
            .find_map(|request| match request {
                CapturedTelegramRequest::SendMessage(body) if body.chat_id == 42 => {
                    Some(body.text.clone())
                },
                _ => None,
            })
            .expect("expected user-facing error message");
        assert!(
            sent.contains("Something went wrong"),
            "expected sanitized failure text, got {sent}"
        );
        assert!(!sent.contains("boom"), "internal error text must not leak");

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn download_error_is_sanitized_and_does_not_include_token() {
        let bot = teloxide::Bot::new("super-secret-token")
            .set_api_url("http://127.0.0.1:1/".parse().unwrap());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            download_telegram_file(&bot, "file-id", 1024),
        )
        .await
        .expect("download should not hang");

        let err = result.expect_err("expected download failure");
        let rendered = err.to_string();
        assert!(
            !rendered.contains("super-secret-token"),
            "error must not contain token: {rendered}"
        );
        assert!(
            !rendered.contains("file/bot"),
            "error must not contain tokenized telegram file URL fragments: {rendered}"
        );
    }

    #[tokio::test]
    async fn photo_get_file_api_failure_sends_feedback_and_does_not_retry() {
        async fn get_file_api_failure_handler(
            State(state): State<MockTelegramApi>,
            uri: Uri,
            body: Bytes,
        ) -> axum::response::Response {
            let method = TelegramApiMethod::from_path(uri.path());
            let raw_body = String::from_utf8_lossy(&body).to_string();
            state
                .requests
                .lock()
                .expect("lock requests")
                .push(CapturedTelegramRequest::Other {
                    method: method.clone(),
                    raw_body,
                });
            match method {
                TelegramApiMethod::GetFile => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "ok": false,
                        "error_code": 400,
                        "description": "Bad Request: wrong file_id or the file is temporarily unavailable"
                    })),
                )
                    .into_response(),
                _ => telegram_api_handler(State(state), uri, body).await.into_response(),
            }
        }

        use axum::response::IntoResponse as _;

        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(get_file_api_failure_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "photo": [
                { "file_id": "photo-file-id", "file_unique_id": "photo-unique", "width": 1, "height": 1, "file_size": 4 }
            ],
            "caption": "hi"
        }))
        .expect("deserialize photo message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("permanent getFile failure should be terminal and send feedback");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "photo dispatch must not continue after terminal getFile failure"
        );

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|request| {
                if let CapturedTelegramRequest::SendMessage(body) = request {
                    body.chat_id == 42 && body.text.contains("couldn't download that photo")
                } else {
                    false
                }
            }),
            "expected terminal getFile failure to send user feedback, requests={requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[test]
    fn helper_chat_id_parse_failure_is_error_not_chatid_zero() {
        assert!(parse_chat_id("not-a-number").is_err());
    }

    #[tokio::test]
    async fn unsupported_document_in_dm_sends_feedback_and_skips_dispatch() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "document": {
                "file_id": "doc-file-id",
                "file_unique_id": "doc-unique-id",
                "file_size": 123,
                "file_name": "a.pdf",
                "mime_type": "application/pdf"
            },
            "caption": "please read"
        }))
        .expect("deserialize document message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        assert_eq!(
            sink.dispatch_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "unsupported document must not be dispatched to chat"
        );

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|request| {
                if let CapturedTelegramRequest::SendMessage(body) = request {
                    body.chat_id == 42 && body.text.contains("isn’t supported")
                } else {
                    false
                }
            }),
            "expected unsupported attachment feedback to be sent, requests={requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn event_sink_missing_in_dm_sends_user_facing_error_message() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "text": "hello"
        }))
        .expect("deserialize message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|request| {
                if let CapturedTelegramRequest::SendMessage(body) = request {
                    body.chat_id == 42 && body.text.contains("Something went wrong")
                } else {
                    false
                }
            }),
            "expected user-facing error feedback, requests={requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn photo_download_uses_bot_api_url_for_file_download() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                post(telegram_api_handler).get(telegram_file_handler),
            )
            .with_state(mock_api);

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve mock telegram api");
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let api_url = reqwest::Url::parse(&format!("http://{addr}/")).expect("parse api url");
        let bot = teloxide::Bot::new("test-token").set_api_url(api_url);

        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = Arc::new(TelegramOutbound {
            accounts: Arc::clone(&accounts),
        });
        let sink = Arc::new(MockSink::default());
        let account_handle = "telegram:test-account";
        {
            let mut map = accounts.write().expect("accounts write lock");
            map.insert(
                account_handle.to_string(),
                AccountState {
                    bot: bot.clone(),
                    bot_user_id: None,
                    bot_username: Some("test_bot".into()),
                    account_handle: account_handle.to_string(),
                    config: TelegramAccountConfig {
                        token: Secret::new("test-token".to_string()),
                        ..Default::default()
                    },
                    outbound: Arc::clone(&outbound),
                    cancel: CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
                    core_bridge: Some(
                        Arc::clone(&sink) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(OtpState::new(300)),
                },
            );
        }

        let msg: Message = serde_json::from_value(json!({
            "message_id": 1,
            "date": 1,
            "chat": { "id": 42, "type": "private", "first_name": "Alice" },
            "from": {
                "id": 1001,
                "is_bot": false,
                "first_name": "Alice",
                "username": "alice"
            },
            "photo": [
                { "file_id": "photo-file-id", "file_unique_id": "photo-unique", "width": 1, "height": 1, "file_size": 4 }
            ],
            "caption": "hi"
        }))
        .expect("deserialize photo message");

        handle_message_direct(msg, &bot, account_handle, &accounts)
            .await
            .expect("handle message");

        let requests = recorded_requests.lock().expect("requests lock");
        assert!(
            requests.iter().any(|r| {
                matches!(r, CapturedTelegramRequest::GetFile(_))
                    || matches!(
                        r,
                        CapturedTelegramRequest::Other {
                            method: TelegramApiMethod::GetFile,
                            ..
                        }
                    )
            }),
            "expected getFile request, got {requests:?}"
        );
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, CapturedTelegramRequest::FileDownload { .. })),
            "expected file download GET request, got {requests:?}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }
}

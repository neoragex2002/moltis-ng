use std::sync::Arc;

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
        ChannelAttachment, ChannelEvent, ChannelMessageKind, ChannelMessageMeta, ChannelOutbound,
        ChannelReplyTarget, ChannelType, message_log::MessageLogEntry,
    },
    moltis_common::types::ChatType,
};

#[cfg(feature = "metrics")]
use moltis_metrics::{counter, histogram, telegram as tg_metrics};

use crate::{
    access::{self, AccessDenied},
    otp::{OtpInitResult, OtpVerifyResult},
    state::AccountStateMap,
};

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

    let (config, bot_user_id, bot_username, outbound, message_log, event_sink) = {
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
        if reason == AccessDenied::NotMentioned {
            let (entities_count, caption_entities_count) = match &msg.kind {
                MessageKind::Common(common) => match &common.media_kind {
                    MediaKind::Text(t) => (t.entities.len(), 0),
                    MediaKind::Animation(a) => (0, a.caption_entities.len()),
                    MediaKind::Audio(a) => (0, a.caption_entities.len()),
                    MediaKind::Document(d) => (0, d.caption_entities.len()),
                    MediaKind::Photo(p) => (0, p.caption_entities.len()),
                    MediaKind::Video(v) => (0, v.caption_entities.len()),
                    MediaKind::Voice(v) => (0, v.caption_entities.len()),
                    _ => (0, 0),
                },
                _ => (0, 0),
            };
            let is_reply = msg.reply_to_message().is_some();
            let reply_from = msg
                .reply_to_message()
                .and_then(|m| m.from.as_ref())
                .map(|u| u.id.0);
            let text_has_at_sign = extract_text(&msg).is_some_and(|t| t.contains('@'));

            warn!(
                account_handle,
                %reason,
                peer_id,
                username = ?username,
                chat_id = msg.chat.id.0,
                message_id = msg.id.0,
                ?chat_type,
                bot_username = ?bot_username,
                bot_user_id = ?bot_user_id.map(|id| id.0),
                entities_count,
                caption_entities_count,
                is_reply,
                reply_from,
                text_has_at_sign,
                "handler: access denied"
            );
        } else {
            warn!(account_handle, %reason, peer_id, username = ?username, "handler: access denied");
        }
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

        // Group listen/sidecar mode (always enabled): ingest unaddressed messages into
        // session history without replying or triggering an LLM run.
        if chat_type == ChatType::Group
            && matches!(reason, AccessDenied::NotMentioned)
            && let Some(ref sink) = event_sink
        {
            let tg_gst_v1 = config.group_session_transcript_format
                == crate::config::GroupSessionTranscriptFormat::TgGstV1;
            let mut body = text.clone().unwrap_or_default();
            if tg_gst_v1 {
                body = tg_gst_v1_apply_media_placeholder(message_kind(&msg), &body);
                if !body.trim().is_empty() {
                    let speaker = tg_gst_v1_format_speaker(
                        username.as_deref(),
                        msg.from.as_ref().map(|u| u.id.0 as u64),
                        sender_name.as_deref(),
                        msg.from.as_ref().is_some_and(|u| u.is_bot),
                    );
                    body = tg_gst_v1_format_line(&speaker, false, &body);
                }
            } else if let Some(ref bot_username) = bot_username {
                let (rewritten, stripped) =
                    strip_self_mention_from_message(&msg, &body, bot_user_id, bot_username);
                if stripped {
                    body = rewritten;
                }
            }
            if !body.trim().is_empty() {
                let reply_target = ChannelReplyTarget {
                    chan_type: ChannelType::Telegram,
                    chan_account_key: account_handle.to_string(),
                    chan_user_name: bot_handle.clone(),
                    chat_id: msg.chat.id.0.to_string(),
                    message_id: Some(msg.id.0.to_string()),
                };
                let meta = ChannelMessageMeta {
                    chan_type: ChannelType::Telegram,
                    sender_name: sender_name.clone(),
                    username: username.clone(),
                    message_kind: message_kind(&msg),
                    model: config.model.clone(),
                };
                info!(
                    account_handle,
                    chat_id = %reply_target.chat_id,
                    message_id = ?reply_target.message_id,
                    body_len = body.len(),
                    "telegram inbound ingested (listen-only)"
                );
                sink.ingest_only(&body, reply_target, meta).await;
            }
        }

        return Ok(());
    }

    debug!(account_handle, "handler: access granted");

    // Check for voice/audio messages and transcribe them
    let (mut body, attachments) = if let Some(voice_file) = extract_voice_file(&msg) {
        // If STT is not configured, reply with guidance and do not dispatch to the LLM.
        if let Some(ref sink) = event_sink
            && !sink.voice_stt_available().await
        {
            if let Err(e) = outbound
                .send_text(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "I can't understand voice, you did not configure it, please visit Settings -> Voice",
                    None,
                )
                .await
            {
                warn!(account_handle, "failed to send STT setup hint: {e}");
            }
            return Ok(());
        }

        // Try to transcribe the voice message
        if let Some(ref sink) = event_sink {
            match download_telegram_file(bot, &voice_file.file_id).await {
                Ok(audio_data) => {
                    debug!(
                        account_handle,
                        file_id = %voice_file.file_id,
                        format = %voice_file.format,
                        size = audio_data.len(),
                        "downloaded voice file, transcribing"
                    );
                    match sink.transcribe_voice(&audio_data, &voice_file.format).await {
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
                        Err(e) => {
                            warn!(account_handle, error = %e, "voice transcription failed");
                            // Fall back to caption or indicate transcription failed
                            (
                                text.clone().unwrap_or_else(|| {
                                    "[Voice message - transcription unavailable]".to_string()
                                }),
                                Vec::new(),
                            )
                        },
                    }
                },
                Err(e) => {
                    warn!(account_handle, error = %e, "failed to download voice file");
                    (
                        text.clone()
                            .unwrap_or_else(|| "[Voice message - download failed]".to_string()),
                        Vec::new(),
                    )
                },
            }
        } else {
            // No event sink, can't transcribe
            (
                text.clone()
                    .unwrap_or_else(|| "[Voice message]".to_string()),
                Vec::new(),
            )
        }
    } else if let Some(photo_file) = extract_photo_file(&msg) {
        // Handle photo messages - download and send as multimodal content
        match download_telegram_file(bot, &photo_file.file_id).await {
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
                warn!(account_handle, error = %e, "failed to download photo");
                (
                    text.clone()
                        .unwrap_or_else(|| "[Photo - download failed]".to_string()),
                    Vec::new(),
                )
            },
        }
    } else if let Some(loc_info) = extract_location(&msg) {
        let lat = loc_info.latitude;
        let lon = loc_info.longitude;

        // Handle location sharing: update stored location and resolve any pending tool request.
        let resolved = if let Some(ref sink) = event_sink {
            let reply_target = ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: account_handle.to_string(),
                chan_user_name: bot_handle.clone(),
                chat_id: msg.chat.id.0.to_string(),
                message_id: Some(msg.id.0.to_string()),
            };
            sink.update_location(&reply_target, lat, lon).await
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
            if let Err(e) = outbound
                .send_text_silent(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "Location updated.",
                    None,
                )
                .await
            {
                warn!(account_handle, "failed to send location confirmation: {e}");
            }
            return Ok(());
        }

        if loc_info.is_live {
            // Live location share — acknowledge silently, subsequent updates arrive
            // as EditedMessage and are handled by handle_edited_location().
            if let Err(e) = outbound
                .send_text_silent(
                    account_handle,
                    &msg.chat.id.0.to_string(),
                    "Live location tracking started. Your location will be updated automatically.",
                    None,
                )
                .await
            {
                warn!(account_handle, "failed to send live location ack: {e}");
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

    // Dispatch to the chat session (per-channel session key derived by the sink).
    // The reply target tells the gateway where to send the LLM response back.
    let has_content = !body.is_empty() || !attachments.is_empty();
    if let Some(ref sink) = event_sink
        && has_content
    {
        let reply_target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: account_handle.to_string(),
            chan_user_name: bot_handle.clone(),
            chat_id: msg.chat.id.0.to_string(),
            message_id: Some(msg.id.0.to_string()),
        };

        info!(
            account_handle,
            chat_id = %reply_target.chat_id,
            message_id = ?reply_target.message_id,
            body_len = body.len(),
            attachment_count = attachments.len(),
            message_kind = ?inbound_kind,
            "telegram inbound dispatched to chat"
        );

        // Intercept slash commands before dispatching to the LLM.
        //
        // NOTE: Telegram supports addressed commands in groups: `/context@MyBot`.
        // We treat unaddressed commands in Group/Channel as ambiguous and ignore them
        // (avoid multi-bot spam), even if the account is configured with a permissive
        // `mention_mode`.
        if body.trim_start().starts_with('/') {
            let body_trim = body.trim_start();
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
                    (Some(_), _) => return Ok(()),
                    (None, _) => return Ok(()),
                }
            } else if let (Some(target), Some(me)) = (&addressed_norm, &bot_username_norm) {
                // DM: if the user explicitly addressed another bot, ignore.
                if target != me {
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
                    let context_result =
                        sink.dispatch_command("context", reply_target.clone()).await;
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        match context_result {
                            Ok(text) => {
                                send_context_card(
                                    &bot,
                                    &reply_target.chat_id,
                                    &text,
                                    &config,
                                    chat_type,
                                )
                                .await;
                            },
                            Err(e) => {
                                let _ = bot
                                    .send_message(
                                        ChatId(reply_target.chat_id.parse().unwrap_or(0)),
                                        format!("Error: {e}"),
                                    )
                                    .await;
                            },
                        }
                    }
                    return Ok(());
                }

                // For /model without args, send an inline keyboard to pick a model.
                if cmd_name == "model" && args.is_empty() {
                    let list_result = sink.dispatch_command("model", reply_target.clone()).await;
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        match list_result {
                            Ok(text) => {
                                send_model_keyboard(&bot, &reply_target.chat_id, &text).await;
                            },
                            Err(e) => {
                                let _ = bot
                                    .send_message(
                                        ChatId(reply_target.chat_id.parse().unwrap_or(0)),
                                        format!("Error: {e}"),
                                    )
                                    .await;
                            },
                        }
                    }
                    return Ok(());
                }

                // For /sandbox without args, send toggle + image keyboard.
                if cmd_name == "sandbox" && args.is_empty() {
                    let list_result = sink.dispatch_command("sandbox", reply_target.clone()).await;
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        match list_result {
                            Ok(text) => {
                                send_sandbox_keyboard(&bot, &reply_target.chat_id, &text).await;
                            },
                            Err(e) => {
                                let _ = bot
                                    .send_message(
                                        ChatId(reply_target.chat_id.parse().unwrap_or(0)),
                                        format!("Error: {e}"),
                                    )
                                    .await;
                            },
                        }
                    }
                    return Ok(());
                }

                // For /sessions without args, send an inline keyboard instead of plain text.
                if cmd_name == "sessions" && args.is_empty() {
                    let list_result = sink
                        .dispatch_command("sessions", reply_target.clone())
                        .await;
                    let bot = {
                        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                        accts.get(account_handle).map(|s| s.bot.clone())
                    };
                    if let Some(bot) = bot {
                        match list_result {
                            Ok(text) => {
                                send_sessions_keyboard(&bot, &reply_target.chat_id, &text).await;
                            },
                            Err(e) => {
                                let _ = bot
                                    .send_message(
                                        ChatId(reply_target.chat_id.parse().unwrap_or(0)),
                                        format!("Error: {e}"),
                                    )
                                    .await;
                            },
                        }
                    }
                    return Ok(());
                }

                let response = if cmd_name == "help" {
                    "Available commands:\n/new — Start a new session\n/sessions — List and switch sessions\n/model — Switch provider/model\n/sandbox — Toggle sandbox and choose image\n/clear — Clear session history\n/compact — Compact session (summarize)\n/context — Show session context info\n/help — Show this help".to_string()
                } else {
                    match sink.dispatch_command(&cmd_text, reply_target.clone()).await {
                        Ok(msg) => msg,
                        Err(e) => format!("Error: {e}"),
                    }
                };
                // Get the outbound Arc before awaiting (avoid holding RwLockReadGuard across await).
                let outbound = {
                    let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                    accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
                };
                if let Some(outbound) = outbound
                    && let Err(e) = outbound
                        .send_text(
                            account_handle,
                            &reply_target.chat_id,
                            &response,
                            reply_target.message_id.as_deref(),
                        )
                        .await
                {
                    warn!(account_handle, "failed to send command response: {e}");
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
                && let Err(e) = outbound
                    .send_text(
                        account_handle,
                        &reply_target.chat_id,
                        &response,
                        reply_target.message_id.as_deref(),
                    )
                    .await
            {
                warn!(account_handle, "failed to send command response: {e}");
            }
            return Ok(());
        }

        let tg_gst_v1 = chat_type == ChatType::Group
            && config.group_session_transcript_format
                == crate::config::GroupSessionTranscriptFormat::TgGstV1;

        if tg_gst_v1 {
            body = tg_gst_v1_apply_media_placeholder(message_kind(&msg), &body);

            if let Some(ref bot_username) = bot_username
                && attachments.is_empty()
                && bot_mentioned
                && tg_gst_v1_is_self_mention_only(&msg, &body, bot_user_id, bot_username)
            {
                // TG-GST v1: keep "@this_bot" as transcript context, while preserving the
                // legacy behavior of replying with a fixed short phrase (no LLM run).
                let speaker = tg_gst_v1_format_speaker(
                    username.as_deref(),
                    msg.from.as_ref().map(|u| u.id.0 as u64),
                    sender_name.as_deref(),
                    msg.from.as_ref().is_some_and(|u| u.is_bot),
                );
                let transcript = tg_gst_v1_format_line(&speaker, true, &body);

                let meta = ChannelMessageMeta {
                    chan_type: ChannelType::Telegram,
                    sender_name: sender_name.clone(),
                    username: username.clone(),
                    message_kind: message_kind(&msg),
                    model: config.model.clone(),
                };
                sink.ingest_only(&transcript, reply_target.clone(), meta).await;

                let outbound = {
                    let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
                    accts.get(account_handle).map(|s| Arc::clone(&s.outbound))
                };
                if let Some(outbound) = outbound
                    && let Err(e) = outbound
                        .send_text(
                            account_handle,
                            &reply_target.chat_id,
                            "我在。",
                            reply_target.message_id.as_deref(),
                        )
                        .await
                {
                    warn!(account_handle, "failed to send presence reply: {e}");
                }
                return Ok(());
            }

            let addressed = bot_mentioned;
            let speaker = tg_gst_v1_format_speaker(
                username.as_deref(),
                msg.from.as_ref().map(|u| u.id.0 as u64),
                sender_name.as_deref(),
                msg.from.as_ref().is_some_and(|u| u.is_bot),
            );
            body = tg_gst_v1_format_line(&speaker, addressed, &body);
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
                            && let Err(e) = outbound
                                .send_text(
                                    account_handle,
                                    &reply_target.chat_id,
                                    "我在。",
                                    reply_target.message_id.as_deref(),
                                )
                                .await
                        {
                            warn!(account_handle, "failed to send presence reply: {e}");
                        }
                        return Ok(());
                    }
                }
            }
        }

        let meta = ChannelMessageMeta {
            chan_type: ChannelType::Telegram,
            sender_name: sender_name.clone(),
            username: username.clone(),
            message_kind: message_kind(&msg),
            model: config.model.clone(),
        };

        if attachments.is_empty() {
            sink.dispatch_to_chat(&body, reply_target, meta).await;
        } else {
            sink.dispatch_to_chat_with_attachments(&body, attachments, reply_target, meta)
                .await;
        }
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

                let _ = bot
                    .send_message(chat_id, "Verified! You now have access to this bot.")
                    .await;

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
                let _ = bot
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
                    .await;

                #[cfg(feature = "metrics")]
                counter!(tg_metrics::OTP_VERIFICATIONS_TOTAL, "result" => "wrong_code")
                    .increment(1);
            },
            OtpVerifyResult::LockedOut => {
                let _ = bot
                    .send_message(chat_id, "Too many failed attempts. Please try again later.")
                    .await;

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
                let _ = bot
                    .send_message(
                        chat_id,
                        "Your code has expired. Send any message to get a new one.",
                    )
                    .await;

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
                let _ = bot
                    .send_message(chat_id, OTP_CHALLENGE_MSG)
                    .parse_mode(ParseMode::Html)
                    .await;

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
            OtpInitResult::AlreadyPending | OtpInitResult::LockedOut => {
                // Silent ignore.
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

    let (event_sink, bot_handle) = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        let handle = accts
            .get(account_handle)
            .and_then(|s| s.bot_username.as_deref().map(|u| format!("@{u}")));
        (
            accts.get(account_handle).and_then(|s| s.event_sink.clone()),
            handle,
        )
    };

    if let Some(ref sink) = event_sink {
        let reply_target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: account_handle.to_string(),
            chan_user_name: bot_handle,
            chat_id: msg.chat.id.0.to_string(),
            message_id: Some(msg.id.0.to_string()),
        };
        sink.update_location(&reply_target, lat, lon).await;
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
async fn send_sessions_keyboard(bot: &Bot, chat_id: &str, sessions_text: &str) {
    let chat = ChatId(chat_id.parse().unwrap_or(0));

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
                format!("sessions_switch:{n}"),
            )]);
        }
    }

    if buttons.is_empty() {
        let _ = bot.send_message(chat, sessions_text).await;
        return;
    }

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let _ = bot
        .send_message(chat, "Select a session:")
        .reply_markup(keyboard)
        .await;
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
) {
    let chat = ChatId(chat_id.parse().unwrap_or(0));

    // Preferred path: context.v1 JSON contract emitted by the gateway.
    if let Some(payload) = parse_context_v1_payload(context_text) {
        let html = render_context_card_v1(&payload, config, chat_type);
        let _ = bot
            .send_message(chat, html)
            .parse_mode(ParseMode::Html)
            .await;
        return;
    } else if context_text.trim_start().starts_with('{') {
        warn!(
            len = context_text.len(),
            "telegram /context: failed to parse context.v1 JSON, falling back to markdown"
        );
    }

    let html = render_context_card_markdown_fallback(context_text);

    let _ = bot
        .send_message(chat, html)
        .parse_mode(ParseMode::Html)
        .await;
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

    let group_reply_mode = match config.mention_mode {
        moltis_channels::gating::MentionMode::Mention => "mention_only",
        moltis_channels::gating::MentionMode::Always => "always",
    };
    let relay_strictness = match config.relay_strictness {
        crate::config::RelayStrictness::Strict => "strict",
        crate::config::RelayStrictness::Loose => "loose",
    };
    let group_scope_note = match chat_type {
        ChatType::Group => format!(
            "reply=<code>{}</code> · listen=<code>on</code> · mirror=<code>on</code> · relay=<code>on</code> · hop_limit=<code>{}</code> · strictness=<code>{}</code>",
            escape_html_simple(group_reply_mode),
            config.relay_hop_limit,
            escape_html_simple(relay_strictness),
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
async fn send_model_keyboard(bot: &Bot, chat_id: &str, text: &str) {
    let chat = ChatId(chat_id.parse().unwrap_or(0));

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
                    format!("model_provider:{provider_name}"),
                )]);
            } else {
                buttons.push(vec![InlineKeyboardButton::callback(
                    display,
                    format!("model_switch:{n}"),
                )]);
            }
        }
    }

    if buttons.is_empty() {
        let _ = bot.send_message(chat, "No models available.").await;
        return;
    }

    let heading = if is_provider_list {
        "🤖 Select a provider:"
    } else {
        "🤖 Select a model:"
    };

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let _ = bot.send_message(chat, heading).reply_markup(keyboard).await;
}

/// Send sandbox status with toggle button and image picker.
///
/// First line is `status:on` or `status:off`. Remaining lines are numbered
/// images, with `*` marking the current one.
async fn send_sandbox_keyboard(bot: &Bot, chat_id: &str, text: &str) {
    let chat = ChatId(chat_id.parse().unwrap_or(0));

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
                format!("sandbox_image:{n}"),
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
        "sandbox_toggle:off"
    } else {
        "sandbox_toggle:on"
    };

    let mut buttons = vec![vec![InlineKeyboardButton::callback(
        toggle_label.to_string(),
        toggle_action.to_string(),
    )]];
    buttons.extend(image_buttons);

    let keyboard = InlineKeyboardMarkup::new(buttons);
    let _ = bot
        .send_message(chat, "⚙️ Sandbox settings:")
        .reply_markup(keyboard)
        .await;
}

fn escape_html_simple(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Handle a Telegram callback query (inline keyboard button press).
pub async fn handle_callback_query(
    query: CallbackQuery,
    _bot: &Bot,
    account_handle: &str,
    accounts: &AccountStateMap,
) -> anyhow::Result<()> {
    let data = match query.data {
        Some(ref d) => d.as_str(),
        None => return Ok(()),
    };

    // Answer the callback to dismiss the loading spinner.
    let bot = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        accts.get(account_handle).map(|s| s.bot.clone())
    };

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
        if let Some(ref bot) = bot {
            let _ = bot.answer_callback_query(&query.id).await;
        }
        return Ok(());
    };

    let chat_id = query
        .message
        .as_ref()
        .map(|m| m.chat().id.0.to_string())
        .unwrap_or_default();

    if chat_id.is_empty() {
        return Ok(());
    }

    let (event_sink, outbound, bot_handle) = {
        let accts = accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = match accts.get(account_handle) {
            Some(s) => s,
            None => return Ok(()),
        };
        (
            state.event_sink.clone(),
            Arc::clone(&state.outbound),
            state.bot_username.as_deref().map(|u| format!("@{u}")),
        )
    };

    let reply_target = moltis_channels::ChannelReplyTarget {
        chan_type: ChannelType::Telegram,
        chan_account_key: account_handle.to_string(),
        chan_user_name: bot_handle,
        chat_id: chat_id.clone(),
        message_id: None, // Callback queries don't have a message to reply-thread to.
    };

    // Provider selection → fetch models for that provider and show a new keyboard.
    if let Some(provider_name) = data.strip_prefix("model_provider:") {
        if let Some(ref bot) = bot {
            let _ = bot.answer_callback_query(&query.id).await;
        }
        if let Some(ref sink) = event_sink {
            let cmd = format!("model provider:{provider_name}");
            match sink.dispatch_command(&cmd, reply_target).await {
                Ok(text) => {
                    if let Some(ref b) = bot {
                        send_model_keyboard(b, &chat_id, &text).await;
                    }
                },
                Err(e) => {
                    if let Err(err) = outbound
                        .send_text(account_handle, &chat_id, &format!("Error: {e}"), None)
                        .await
                    {
                        warn!(account_handle, "failed to send callback response: {err}");
                    }
                },
            }
        }
        return Ok(());
    }

    let Some(cmd_text) = cmd_text else {
        return Ok(());
    };

    if let Some(ref sink) = event_sink {
        let response = match sink.dispatch_command(&cmd_text, reply_target).await {
            Ok(msg) => msg,
            Err(e) => format!("Error: {e}"),
        };

        // Answer callback query with the response text (shows as toast).
        if let Some(ref bot) = bot {
            let _ = bot.answer_callback_query(&query.id).text(&response).await;
        }

        // Also send as a regular message for visibility.
        if let Err(e) = outbound
            .send_text(account_handle, &chat_id, &response, None)
            .await
        {
            warn!(account_handle, "failed to send callback response: {e}");
        }
    } else if let Some(ref bot) = bot {
        let _ = bot.answer_callback_query(&query.id).await;
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

/// Download a file from Telegram by file ID.
async fn download_telegram_file(bot: &Bot, file_id: &str) -> anyhow::Result<Vec<u8>> {
    // Get file info from Telegram
    let file = bot.get_file(file_id).await?;

    // Build the download URL
    // Telegram file URL format: https://api.telegram.org/file/bot<token>/<file_path>
    let token = bot.token();
    let url = format!("https://api.telegram.org/file/bot{}/{}", token, file.path);

    // Download using reqwest
    let response = reqwest::get(&url).await?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "failed to download file: HTTP {}",
            response.status()
        ));
    }

    let data = response.bytes().await?.to_vec();
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

fn tg_gst_v1_format_line(speaker: &str, addressed: bool, body: &str) -> String {
    let addr_flag = if addressed { " -> you" } else { "" };
    format!("{speaker}{addr_flag}: {body}")
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

fn tg_gst_v1_apply_media_placeholder(kind: Option<ChannelMessageKind>, body: &str) -> String {
    let body = body.to_string();
    let Some(kind) = kind else {
        return body;
    };
    if matches!(kind, ChannelMessageKind::Text) {
        return body;
    }

    let tag = match kind {
        ChannelMessageKind::Photo => "photo",
        ChannelMessageKind::Video => "video",
        ChannelMessageKind::Voice => "voice",
        ChannelMessageKind::Audio => "audio",
        ChannelMessageKind::Document => "file",
        ChannelMessageKind::Location => "location",
        _ => "attachment",
    };

    if body.trim().is_empty() {
        format!("[{tag}]")
    } else {
        format!("[{tag}] caption: {body}")
    }
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

#[allow(dead_code)]
fn build_chan_chat_key(chan_account_key: &str, chat_id: &str, thread_id: Option<&str>) -> String {
    let chan_user_id = chan_account_key
        .strip_prefix("telegram:")
        .unwrap_or(chan_account_key);
    moltis_common::identity::format_chan_chat_key("telegram", chan_user_id, chat_id, thread_id)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{
            collections::HashMap,
            sync::{Arc, Mutex},
        },
    };

    use {
        anyhow::Result,
        async_trait::async_trait,
        axum::{Json, Router, body::Bytes, extract::State, http::Uri, routing::post},
        moltis_channels::{ChannelEvent, ChannelEventSink, ChannelMessageMeta, ChannelReplyTarget},
        secrecy::Secret,
        serde::{Deserialize, Serialize},
        serde_json::json,
        tokio::sync::oneshot,
        tokio_util::sync::CancellationToken,
    };

    use crate::{
        config::TelegramAccountConfig,
        otp::OtpState,
        outbound::TelegramOutbound,
        state::{AccountState, AccountStateMap},
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TelegramApiMethod {
        SendMessage,
        SendChatAction,
        Other(String),
    }

    impl TelegramApiMethod {
        fn from_path(path: &str) -> Self {
            let method = path.rsplit('/').next().unwrap_or_default();
            match method {
                "SendMessage" => Self::SendMessage,
                "SendChatAction" => Self::SendChatAction,
                _ => Self::Other(method.to_string()),
            }
        }
    }

    #[derive(Debug, Clone)]
    enum CapturedTelegramRequest {
        SendMessage(SendMessageRequest),
        SendChatAction(SendChatActionRequest),
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

    #[derive(Debug, Serialize)]
    struct TelegramApiResponse {
        ok: bool,
        result: TelegramApiResult,
    }

    #[derive(Debug, Serialize)]
    #[serde(untagged)]
    enum TelegramApiResult {
        Message(TelegramMessageResult),
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
            TelegramApiMethod::SendChatAction | TelegramApiMethod::Other(_) => {
                Json(TelegramApiResponse {
                    ok: true,
                    result: TelegramApiResult::Bool(true),
                })
            },
        }
    }

    #[derive(Default)]
    struct MockSink {
        dispatch_calls: std::sync::atomic::AtomicUsize,
        ingest_calls: std::sync::atomic::AtomicUsize,
        command_calls: std::sync::atomic::AtomicUsize,
        last_dispatch_text: Mutex<Option<String>>,
        last_ingest_text: Mutex<Option<String>>,
        last_command: Mutex<Option<String>>,
    }

    #[async_trait]
    impl ChannelEventSink for MockSink {
        async fn emit(&self, _event: ChannelEvent) {}

        async fn dispatch_to_chat(
            &self,
            text: &str,
            _reply_to: ChannelReplyTarget,
            _meta: ChannelMessageMeta,
        ) {
            let mut last = self.last_dispatch_text.lock().unwrap();
            *last = Some(text.to_string());
            self.dispatch_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        async fn ingest_only(
            &self,
            text: &str,
            _reply_to: ChannelReplyTarget,
            _meta: ChannelMessageMeta,
        ) {
            let mut last = self.last_ingest_text.lock().unwrap();
            *last = Some(text.to_string());
            self.ingest_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        async fn dispatch_command(
            &self,
            command: &str,
            _reply_to: ChannelReplyTarget,
        ) -> anyhow::Result<String> {
            {
                let mut last = self.last_command.lock().unwrap();
                *last = Some(command.to_string());
            }
            self.command_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            Err(anyhow::anyhow!(
                "transcribe should not be called when STT unavailable"
            ))
        }

        async fn voice_stt_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn session_key_dm() {
        let key = build_chan_chat_key("telegram:bot1", "1001", None);
        assert_eq!(key, "telegram:bot1:1001");
    }

    #[test]
    fn session_key_group() {
        let key = build_chan_chat_key("telegram:bot1", "-100999", None);
        assert_eq!(key, "telegram:bot1:-100999");
    }

    #[test]
    fn session_key_thread() {
        let key = build_chan_chat_key("telegram:bot1", "-100999", Some("12"));
        assert_eq!(key, "telegram:bot1:-100999:12");
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
            .route("/{*path}", post(telegram_api_handler))
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
            .route("/{*path}", post(telegram_api_handler))
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        assert_eq!(last.as_deref(), Some("hello everyone"));
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
                group_session_transcript_format: crate::config::GroupSessionTranscriptFormat::TgGstV1,
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        assert_eq!(last.as_deref(), Some("alice: hello everyone"));
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        // Self-mention is stripped before dispatch.
        assert_eq!(last.as_deref(), Some("hi"));
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
                group_session_transcript_format: crate::config::GroupSessionTranscriptFormat::TgGstV1,
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        assert_eq!(last.as_deref(), Some("alice -> you: @test_bot hi"));
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
                group_session_transcript_format: crate::config::GroupSessionTranscriptFormat::TgGstV1,
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        assert_eq!(last.as_deref(), Some("alice -> you: @a @test_bot @c do X"));

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
        assert_eq!(last2.as_deref(), Some("alice -> you: @test_bot\n\n你处理下X"));
    }

    #[tokio::test]
    async fn group_self_mention_only_tg_gst_v1_ingests_and_presence_reply() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route("/{*path}", post(telegram_api_handler))
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
                group_session_transcript_format: crate::config::GroupSessionTranscriptFormat::TgGstV1,
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
        assert_eq!(last.as_deref(), Some("alice -> you: @test_bot"));

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
    async fn group_not_mentioned_always_respond_tg_gst_v1_dispatches_without_you_flag() {
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
                mention_mode: moltis_channels::gating::MentionMode::Always,
                group_session_transcript_format: crate::config::GroupSessionTranscriptFormat::TgGstV1,
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
            1,
            "always-respond must dispatch even when not mentioned"
        );
        let last = sink.last_dispatch_text.lock().unwrap().clone();
        assert_eq!(last.as_deref(), Some("alice: hello everyone"));
    }

    #[tokio::test]
    async fn addressed_slash_command_in_dm_is_intercepted() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route("/{*path}", post(telegram_api_handler))
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
    async fn addressed_slash_command_to_other_bot_in_dm_is_ignored() {
        let recorded_requests = Arc::new(Mutex::new(Vec::<CapturedTelegramRequest>::new()));
        let mock_api = MockTelegramApi {
            requests: Arc::clone(&recorded_requests),
        };
        let app = Router::new()
            .route("/{*path}", post(telegram_api_handler))
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
                    message_log: None,
                    event_sink: Some(Arc::clone(&sink) as Arc<dyn ChannelEventSink>),
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
                "key": "telegram:bot:group:123",
                "messageCount": 42,
                "provider": "openai-responses",
                "model": "openai-responses::gpt-5.2"
            },
            "llm": {
                "provider": "openai-responses",
                "model": "openai-responses::gpt-5.2",
                "overrides": {
                    "prompt_cache_key": "moltis:openai-responses:gpt-5.2:telegram:bot:group:123:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
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
        assert!(html.contains("telegram:bot:group:123"));
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
}

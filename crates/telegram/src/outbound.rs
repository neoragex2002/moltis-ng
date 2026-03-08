use {
    anyhow::Result,
    async_trait::async_trait,
    base64::Engine,
    teloxide::{
        payloads::{SendLocationSetters, SendMessageSetters, SendVenueSetters},
        prelude::*,
        types::{ChatAction, ChatId, InputFile, MessageId, ParseMode, ReplyParameters},
    },
    tracing::{debug, info, warn},
};

use {
    moltis_channels::plugin::{
        ChannelOutbound, ChannelStreamOutbound, SentMessageRef, StreamEvent, StreamReceiver,
    },
    moltis_common::types::ReplyPayload,
};

use crate::{
    markdown::{self, TELEGRAM_MAX_MESSAGE_LEN},
    state::AccountStateMap,
};

/// Outbound message sender for Telegram.
pub struct TelegramOutbound {
    pub(crate) accounts: AccountStateMap,
}

fn classify_request_error(e: &teloxide::RequestError) -> &'static str {
    match e {
        teloxide::RequestError::Api(_) => "api",
        teloxide::RequestError::Network(_) => "network",
        _ => "other",
    }
}

impl TelegramOutbound {
    fn get_bot(&self, account_handle: &str) -> Result<teloxide::Bot> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_handle)
            .map(|s| s.bot.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account_handle}"))
    }

    fn reply_params(
        &self,
        _account_handle: &str,
        reply_to: Option<&str>,
    ) -> Option<ReplyParameters> {
        parse_reply_params(reply_to)
    }

    async fn send_text_inner(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
        silent: bool,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, reply_to);

        // Send typing indicator
        if !silent {
            let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;
        }

        let html = markdown::markdown_to_telegram_html(text);
        let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);
        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            text_len = text.len(),
            chunk_count = chunks.len(),
            silent,
            "telegram outbound text send start"
        );

        let mut first_id: Option<MessageId> = None;
        for (i, chunk) in chunks.iter().enumerate() {
            let mut req = bot.send_message(chat_id, chunk).parse_mode(ParseMode::Html);
            if silent {
                req = req.disable_notification(true);
            }
            // Thread only the first chunk as a reply to the original message.
            if i == 0
                && let Some(ref rp) = rp
            {
                req = req.reply_parameters(rp.clone());
            }
            let sent = match req.await {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        event = "telegram.outbound.failed",
                        op = "send_message",
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        chunk_idx = i,
                        chunk_count = chunks.len(),
                        text_len = text.len(),
                        silent,
                        error_class = classify_request_error(&e),
                        error = %e,
                        "telegram outbound send_message failed"
                    );
                    return Err(e.into());
                },
            };
            if first_id.is_none() {
                first_id = Some(sent.id);
            }
        }

        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            text_len = text.len(),
            chunk_count = chunks.len(),
            silent,
            "telegram outbound text sent"
        );
        Ok(first_id)
    }

    async fn send_text_with_suffix_inner(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, reply_to);

        // Send typing indicator
        let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;

        let html = markdown::markdown_to_telegram_html(text);

        // Append the pre-formatted suffix (e.g. activity logbook) to the last chunk.
        let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);
        let last_idx = chunks.len().saturating_sub(1);
        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            text_len = text.len(),
            suffix_len = suffix_html.len(),
            chunk_count = chunks.len(),
            "telegram outbound text+suffix send start"
        );

        let mut first_id: Option<MessageId> = None;
        for (i, chunk) in chunks.iter().enumerate() {
            let content = if i == last_idx {
                // Append suffix to the last chunk. If it would exceed the limit,
                // the suffix becomes a separate final message.
                let combined = format!("{chunk}\n\n{suffix_html}");
                if combined.len() <= TELEGRAM_MAX_MESSAGE_LEN {
                    combined
                } else {
                    // Send this chunk first, then the suffix as a separate message.
                    let mut req = bot.send_message(chat_id, chunk).parse_mode(ParseMode::Html);
                    if i == 0
                        && let Some(ref rp) = rp
                    {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = match req.await {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(
                                event = "telegram.outbound.failed",
                                op = "send_message",
                                account_handle,
                                chat_id = to,
                                reply_to = ?reply_to,
                                chunk_idx = i,
                                chunk_count = chunks.len(),
                                text_len = text.len(),
                                suffix_len = suffix_html.len(),
                                error_class = classify_request_error(&e),
                                error = %e,
                                "telegram outbound send_message failed"
                            );
                            return Err(e.into());
                        },
                    };
                    if first_id.is_none() {
                        first_id = Some(sent.id);
                    }

                    // Send suffix as the final message (no reply threading).
                    if let Err(e) = bot
                        .send_message(chat_id, suffix_html)
                        .parse_mode(ParseMode::Html)
                        .disable_notification(true)
                        .await
                    {
                        warn!(
                            event = "telegram.outbound.failed",
                            op = "send_message_suffix",
                            account_handle,
                            chat_id = to,
                            reply_to = ?reply_to,
                            text_len = text.len(),
                            suffix_len = suffix_html.len(),
                            error_class = classify_request_error(&e),
                            error = %e,
                            "telegram outbound send_message failed (suffix)"
                        );
                        return Err(e.into());
                    }

                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        text_len = text.len(),
                        suffix_len = suffix_html.len(),
                        chunk_count = chunks.len(),
                        "telegram outbound text+suffix sent (separate suffix message)"
                    );
                    return Ok(first_id);
                }
            } else {
                chunk.clone()
            };
            let mut req = bot
                .send_message(chat_id, &content)
                .parse_mode(ParseMode::Html);
            if i == 0
                && let Some(ref rp) = rp
            {
                req = req.reply_parameters(rp.clone());
            }
            let sent = match req.await {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        event = "telegram.outbound.failed",
                        op = "send_message",
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        chunk_idx = i,
                        chunk_count = chunks.len(),
                        text_len = text.len(),
                        suffix_len = suffix_html.len(),
                        error_class = classify_request_error(&e),
                        error = %e,
                        "telegram outbound send_message failed"
                    );
                    return Err(e.into());
                },
            };
            if first_id.is_none() {
                first_id = Some(sent.id);
            }
        }

        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            text_len = text.len(),
            suffix_len = suffix_html.len(),
            chunk_count = chunks.len(),
            "telegram outbound text+suffix sent"
        );
        Ok(first_id)
    }

    async fn send_media_inner(
        &self,
        account_handle: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, reply_to);
        let media_mime = payload
            .media
            .as_ref()
            .map(|m| m.mime_type.as_str())
            .unwrap_or("none");
        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            has_media = payload.media.is_some(),
            media_mime,
            caption_len = payload.text.len(),
            "telegram outbound media send start"
        );

        if let Some(ref media) = payload.media {
            // Handle base64 data URIs (e.g., "data:image/png;base64,...")
            if media.url.starts_with("data:") {
                // Parse data URI: data:<mime>;base64,<data>
                let Some(comma_pos) = media.url.find(',') else {
                    anyhow::bail!("invalid data URI: no comma separator");
                };
                let base64_data = &media.url[comma_pos + 1..];
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64_data)
                    .map_err(|e| anyhow::anyhow!("failed to decode base64: {e}"))?;

                debug!(
                    bytes = bytes.len(),
                    mime_type = %media.mime_type,
                    "sending base64 media to telegram"
                );

                // Determine file extension
                let ext = match media.mime_type.as_str() {
                    "image/png" => "png",
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    _ => "bin",
                };
                let filename = format!("screenshot.{ext}");

                // For images, try as photo first, fall back to document on dimension errors
                if media.mime_type.starts_with("image/") {
                    let input = InputFile::memory(bytes.clone()).file_name(filename.clone());
                    let mut req = bot.send_photo(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }

                    match req.await {
                        Ok(sent) => {
                            info!(
                                account_handle,
                                chat_id = to,
                                reply_to = ?reply_to,
                                media_mime = %media.mime_type,
                                caption_len = payload.text.len(),
                                "telegram outbound media sent as photo"
                            );
                            return Ok(Some(sent.id));
                        },
                        Err(e) => {
                            let err_str = e.to_string();
                            // Retry as document if photo dimensions are invalid
                            if err_str.contains("PHOTO_INVALID_DIMENSIONS")
                                || err_str.contains("PHOTO_SAVE_FILE_INVALID")
                            {
                                debug!(
                                    error = %err_str,
                                    "photo rejected, retrying as document"
                                );
                                let input = InputFile::memory(bytes).file_name(filename);
                                let mut req = bot.send_document(chat_id, input);
                                if !payload.text.is_empty() {
                                    req = req.caption(&payload.text);
                                }
                                if let Some(ref rp) = rp {
                                    req = req.reply_parameters(rp.clone());
                                }
                                let sent = req.await?;
                                info!(
                                    account_handle,
                                    chat_id = to,
                                    reply_to = ?reply_to,
                                    media_mime = %media.mime_type,
                                    caption_len = payload.text.len(),
                                    "telegram outbound media sent as document fallback"
                                );
                                return Ok(Some(sent.id));
                            }
                            return Err(e.into());
                        },
                    }
                }

                // Non-image types: send as document/voice/audio depending on mime type.
                if media.mime_type == "audio/ogg" {
                    let input = InputFile::memory(bytes).file_name("voice.ogg");
                    let mut req = bot.send_voice(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound media sent as voice"
                    );
                    return Ok(Some(sent.id));
                }
                if media.mime_type.starts_with("audio/") {
                    let input = InputFile::memory(bytes).file_name("audio.mp3");
                    let mut req = bot.send_audio(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound media sent as audio"
                    );
                    return Ok(Some(sent.id));
                }

                let input = InputFile::memory(bytes).file_name(filename);
                let mut req = bot.send_document(chat_id, input);
                if !payload.text.is_empty() {
                    req = req.caption(&payload.text);
                }
                if let Some(ref rp) = rp {
                    req = req.reply_parameters(rp.clone());
                }
                let sent = req.await?;
                info!(
                    account_handle,
                    chat_id = to,
                    reply_to = ?reply_to,
                    media_mime = %media.mime_type,
                    caption_len = payload.text.len(),
                    "telegram outbound media sent as document"
                );
                return Ok(Some(sent.id));
            }

            // URL-based media
            let input = InputFile::url(media.url.parse()?);
            let sent = match media.mime_type.as_str() {
                t if t.starts_with("image/") => {
                    let mut req = bot.send_photo(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound URL media sent as photo"
                    );
                    sent.id
                },
                "audio/ogg" => {
                    let mut req = bot.send_voice(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound URL media sent as voice"
                    );
                    sent.id
                },
                t if t.starts_with("audio/") => {
                    let mut req = bot.send_audio(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound URL media sent as audio"
                    );
                    sent.id
                },
                _ => {
                    let mut req = bot.send_document(chat_id, input);
                    if !payload.text.is_empty() {
                        req = req.caption(&payload.text);
                    }
                    if let Some(ref rp) = rp {
                        req = req.reply_parameters(rp.clone());
                    }
                    let sent = req.await?;
                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        media_mime = %media.mime_type,
                        caption_len = payload.text.len(),
                        "telegram outbound URL media sent as document"
                    );
                    sent.id
                },
            };
            return Ok(Some(sent));
        }

        if !payload.text.is_empty() {
            let sent = self
                .send_text_inner(account_handle, to, &payload.text, reply_to, false)
                .await?;
            return Ok(sent);
        }

        Ok(None)
    }

    async fn send_location_inner(
        &self,
        account_handle: &str,
        to: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, reply_to);
        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            latitude,
            longitude,
            has_title = title.is_some(),
            "telegram outbound location send start"
        );

        let sent = if let Some(name) = title {
            // Venue shows the place name in the chat bubble.
            let address = format!("{latitude:.6}, {longitude:.6}");
            let mut req = bot.send_venue(chat_id, latitude, longitude, name, address);
            if let Some(ref rp) = rp {
                req = req.reply_parameters(rp.clone());
            }
            req.await?
        } else {
            let mut req = bot.send_location(chat_id, latitude, longitude);
            if let Some(ref rp) = rp {
                req = req.reply_parameters(rp.clone());
            }
            req.await?
        };

        info!(
            account_handle,
            chat_id = to,
            reply_to = ?reply_to,
            latitude,
            longitude,
            has_title = title.is_some(),
            "telegram outbound location sent"
        );
        Ok(Some(sent.id))
    }
}

/// Parse a platform message ID string into Telegram `ReplyParameters`.
/// Returns `None` if the string is not a valid i32 (Telegram message IDs are i32).
fn parse_reply_params(reply_to: Option<&str>) -> Option<ReplyParameters> {
    reply_to
        .and_then(|id| id.parse::<i32>().ok())
        .map(|id| ReplyParameters::new(MessageId(id)).allow_sending_without_reply())
}

#[async_trait]
impl ChannelOutbound for TelegramOutbound {
    async fn send_text(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_text_inner(account_handle, to, text, reply_to, false)
            .await?;
        Ok(())
    }

    async fn send_text_with_ref(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_inner(account_handle, to, text, reply_to, false)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_with_suffix(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_text_with_suffix_inner(account_handle, to, text, suffix_html, reply_to)
            .await?;
        Ok(())
    }

    async fn send_text_with_suffix_with_ref(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_with_suffix_inner(account_handle, to, text, suffix_html, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_typing(&self, account_handle: &str, to: &str) -> Result<()> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;
        Ok(())
    }

    async fn send_text_silent(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_text_inner(account_handle, to, text, reply_to, true)
            .await?;
        Ok(())
    }

    async fn send_text_silent_with_ref(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_inner(account_handle, to, text, reply_to, true)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_media(
        &self,
        account_handle: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_media_inner(account_handle, to, payload, reply_to)
            .await?;
        Ok(())
    }

    async fn send_media_with_ref(
        &self,
        account_handle: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_media_inner(account_handle, to, payload, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_location(
        &self,
        account_handle: &str,
        to: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_location_inner(account_handle, to, latitude, longitude, title, reply_to)
            .await?;
        Ok(())
    }

    async fn send_location_with_ref(
        &self,
        account_handle: &str,
        to: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_location_inner(account_handle, to, latitude, longitude, title, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }
}

impl TelegramOutbound {
    /// Send a `ReplyPayload` — dispatches to text or media.
    pub async fn send_reply(
        &self,
        bot: &teloxide::Bot,
        to: &str,
        payload: &ReplyPayload,
    ) -> Result<()> {
        let chat_id = ChatId(to.parse::<i64>()?);

        // Send typing indicator
        let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;

        if payload.media.is_some() {
            // Use the media path — but we need account_id, which we don't have here.
            // For direct bot usage, delegate to send_text for now.
            let html = markdown::markdown_to_telegram_html(&payload.text);
            let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);
            for chunk in chunks {
                bot.send_message(chat_id, &chunk)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        } else if !payload.text.is_empty() {
            let html = markdown::markdown_to_telegram_html(&payload.text);
            let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);
            for chunk in chunks {
                bot.send_message(chat_id, &chunk)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl ChannelStreamOutbound for TelegramOutbound {
    async fn send_stream(
        &self,
        account_handle: &str,
        to: &str,
        mut stream: StreamReceiver,
    ) -> Result<()> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);

        let throttle_ms = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            accounts
                .get(account_handle)
                .map(|s| s.config.edit_throttle_ms)
                .unwrap_or(300)
        };

        // Send typing indicator
        let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;

        // Send initial placeholder
        let placeholder = match bot
            .send_message(chat_id, "…")
            .parse_mode(ParseMode::Html)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    event = "telegram.outbound.failed",
                    op = "send_message_placeholder",
                    account_handle,
                    chat_id = to,
                    error_class = classify_request_error(&e),
                    error = %e,
                    "telegram outbound send_message failed (stream placeholder)"
                );
                return Err(e.into());
            },
        };
        let msg_id = placeholder.id;

        let mut accumulated = String::new();
        let mut last_edit = tokio::time::Instant::now();
        let throttle = std::time::Duration::from_millis(throttle_ms);
        let mut consecutive_edit_failures: u32 = 0;

        while let Some(event) = stream.recv().await {
            match event {
                StreamEvent::Delta(delta) => {
                    accumulated.push_str(&delta);
                    if last_edit.elapsed() >= throttle {
                        let html = markdown::markdown_to_telegram_html(&accumulated);
                        // Telegram rejects edits with identical content; truncate to limit.
                        let display = markdown::truncate_utf8(&html, TELEGRAM_MAX_MESSAGE_LEN);
                        let edit_res = bot
                            .edit_message_text(chat_id, msg_id, display)
                            .parse_mode(ParseMode::Html)
                            .await;
                        match edit_res {
                            Ok(_) => {
                                consecutive_edit_failures = 0;
                            },
                            Err(e) => {
                                consecutive_edit_failures = consecutive_edit_failures.saturating_add(1);
                                if consecutive_edit_failures == 1 || consecutive_edit_failures % 10 == 0 {
                                    warn!(
                                        event = "telegram.outbound.degraded",
                                        op = "edit_message_text",
                                        account_handle,
                                        chat_id = to,
                                        message_id = msg_id.0,
                                        consecutive_failures = consecutive_edit_failures,
                                        error_class = classify_request_error(&e),
                                        error = %e,
                                        "telegram outbound edit_message_text failed (streaming)"
                                    );
                                }
                            },
                        }
                        last_edit = tokio::time::Instant::now();
                    }
                },
                StreamEvent::Done => {
                    break;
                },
                StreamEvent::Error(e) => {
                    debug!("stream error: {e}");
                    accumulated.push_str(&format!("\n\n⚠ Error: {e}"));
                    break;
                },
            }
        }

        // Final edit with complete content
        if !accumulated.is_empty() {
            let html = markdown::markdown_to_telegram_html(&accumulated);
            let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);

            // Edit the placeholder with the first chunk
            if let Err(e) = bot
                .edit_message_text(chat_id, msg_id, &chunks[0])
                .parse_mode(ParseMode::Html)
                .await
            {
                warn!(
                    event = "telegram.outbound.failed",
                    op = "edit_message_text_final",
                    account_handle,
                    chat_id = to,
                    message_id = msg_id.0,
                    text_len = accumulated.len(),
                    chunk_count = chunks.len(),
                    error_class = classify_request_error(&e),
                    error = %e,
                    "telegram outbound final edit_message_text failed (streaming)"
                );
            }

            // Send remaining chunks as new messages
            for chunk in &chunks[1..] {
                if let Err(e) = bot
                    .send_message(chat_id, chunk)
                    .parse_mode(ParseMode::Html)
                    .await
                {
                    warn!(
                        event = "telegram.outbound.failed",
                        op = "send_message_stream_chunk",
                        account_handle,
                        chat_id = to,
                        message_id = msg_id.0,
                        chunk_count = chunks.len(),
                        chunk_len = chunk.len(),
                        error_class = classify_request_error(&e),
                        error = %e,
                        "telegram outbound send_message failed (streaming chunk)"
                    );
                    return Err(e.into());
                }
            }
        }

        Ok(())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{collections::HashMap, sync::Arc},
    };

    #[tokio::test]
    async fn send_location_unknown_account_returns_error() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };

        let result = outbound
            .send_location("nonexistent", "12345", 48.8566, 2.3522, Some("Paris"), None)
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("unknown account"),
            "should report unknown account"
        );
    }
}

use {
    anyhow::Result, async_trait::async_trait, moltis_common::types::ReplyPayload, tokio::sync::mpsc,
};

// ── Channel type enum ───────────────────────────────────────────────────────

/// Supported channel types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelType {
    Telegram,
    // Future: Discord, Slack, WhatsApp, etc.
}

impl ChannelType {
    /// Returns the channel type identifier as a string slice.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ChannelType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "telegram" => Ok(Self::Telegram),
            other => Err(format!("unknown channel type: {other}")),
        }
    }
}

// ── Channel events (pub/sub) ────────────────────────────────────────────────

/// Events emitted by channel plugins for real-time UI updates.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ChannelEvent {
    InboundMessage {
        chan_type: ChannelType,
        chan_account_key: String,
        peer_id: String,
        username: Option<String>,
        sender_name: Option<String>,
        message_count: Option<i64>,
        access_granted: bool,
    },
    /// A channel account was automatically disabled due to a runtime error.
    AccountDisabled {
        chan_type: ChannelType,
        chan_account_key: String,
        reason: String,
    },
    /// An OTP challenge was issued to a non-allowlisted DM user.
    OtpChallenge {
        chan_type: ChannelType,
        chan_account_key: String,
        peer_id: String,
        username: Option<String>,
        sender_name: Option<String>,
        code: String,
        expires_at: i64,
    },
    /// An OTP challenge was resolved (approved, locked out, or expired).
    OtpResolved {
        chan_type: ChannelType,
        chan_account_key: String,
        peer_id: String,
        username: Option<String>,
        resolution: String,
    },
}

/// Sink for channel events — the gateway provides the concrete implementation.
#[async_trait]
pub trait ChannelEventSink: Send + Sync {
    /// Broadcast a channel event for real-time UI updates.
    async fn emit(&self, event: ChannelEvent);

    /// Dispatch an inbound message to the main chat session (like sending
    /// from the web UI). The response is broadcast over WebSocket and
    /// routed back to the originating channel.
    async fn dispatch_to_chat(
        &self,
        text: &str,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    );

    /// Ingest an inbound message into the chat session history without
    /// triggering an LLM run or sending any outbound reply.
    ///
    /// This enables "listen/sidecar" group modes where only addressed
    /// messages generate replies, but all messages can still be recorded
    /// as context.
    async fn ingest_only(&self, text: &str, reply_to: ChannelReplyTarget, meta: ChannelMessageMeta);

    /// Dispatch a slash command (e.g. "new", "clear", "compact", "context")
    /// and return a text result to send back to the channel.
    async fn dispatch_command(
        &self,
        command: &str,
        reply_to: ChannelReplyTarget,
    ) -> anyhow::Result<String>;

    /// Request disabling a channel account due to a runtime error.
    ///
    /// This is used when the polling loop detects an unrecoverable error
    /// (e.g. another bot instance is running with the same token).
    async fn request_disable_account(&self, chan_type: &str, chan_account_key: &str, reason: &str);

    /// Request adding a sender to the allowlist (OTP self-approval).
    ///
    /// The gateway implementation calls `sender_approve` to persist the change
    /// and restart the account.
    async fn request_sender_approval(
        &self,
        _channel_type: &str,
        _account_handle: &str,
        _identifier: &str,
    ) {
    }

    /// Transcribe voice audio to text using the configured STT provider.
    ///
    /// Returns the transcribed text, or an error if transcription fails.
    /// The audio format is specified (e.g., "ogg", "mp3", "webm").
    async fn transcribe_voice(&self, audio_data: &[u8], format: &str) -> Result<String> {
        let _ = (audio_data, format);
        Err(anyhow::anyhow!("voice transcription not available"))
    }

    /// Whether voice STT is configured and available for channel audio messages.
    async fn voice_stt_available(&self) -> bool {
        true
    }

    /// Update the user's geolocation from a channel message (e.g. Telegram location share).
    ///
    /// Returns `true` if a pending tool-triggered location request was resolved.
    async fn update_location(
        &self,
        _reply_to: &ChannelReplyTarget,
        _latitude: f64,
        _longitude: f64,
    ) -> bool {
        false
    }

    /// Dispatch an inbound message with attachments (images, files) to the chat session.
    ///
    /// This is used when a channel message contains both text and media (e.g., a
    /// Telegram photo with a caption). The attachments are sent to the LLM as
    /// multimodal content.
    async fn dispatch_to_chat_with_attachments(
        &self,
        text: &str,
        attachments: Vec<ChannelAttachment>,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        // Default implementation ignores attachments and just sends text.
        let _ = attachments;
        self.dispatch_to_chat(text, reply_to, meta).await;
    }
}

/// Metadata about a channel message, used for UI display.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageMeta {
    pub chan_type: ChannelType,
    pub sender_name: Option<String>,
    pub username: Option<String>,
    /// Original inbound message media kind (voice, audio, photo, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_kind: Option<ChannelMessageKind>,
    /// Default model configured for this channel account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram: Option<ChannelTelegramMeta>,
}

/// Inbound channel message media kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMessageKind {
    Text,
    Voice,
    Audio,
    Photo,
    Document,
    Video,
    Location,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelTranscriptFormat {
    Legacy,
    TgGstV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramChatKind {
    Direct,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTelegramMeta {
    pub chat_kind: TelegramChatKind,
    pub transcript_format: ChannelTranscriptFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<u64>,
    pub sender_is_bot: bool,
    pub addressed: bool,
}

/// An attachment (image, file) from a channel message.
#[derive(Debug, Clone)]
pub struct ChannelAttachment {
    /// MIME type of the attachment (e.g., "image/jpeg", "image/png").
    pub media_type: String,
    /// Raw binary data of the attachment.
    pub data: Vec<u8>,
}

/// Where to send the LLM response back.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelReplyTarget {
    pub chan_type: ChannelType,
    pub chan_account_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_user_name: Option<String>,
    /// Chat/peer ID to send the reply to.
    pub chat_id: String,
    /// Platform-specific message ID of the inbound message.
    /// Used to thread replies (e.g. Telegram `reply_to_message_id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Optional topic/thread identifier for channels that support sub-thread delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Optional session bucket key selected by the channel adapter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_key: Option<String>,
}

/// Core channel plugin trait. Each messaging platform implements this.
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// Channel identifier (e.g. "telegram", "discord").
    fn id(&self) -> &str;

    /// Human-readable channel name.
    fn name(&self) -> &str;

    /// Start an account connection.
    async fn start_account(
        &mut self,
        chan_account_key: &str,
        config: serde_json::Value,
    ) -> Result<()>;

    /// Stop an account connection.
    async fn stop_account(&mut self, chan_account_key: &str) -> Result<()>;

    /// Get outbound adapter for sending messages.
    fn outbound(&self) -> Option<&dyn ChannelOutbound>;

    /// Get status adapter for health checks.
    fn status(&self) -> Option<&dyn ChannelStatus>;
}

/// Send messages to a channel.
///
/// `reply_to` is an optional platform-specific message ID that the outbound
/// message should thread as a reply to (e.g. Telegram `reply_to_message_id`).
#[async_trait]
pub trait ChannelOutbound: Send + Sync {
    async fn send_text(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<()>;

    /// Send a text message and return a best-effort reference to the sent message.
    ///
    /// For platforms that support it (e.g. Telegram), this should return the
    /// server-assigned message ID of the *primary* message (for chunked sends,
    /// the first chunk; for "text+suffix", the main text message).
    ///
    /// Default implementation calls `send_text()` and returns `None`.
    async fn send_text_with_ref(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        self.send_text(chan_account_key, to, text, reply_to).await?;
        Ok(None)
    }

    /// Send text to a structured channel target.
    async fn send_text_to_target(&self, target: &ChannelReplyTarget, text: &str) -> Result<()> {
        self.send_text(
            &target.chan_account_key,
            &target.chat_id,
            text,
            target.message_id.as_deref(),
        )
        .await
    }

    /// Send text to a structured target and return a best-effort message ref.
    async fn send_text_to_target_with_ref(
        &self,
        target: &ChannelReplyTarget,
        text: &str,
    ) -> Result<Option<SentMessageRef>> {
        self.send_text_with_ref(
            &target.chan_account_key,
            &target.chat_id,
            text,
            target.message_id.as_deref(),
        )
        .await
    }

    async fn send_media(
        &self,
        chan_account_key: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> Result<()>;

    /// Send media and return a best-effort reference to the sent message.
    ///
    /// Default implementation calls `send_media()` and returns `None`.
    async fn send_media_with_ref(
        &self,
        chan_account_key: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        self.send_media(chan_account_key, to, payload, reply_to)
            .await?;
        Ok(None)
    }

    /// Send media to a structured target.
    async fn send_media_to_target(
        &self,
        target: &ChannelReplyTarget,
        payload: &ReplyPayload,
    ) -> Result<()> {
        self.send_media(
            &target.chan_account_key,
            &target.chat_id,
            payload,
            target.message_id.as_deref(),
        )
        .await
    }

    /// Send media to a structured target and return a best-effort message ref.
    async fn send_media_to_target_with_ref(
        &self,
        target: &ChannelReplyTarget,
        payload: &ReplyPayload,
    ) -> Result<Option<SentMessageRef>> {
        self.send_media_with_ref(
            &target.chan_account_key,
            &target.chat_id,
            payload,
            target.message_id.as_deref(),
        )
        .await
    }
    /// Send a "typing" indicator. No-op by default.
    async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> Result<()> {
        Ok(())
    }
    /// Send a typing indicator to a structured target.
    async fn send_typing_to_target(&self, target: &ChannelReplyTarget) -> Result<()> {
        self.send_typing(&target.chan_account_key, &target.chat_id)
            .await
    }
    /// Send a text message with a pre-formatted HTML suffix appended after the main
    /// content. Used to attach a collapsible activity logbook to channel replies.
    /// The default implementation ignores the suffix and calls `send_text`.
    async fn send_text_with_suffix(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = suffix_html;
        self.send_text(chan_account_key, to, text, reply_to).await
    }

    /// Send text plus suffix to a structured target.
    async fn send_text_with_suffix_to_target(
        &self,
        target: &ChannelReplyTarget,
        text: &str,
        suffix_html: &str,
    ) -> Result<()> {
        self.send_text_with_suffix(
            &target.chan_account_key,
            &target.chat_id,
            text,
            suffix_html,
            target.message_id.as_deref(),
        )
        .await
    }

    /// Like `send_text_with_suffix`, but returns a best-effort sent message ref.
    ///
    /// Default implementation falls back to `send_text_with_ref` (ignores suffix).
    async fn send_text_with_suffix_with_ref(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let _ = suffix_html;
        self.send_text_with_ref(chan_account_key, to, text, reply_to)
            .await
    }

    /// Send text plus suffix to a structured target and return a best-effort ref.
    async fn send_text_with_suffix_to_target_with_ref(
        &self,
        target: &ChannelReplyTarget,
        text: &str,
        suffix_html: &str,
    ) -> Result<Option<SentMessageRef>> {
        self.send_text_with_suffix_with_ref(
            &target.chan_account_key,
            &target.chat_id,
            text,
            suffix_html,
            target.message_id.as_deref(),
        )
        .await
    }
    /// Send a text message without notification (silent). Falls back to send_text by default.
    async fn send_text_silent(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        self.send_text(chan_account_key, to, text, reply_to).await
    }

    /// Send silent text to a structured target.
    async fn send_text_silent_to_target(
        &self,
        target: &ChannelReplyTarget,
        text: &str,
    ) -> Result<()> {
        self.send_text_silent(
            &target.chan_account_key,
            &target.chat_id,
            text,
            target.message_id.as_deref(),
        )
        .await
    }

    /// Like `send_text_silent`, but returns a best-effort sent message ref.
    ///
    /// Default implementation falls back to `send_text_with_ref`.
    async fn send_text_silent_with_ref(
        &self,
        chan_account_key: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        self.send_text_with_ref(chan_account_key, to, text, reply_to)
            .await
    }

    /// Send silent text to a structured target and return a best-effort ref.
    async fn send_text_silent_to_target_with_ref(
        &self,
        target: &ChannelReplyTarget,
        text: &str,
    ) -> Result<Option<SentMessageRef>> {
        self.send_text_silent_with_ref(
            &target.chan_account_key,
            &target.chat_id,
            text,
            target.message_id.as_deref(),
        )
        .await
    }
    /// Send a native location pin to the channel.
    ///
    /// When `title` is provided, platforms that support it (e.g. Telegram) send
    /// a venue with the place name visible in the chat bubble. Otherwise a raw
    /// location pin is sent.
    ///
    /// Default implementation is a no-op so channels that don't support native
    /// location pins are unaffected.
    async fn send_location(
        &self,
        chan_account_key: &str,
        to: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let _ = (chan_account_key, to, latitude, longitude, title, reply_to);
        Ok(())
    }

    /// Like `send_location`, but returns a best-effort sent message ref.
    ///
    /// Default implementation calls `send_location()` and returns `None`.
    async fn send_location_with_ref(
        &self,
        chan_account_key: &str,
        to: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        self.send_location(chan_account_key, to, latitude, longitude, title, reply_to)
            .await?;
        Ok(None)
    }

    /// Send a native location to a structured target.
    async fn send_location_to_target(
        &self,
        target: &ChannelReplyTarget,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
    ) -> Result<()> {
        self.send_location(
            &target.chan_account_key,
            &target.chat_id,
            latitude,
            longitude,
            title,
            target.message_id.as_deref(),
        )
        .await
    }

    /// Send a native location to a structured target and return a best-effort ref.
    async fn send_location_to_target_with_ref(
        &self,
        target: &ChannelReplyTarget,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        self.send_location_with_ref(
            &target.chan_account_key,
            &target.chat_id,
            latitude,
            longitude,
            title,
            target.message_id.as_deref(),
        )
        .await
    }
}

/// Best-effort reference to an outbound message sent on a channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SentMessageRef {
    pub message_id: String,
}

/// Probe channel account health.
#[async_trait]
pub trait ChannelStatus: Send + Sync {
    async fn probe(&self, chan_account_key: &str) -> Result<ChannelHealthSnapshot>;
}

/// Channel health snapshot.
#[derive(Debug, Clone)]
pub struct ChannelHealthSnapshot {
    pub connected: bool,
    pub chan_account_key: String,
    pub details: Option<String>,
}

/// Stream event for edit-in-place streaming.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text to append.
    Delta(String),
    /// Stream is complete.
    Done,
    /// An error occurred.
    Error(String),
}

/// Receiver end of a stream channel.
pub type StreamReceiver = mpsc::Receiver<StreamEvent>;

/// Sender end of a stream channel.
pub type StreamSender = mpsc::Sender<StreamEvent>;

/// Streaming outbound — send responses via edit-in-place updates.
#[async_trait]
pub trait ChannelStreamOutbound: Send + Sync {
    /// Send a streaming response that updates a message in place.
    async fn send_stream(
        &self,
        chan_account_key: &str,
        to: &str,
        stream: StreamReceiver,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummySink;

    #[async_trait]
    impl ChannelEventSink for DummySink {
        async fn emit(&self, _event: ChannelEvent) {}

        async fn dispatch_to_chat(
            &self,
            _text: &str,
            _reply_to: ChannelReplyTarget,
            _meta: ChannelMessageMeta,
        ) {
        }

        async fn ingest_only(
            &self,
            _text: &str,
            _reply_to: ChannelReplyTarget,
            _meta: ChannelMessageMeta,
        ) {
        }

        async fn dispatch_command(
            &self,
            _command: &str,
            _reply_to: ChannelReplyTarget,
        ) -> anyhow::Result<String> {
            Ok(String::new())
        }

        async fn request_disable_account(
            &self,
            _channel_type: &str,
            _account_handle: &str,
            _reason: &str,
        ) {
        }
    }

    #[tokio::test]
    async fn default_voice_stt_available_is_true() {
        let sink = DummySink;
        assert!(sink.voice_stt_available().await);
    }

    #[tokio::test]
    async fn default_update_location_returns_false() {
        let sink = DummySink;
        let target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:1".into(),
            chan_user_name: None,
            chat_id: "42".into(),
            message_id: None,
        };
        assert!(!sink.update_location(&target, 48.8566, 2.3522).await);
    }

    struct DummyOutbound;

    #[async_trait]
    impl ChannelOutbound for DummyOutbound {
        async fn send_text(
            &self,
            _account_handle: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        async fn send_media(
            &self,
            _account_handle: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_send_location_is_noop() {
        let out = DummyOutbound;
        let result = out
            .send_location("acct", "42", 48.8566, 2.3522, Some("Eiffel Tower"), None)
            .await;
        assert!(result.is_ok());
    }
}

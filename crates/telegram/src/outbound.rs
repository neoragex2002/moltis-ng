use {
    anyhow::Result,
    async_trait::async_trait,
    base64::Engine,
    rand::Rng,
    std::future::Future,
    teloxide::{
        payloads::{
            SendAudioSetters, SendChatActionSetters, SendDocumentSetters, SendLocationSetters,
            SendMessageSetters, SendPhotoSetters, SendVenueSetters, SendVoiceSetters,
        },
        prelude::*,
        types::{ChatAction, ChatId, InputFile, MessageId, ParseMode, ReplyParameters, ThreadId},
    },
    tracing::{debug, info, warn},
};

use {
    moltis_channels::plugin::{
        ChannelOutbound, ChannelStreamOutbound, SentMessageRef, StreamEvent, StreamReceiver,
    },
    moltis_channels::{ChannelReplyTarget, ChannelType},
    moltis_common::types::ReplyPayload,
};

use crate::{
    adapter::{
        TgContent, TgInbound, TgInboundKind, TgInboundMode, TgInboundRequest, TgPrivateSource,
        TgPrivateTarget, plan_group_target_action, resolve_tg_route,
    },
    markdown::{self, TELEGRAM_MAX_MESSAGE_LEN},
    state::AccountStateMap,
};

#[cfg(not(test))]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);
#[cfg(test)]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(10);
#[cfg(not(test))]
const TELEGRAM_TYPING_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
#[cfg(test)]
const TELEGRAM_TYPING_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(20);

/// Outbound message sender for Telegram.
pub struct TelegramOutbound {
    pub(crate) accounts: AccountStateMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundOutcomeKind {
    DefinitiveFailure,
    UnknownOutcome,
    NonRetryableFailure,
}

impl OutboundOutcomeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefinitiveFailure => "definitive_failure",
            Self::UnknownOutcome => "unknown_outcome",
            Self::NonRetryableFailure => "non_retryable_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundDeliveryState {
    FirstChunkUnsent,
    PartialSent,
    PlaceholderUnsent,
    PlaceholderSent,
}

impl OutboundDeliveryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstChunkUnsent => "first_chunk_unsent",
            Self::PartialSent => "partial_sent",
            Self::PlaceholderUnsent => "placeholder_unsent",
            Self::PlaceholderSent => "placeholder_sent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramOutboundOp {
    SendMessage,
    SendMessageSuffix,
    SendMessagePlaceholder,
    EditMessageText,
    EditMessageTextFinal,
    SendMessageStreamChunk,
}

impl TelegramOutboundOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SendMessage => "send_message",
            Self::SendMessageSuffix => "send_message_suffix",
            Self::SendMessagePlaceholder => "send_message_placeholder",
            Self::EditMessageText => "edit_message_text",
            Self::EditMessageTextFinal => "edit_message_text_final",
            Self::SendMessageStreamChunk => "send_message_stream_chunk",
        }
    }

    const fn is_edit(self) -> bool {
        matches!(self, Self::EditMessageText | Self::EditMessageTextFinal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundErrorClass {
    Api,
    RetryAfter,
    Network,
    InvalidJson,
    Io,
    Other,
}

impl OutboundErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::RetryAfter => "retry_after",
            Self::Network => "network",
            Self::InvalidJson => "invalid_json",
            Self::Io => "io",
            Self::Other => "other",
        }
    }
}

#[derive(Debug)]
pub(crate) enum TypingRequestError {
    Request(teloxide::RequestError),
    Timeout,
}

impl std::fmt::Display for TypingRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(err) => write!(f, "{err}"),
            Self::Timeout => write!(f, "telegram send_typing timed out"),
        }
    }
}

impl std::error::Error for TypingRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(err) => Some(err),
            Self::Timeout => None,
        }
    }
}

#[derive(Debug)]
pub struct TelegramOutboundError {
    pub op: TelegramOutboundOp,
    pub outcome_kind: OutboundOutcomeKind,
    pub error_class: OutboundErrorClass,
    pub delivery_state: Option<OutboundDeliveryState>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub retry_after_secs: Option<u64>,
    source: Option<teloxide::RequestError>,
}

impl TelegramOutboundError {
    pub fn from_request_error(
        op: TelegramOutboundOp,
        delivery_state: Option<OutboundDeliveryState>,
        attempts: u32,
        max_attempts: u32,
        err: teloxide::RequestError,
    ) -> Self {
        Self {
            op,
            outcome_kind: classify_outcome_kind(op, &err),
            error_class: classify_request_error(&err),
            delivery_state,
            attempts,
            max_attempts,
            retry_after_secs: retry_after_secs(&err),
            source: Some(err),
        }
    }

    pub fn new_without_source(
        op: TelegramOutboundOp,
        outcome_kind: OutboundOutcomeKind,
        error_class: OutboundErrorClass,
        delivery_state: Option<OutboundDeliveryState>,
        attempts: u32,
        max_attempts: u32,
    ) -> Self {
        Self {
            op,
            outcome_kind,
            error_class,
            delivery_state,
            attempts,
            max_attempts,
            retry_after_secs: None,
            source: None,
        }
    }
}

impl std::fmt::Display for TelegramOutboundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "telegram outbound {} failed after {}/{} attempts ({}, {})",
            self.op.as_str(),
            self.attempts,
            self.max_attempts,
            self.error_class.as_str(),
            self.outcome_kind.as_str()
        )
    }
}

impl std::error::Error for TelegramOutboundError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|err| err as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug, Clone, Copy)]
struct OutboundRetryConfig {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

impl OutboundRetryConfig {
    fn from_account(cfg: &crate::config::TelegramAccountConfig) -> Self {
        Self {
            max_attempts: cfg.outbound_max_attempts.max(1),
            base_delay_ms: cfg.outbound_retry_base_delay_ms,
            max_delay_ms: cfg
                .outbound_retry_max_delay_ms
                .max(cfg.outbound_retry_base_delay_ms),
        }
    }
}

#[derive(Debug, Clone)]
struct SendMessageOptions {
    disable_notification: bool,
    reply_parameters: Option<ReplyParameters>,
    thread_id: Option<ThreadId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryDecision {
    Retry(std::time::Duration),
    SuccessEquivalent,
    GiveUp,
}

#[async_trait]
trait TelegramTextTransport: Send + Sync {
    async fn send_typing(
        &self,
        chat_id: ChatId,
        thread_id: Option<ThreadId>,
    ) -> std::result::Result<(), TypingRequestError>;

    async fn send_message_html(
        &self,
        chat_id: ChatId,
        text: &str,
        options: SendMessageOptions,
    ) -> std::result::Result<MessageId, teloxide::RequestError>;

    async fn edit_message_text_html(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: &str,
    ) -> std::result::Result<(), teloxide::RequestError>;
}

struct BotTextTransport {
    bot: teloxide::Bot,
}

#[async_trait]
impl TelegramTextTransport for BotTextTransport {
    async fn send_typing(
        &self,
        chat_id: ChatId,
        thread_id: Option<ThreadId>,
    ) -> std::result::Result<(), TypingRequestError> {
        send_chat_action_typing(&self.bot, chat_id, thread_id).await
    }

    async fn send_message_html(
        &self,
        chat_id: ChatId,
        text: &str,
        options: SendMessageOptions,
    ) -> std::result::Result<MessageId, teloxide::RequestError> {
        let mut req = self
            .bot
            .send_message(chat_id, text)
            .parse_mode(ParseMode::Html);
        if options.disable_notification {
            req = req.disable_notification(true);
        }
        if let Some(thread_id) = options.thread_id {
            req = req.message_thread_id(thread_id);
        }
        if let Some(rp) = options.reply_parameters {
            req = req.reply_parameters(rp);
        }
        req.await.map(|msg| msg.id)
    }

    async fn edit_message_text_html(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: &str,
    ) -> std::result::Result<(), teloxide::RequestError> {
        self.bot
            .edit_message_text(chat_id, message_id, text)
            .parse_mode(ParseMode::Html)
            .await
            .map(|_| ())
    }
}

fn classify_request_error(e: &teloxide::RequestError) -> OutboundErrorClass {
    match e {
        teloxide::RequestError::Api(_) => OutboundErrorClass::Api,
        teloxide::RequestError::RetryAfter(_) => OutboundErrorClass::RetryAfter,
        teloxide::RequestError::Network(_) => OutboundErrorClass::Network,
        teloxide::RequestError::InvalidJson { .. } => OutboundErrorClass::InvalidJson,
        teloxide::RequestError::Io(_) => OutboundErrorClass::Io,
        _ => OutboundErrorClass::Other,
    }
}

fn retry_after_secs(e: &teloxide::RequestError) -> Option<u64> {
    match e {
        teloxide::RequestError::RetryAfter(secs) => Some(secs.seconds() as u64),
        _ => None,
    }
}

fn typing_reply_target(target: &TgPrivateTarget) -> ChannelReplyTarget {
    ChannelReplyTarget {
        chan_type: ChannelType::Telegram,
        chan_account_key: target.account_handle.clone(),
        chan_user_name: None,
        chat_id: target.chat_id.clone(),
        message_id: None,
        thread_id: target.thread_id.clone(),
        bucket_key: None,
    }
}

pub(crate) async fn send_chat_action_typing(
    bot: &teloxide::Bot,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
) -> std::result::Result<(), TypingRequestError> {
    let mut req = bot.send_chat_action(chat_id, ChatAction::Typing);
    if let Some(thread_id) = thread_id {
        req = req.message_thread_id(thread_id);
    }
    match tokio::time::timeout(TELEGRAM_TYPING_REQUEST_TIMEOUT, req).await {
        Ok(result) => result.map(|_| ()).map_err(TypingRequestError::Request),
        Err(_) => Err(TypingRequestError::Timeout),
    }
}

pub async fn run_with_targeted_typing_loop<T>(
    outbound: std::sync::Arc<dyn ChannelOutbound>,
    target: TgPrivateTarget,
    op: &'static str,
    keepalive_interval: std::time::Duration,
    operation: impl Future<Output = T>,
) -> T {
    async fn send_typing_once(
        outbound: &std::sync::Arc<dyn ChannelOutbound>,
        target: &TgPrivateTarget,
        op: &'static str,
        typing_failed: &mut bool,
    ) {
        let reply_target = typing_reply_target(target);
        match outbound.send_typing_to_target(&reply_target).await {
            Ok(()) => {
                if *typing_failed {
                    info!(
                        event = "telegram.typing.recovered",
                        op,
                        chan_account_key = target.account_handle,
                        chat_id = target.chat_id,
                        "typing indicator recovered"
                    );
                    *typing_failed = false;
                }
            },
            Err(error) => {
                if !*typing_failed {
                    warn!(
                        event = "telegram.typing.failed",
                        op,
                        chan_account_key = target.account_handle,
                        chat_id = target.chat_id,
                        reason_code = "send_typing_failed",
                        error = %error,
                        "typing indicator failed"
                    );
                    *typing_failed = true;
                } else {
                    debug!(
                        event = "telegram.typing.failed",
                        op,
                        chan_account_key = target.account_handle,
                        chat_id = target.chat_id,
                        reason_code = "send_typing_failed",
                        "typing indicator still failing"
                    );
                }
            },
        }
    }

    let operation = operation;
    let typing_loop = async {
        let mut typing_failed = false;
        send_typing_once(&outbound, &target, op, &mut typing_failed).await;

        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + keepalive_interval,
            keepalive_interval,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            keepalive.tick().await;
            send_typing_once(&outbound, &target, op, &mut typing_failed).await;
        }
    };

    tokio::pin!(operation);
    tokio::pin!(typing_loop);

    tokio::select! {
        result = &mut operation => result,
        _ = &mut typing_loop => unreachable!("telegram typing loop must stay pending until the operation completes"),
    }
}

pub fn spawn_targeted_typing_loop_until(
    outbound: std::sync::Arc<dyn ChannelOutbound>,
    target: TgPrivateTarget,
    op: &'static str,
    keepalive_interval: std::time::Duration,
    completion: impl Future<Output = ()> + Send + 'static,
) {
    tokio::spawn(async move {
        async fn send_typing_once(
            outbound: &std::sync::Arc<dyn ChannelOutbound>,
            target: &TgPrivateTarget,
            op: &'static str,
            typing_failed: &mut bool,
        ) {
            let reply_target = typing_reply_target(target);
            match outbound.send_typing_to_target(&reply_target).await {
                Ok(()) => {
                    if *typing_failed {
                        info!(
                            event = "telegram.typing.recovered",
                            op,
                            chan_account_key = target.account_handle,
                            chat_id = target.chat_id,
                            "typing indicator recovered"
                        );
                        *typing_failed = false;
                    }
                },
                Err(error) => {
                    if !*typing_failed {
                        warn!(
                            event = "telegram.typing.failed",
                            op,
                            chan_account_key = target.account_handle,
                            chat_id = target.chat_id,
                            reason_code = "send_typing_failed",
                            error = %error,
                            "typing indicator failed"
                        );
                        *typing_failed = true;
                    } else {
                        debug!(
                            event = "telegram.typing.failed",
                            op,
                            chan_account_key = target.account_handle,
                            chat_id = target.chat_id,
                            reason_code = "send_typing_failed",
                            "typing indicator still failing"
                        );
                    }
                },
            }
        }

        let mut typing_failed = false;
        tokio::pin!(completion);

        tokio::select! {
            _ = &mut completion => return,
            _ = send_typing_once(&outbound, &target, op, &mut typing_failed) => {}
        }

        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + keepalive_interval,
            keepalive_interval,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut completion => break,
                _ = keepalive.tick() => {
                    send_typing_once(&outbound, &target, op, &mut typing_failed).await;
                },
            }
        }
    });
}

async fn send_typing_keepalive<T: TelegramTextTransport + ?Sized>(
    transport: &T,
    account_handle: &str,
    to: &str,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
    op: &'static str,
    typing_failed: &mut bool,
) {
    match transport.send_typing(chat_id, thread_id).await {
        Ok(()) => {
            if *typing_failed {
                info!(
                    event = "telegram.typing.recovered",
                    op,
                    account_handle,
                    chat_id = to,
                    "typing indicator recovered"
                );
                *typing_failed = false;
            }
        },
        Err(error) => {
            let (error_class, reason_code) = match &error {
                TypingRequestError::Request(request_error) => (
                    classify_request_error(request_error).as_str(),
                    "send_typing_failed",
                ),
                TypingRequestError::Timeout => ("timeout", "send_typing_timeout"),
            };
            if !*typing_failed {
                warn!(
                    event = "telegram.typing.failed",
                    op,
                    account_handle,
                    chat_id = to,
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
                    chat_id = to,
                    reason_code,
                    error_class,
                    "typing indicator still failing"
                );
            }
        },
    }
}

fn classify_outcome_kind(
    _op: TelegramOutboundOp,
    err: &teloxide::RequestError,
) -> OutboundOutcomeKind {
    match err {
        teloxide::RequestError::RetryAfter(_) => OutboundOutcomeKind::DefinitiveFailure,
        teloxide::RequestError::Network(_) => OutboundOutcomeKind::UnknownOutcome,
        teloxide::RequestError::InvalidJson { .. } => OutboundOutcomeKind::UnknownOutcome,
        teloxide::RequestError::Api(_) | teloxide::RequestError::Io(_) => {
            OutboundOutcomeKind::NonRetryableFailure
        },
        _ => OutboundOutcomeKind::NonRetryableFailure,
    }
}

fn compute_backoff_delay(cfg: OutboundRetryConfig, attempt: u32) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(10);
    let multiplier = 1u64 << shift;
    let base = cfg
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(cfg.max_delay_ms);
    let jitter_cap = base.min(250);
    let jitter = if jitter_cap == 0 {
        0
    } else {
        rand::rng().random_range(0..=jitter_cap)
    };
    std::time::Duration::from_millis(base.saturating_add(jitter))
}

fn classify_retry_decision(
    op: TelegramOutboundOp,
    err: &teloxide::RequestError,
    attempt: u32,
    cfg: OutboundRetryConfig,
) -> RetryDecision {
    if attempt >= cfg.max_attempts {
        return RetryDecision::GiveUp;
    }

    match err {
        teloxide::RequestError::RetryAfter(secs) => RetryDecision::Retry(secs.duration()),
        teloxide::RequestError::Network(_)
            if matches!(
                op,
                TelegramOutboundOp::SendMessage
                    | TelegramOutboundOp::SendMessageSuffix
                    | TelegramOutboundOp::SendMessagePlaceholder
                    | TelegramOutboundOp::SendMessageStreamChunk
                    | TelegramOutboundOp::EditMessageText
                    | TelegramOutboundOp::EditMessageTextFinal
            ) =>
        {
            RetryDecision::Retry(compute_backoff_delay(cfg, attempt))
        },
        teloxide::RequestError::Api(teloxide::ApiError::MessageNotModified) if op.is_edit() => {
            RetryDecision::SuccessEquivalent
        },
        _ => RetryDecision::GiveUp,
    }
}

impl TelegramOutbound {
    pub fn new(accounts: AccountStateMap) -> Self {
        Self { accounts }
    }

    fn get_bot(&self, account_handle: &str) -> Result<teloxide::Bot> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_handle)
            .map(|s| s.bot.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account_handle}"))
    }

    fn get_retry_config(&self, account_handle: &str) -> Result<OutboundRetryConfig> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = accounts
            .get(account_handle)
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account_handle}"))?;
        Ok(OutboundRetryConfig::from_account(&state.config))
    }

    fn is_group_chat(chat_id: &str) -> bool {
        chat_id.parse::<i64>().is_ok_and(|id| id < 0)
    }

    fn group_action_dedupe_key(account_handle: &str, chat_id: &str, message_id: &str) -> String {
        format!(
            "telegram.group.action|account:{account_handle}|chat:{chat_id}|message:{message_id}"
        )
    }

    fn register_group_visible_outbound_contexts(
        &self,
        account_handle: &str,
        chat_id: &str,
        lineage_message_id: Option<&str>,
        sent_message_ids: &[String],
    ) {
        if !Self::is_group_chat(chat_id) || sent_message_ids.is_empty() {
            return;
        }
        let Some(lineage_message_id) = lineage_message_id else {
            return;
        };
        let binding = crate::state::shared_group_runtime(&self.accounts);
        let mut runtime = binding.lock().unwrap_or_else(|e| e.into_inner());
        let Some(root_message_id) = runtime.inherited_root_message_id(chat_id, lineage_message_id)
        else {
            return;
        };
        runtime.register_participant(chat_id, account_handle);
        let _ = runtime.register_sent_message_contexts(
            chat_id,
            sent_message_ids,
            account_handle,
            &root_message_id,
        );
    }

    async fn emit_group_visible_outbound_event(
        &self,
        source_account_handle: &str,
        chat_id: &str,
        reply_to_message_id: Option<&str>,
        lineage_message_id: Option<&str>,
        thread_id: Option<&str>,
        text: &str,
        message_kind: Option<moltis_channels::ChannelMessageKind>,
        sent_message_id: &str,
    ) {
        if !Self::is_group_chat(chat_id)
            || (text.trim().is_empty()
                && !message_kind
                    .is_some_and(|kind| kind != moltis_channels::ChannelMessageKind::Text))
        {
            return;
        }

        self.register_group_visible_outbound_contexts(
            source_account_handle,
            chat_id,
            lineage_message_id,
            &[sent_message_id.to_string()],
        );

        let (
            snapshots,
            identity_links,
            source_username,
            source_sender_name,
            source_sender_id,
            source_sender_token,
            participants,
        ) = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            let Some(source_state) = accounts.get(source_account_handle) else {
                return;
            };
            let source_sender_token = crate::state::effective_bot_user_id(source_state)
                .map(|id| id.to_string())
                .or_else(|| crate::state::effective_bot_username(source_state));
            let snapshots = accounts
                .iter()
                .map(
                    |(account_handle, state)| crate::config::TelegramBusAccountSnapshot {
                        account_handle: account_handle.clone(),
                        agent_id: state.config.agent_id.clone(),
                        chan_user_id: crate::state::effective_bot_user_id(state),
                        chan_user_name: crate::state::effective_bot_username(state),
                        chan_nickname: state.config.chan_nickname.clone(),
                        dm_scope: state.config.dm_scope.clone(),
                        group_scope: state.config.group_scope.clone(),
                    },
                )
                .collect::<Vec<_>>();
            let identity_links = crate::state::telegram_identity_links_snapshot(&self.accounts);
            let group_runtime = crate::state::shared_group_runtime(&self.accounts);
            let mut runtime = group_runtime.lock().unwrap_or_else(|e| e.into_inner());
            runtime.register_participant(chat_id, source_account_handle);
            let participants = runtime.participants_for_chat(chat_id);
            (
                snapshots,
                identity_links,
                crate::state::effective_bot_username(source_state),
                source_state
                    .config
                    .chan_nickname
                    .clone()
                    .or_else(|| crate::state::effective_bot_username(source_state)),
                crate::state::effective_bot_user_id(source_state),
                source_sender_token,
                participants,
            )
        };

        if participants.is_empty() {
            return;
        }

        let reply_to_target_account = reply_to_message_id.and_then(|message_id| {
            crate::state::shared_group_runtime(&self.accounts)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .message_author(chat_id, message_id)
        });

        let targets = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            participants
                .into_iter()
                .filter(|account_handle| account_handle != source_account_handle)
                .filter_map(|account_handle| {
                    let state = accounts.get(&account_handle)?;
                    Some((
                        account_handle,
                        state.config.clone(),
                        state.bot_username.clone(),
                        state.core_bridge.clone(),
                    ))
                })
                .collect::<Vec<_>>()
        };

        for (target_account_handle, target_config, target_username, target_bridge) in targets {
            let Some(bridge) = target_bridge else {
                continue;
            };
            let Some(action) = plan_group_target_action(
                text,
                &snapshots,
                &target_account_handle,
                target_username.as_deref(),
                reply_to_target_account.as_deref(),
                target_config.group_line_start_mention_dispatch,
                target_config.group_reply_to_dispatch,
                message_kind.is_some_and(|kind| kind != moltis_channels::ChannelMessageKind::Text),
            ) else {
                continue;
            };

            let dedupe_key =
                Self::group_action_dedupe_key(&target_account_handle, chat_id, sent_message_id);
            let is_dup = crate::state::shared_group_runtime(&self.accounts)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .check_and_insert_action(&dedupe_key);
            if is_dup {
                info!(
                    event = "telegram.group.outbound_plan",
                    source_account_handle,
                    target_account_handle,
                    chat_id,
                    message_id = sent_message_id,
                    reason_code = "tg_dedup_hit",
                    decision = "drop",
                    policy = "group_record_dispatch_v3",
                    "telegram group outbound event deduped before gateway handoff"
                );
                continue;
            }

            let action = if matches!(action.mode, TgInboundMode::Dispatch) {
                let admission = crate::state::shared_group_runtime(&self.accounts)
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .admit_managed_dispatch(chat_id, sent_message_id);
                match admission {
                    Some(admission) if admission.allowed => action,
                    Some(admission) => {
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
                                thread_id = thread_id,
                                source_account_handle,
                                target_account_handle,
                                message_id = sent_message_id,
                                "telegram group outbound dispatch downgraded by root dispatch fuse"
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
                                thread_id = thread_id,
                                source_account_handle,
                                target_account_handle,
                                message_id = sent_message_id,
                                "telegram group outbound dispatch downgraded by root dispatch fuse"
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
                            thread_id = thread_id,
                            source_account_handle,
                            target_account_handle,
                            message_id = sent_message_id,
                            "telegram group outbound dispatch downgraded because root context is missing"
                        );
                        crate::adapter::TgGroupTargetAction {
                            mode: TgInboundMode::RecordOnly,
                            ..action
                        }
                    },
                }
            } else {
                action
            };

            let inbound = TgInbound {
                kind: TgInboundKind::Group,
                mode: action.mode,
                body: TgContent {
                    text: action.body,
                    has_attachments: false,
                    has_location: matches!(
                        message_kind,
                        Some(moltis_channels::ChannelMessageKind::Location)
                    ),
                },
                private_source: TgPrivateSource {
                    account_handle: target_account_handle.clone(),
                    chat_id: chat_id.to_string(),
                    message_id: Some(sent_message_id.to_string()),
                    thread_id: thread_id.map(str::to_string),
                    peer: chat_id.to_string(),
                    sender: source_sender_token.clone(),
                    addressed: action.addressed,
                },
            };
            let route = resolve_tg_route(&target_config, &inbound);
            let rendered = crate::adapter::tg_gst_v1_render_text(
                inbound.body.text.as_str(),
                message_kind,
                &snapshots,
                &identity_links,
                source_username.as_deref(),
                source_sender_id,
                source_sender_name.as_deref(),
                true,
                route.addressed,
            );
            if rendered.degraded || rendered.disambiguated {
                info!(
                    event = "telegram.speaker_resolution",
                    account_handle = target_account_handle,
                    reason_code = rendered.reason_code,
                    decision = if rendered.degraded {
                        "degraded"
                    } else {
                        "disambiguated"
                    },
                    policy = "tg_gst_v1_speaker",
                    match_method = rendered.match_method,
                    collision = rendered.disambiguated,
                    sender_short_id = source_sender_id.map(|id| id % 100000),
                    "telegram outbound speaker rendering required fallback or disambiguation"
                );
            }
            let mut inbound = inbound;
            inbound.body.text = rendered.text;
            let request = TgInboundRequest {
                inbound,
                route: route.clone(),
                private_target: TgPrivateTarget {
                    account_handle: target_account_handle.clone(),
                    chat_id: chat_id.to_string(),
                    message_id: Some(sent_message_id.to_string()),
                    thread_id: thread_id.map(str::to_string),
                },
                transcript_format: crate::adapter::TgTranscriptFormat::TgGstV1,
                sender_name: source_sender_name.clone(),
                username: source_username.clone(),
                sender_id: source_sender_id,
                sender_is_bot: true,
                model: target_config.model.clone(),
                message_kind,
                attachments: Vec::new(),
            };
            info!(
                event = "telegram.group.outbound_plan",
                source_account_handle,
                target_account_handle,
                chat_id,
                message_id = sent_message_id,
                reason_code = if matches!(request.inbound.mode, TgInboundMode::Dispatch) {
                    "tg_dispatch_promoted_from_record"
                } else {
                    "tg_record_context"
                },
                decision = if matches!(request.inbound.mode, TgInboundMode::Dispatch) {
                    "dispatch"
                } else {
                    "record"
                },
                policy = "group_record_dispatch_v3",
                "telegram group outbound event handed to gateway"
            );
            bridge.handle_inbound(request).await;
        }
    }

    fn reply_params(
        &self,
        account_handle: &str,
        chat_id_text: &str,
        op: &str,
        reply_to: Option<&str>,
    ) -> Option<ReplyParameters> {
        let (rp, invalid) = parse_reply_params_checked(reply_to);
        if invalid {
            warn!(
                event = "telegram.outbound.thread_target_invalid",
                op,
                account_handle,
                chat_id = chat_id_text,
                reply_to = ?reply_to,
                reason_code = "reply_to_invalid",
                "invalid reply_to message id; sending without threading"
            );
        }
        rp
    }

    fn thread_id(
        &self,
        account_handle: &str,
        chat_id_text: &str,
        op: &str,
        thread_id: Option<&str>,
    ) -> Option<ThreadId> {
        let Some(thread_id) = thread_id else {
            return None;
        };
        match thread_id.parse::<i32>() {
            Ok(thread_id) => Some(ThreadId(MessageId(thread_id))),
            Err(_) => {
                warn!(
                    event = "telegram.outbound.thread_target_invalid",
                    op,
                    account_handle,
                    chat_id = chat_id_text,
                    thread_id = ?thread_id,
                    reason_code = "thread_id_invalid",
                    "invalid thread id; sending without topic targeting"
                );
                None
            },
        }
    }

    async fn send_message_with_retry<T: TelegramTextTransport>(
        &self,
        transport: &T,
        retry_cfg: OutboundRetryConfig,
        account_handle: &str,
        chat_id: ChatId,
        chat_id_text: &str,
        op: TelegramOutboundOp,
        text: &str,
        options: SendMessageOptions,
        delivery_state: Option<OutboundDeliveryState>,
        reply_to: Option<&str>,
        chunk_idx: Option<usize>,
        chunk_count: Option<usize>,
        message_id: Option<MessageId>,
        text_len: usize,
        suffix_len: Option<usize>,
    ) -> Result<MessageId> {
        let mut attempt = 1;
        loop {
            match transport
                .send_message_html(chat_id, text, options.clone())
                .await
            {
                Ok(message_id_sent) => return Ok(message_id_sent),
                Err(err) => match classify_retry_decision(op, &err, attempt, retry_cfg) {
                    RetryDecision::Retry(delay) => {
                        warn!(
                            event = "telegram.outbound.retrying",
                            op = op.as_str(),
                            account_handle,
                            chat_id = chat_id_text,
                            reply_to = ?reply_to,
                            message_id = message_id.map(|id| id.0),
                            chunk_idx,
                            chunk_count,
                            text_len,
                            suffix_len,
                            attempt,
                            max_attempts = retry_cfg.max_attempts,
                            error_class = classify_request_error(&err).as_str(),
                            outcome_kind = classify_outcome_kind(op, &err).as_str(),
                            retry_after_secs = retry_after_secs(&err),
                            retry_delay_ms = delay.as_millis() as u64,
                            delivery_state = delivery_state.map(OutboundDeliveryState::as_str),
                            error = %err,
                            "telegram outbound retrying send_message"
                        );
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        attempt += 1;
                    },
                    RetryDecision::GiveUp => {
                        let outbound_err = TelegramOutboundError::from_request_error(
                            op,
                            delivery_state,
                            attempt,
                            retry_cfg.max_attempts,
                            err,
                        );
                        warn!(
                            event = "telegram.outbound.gave_up",
                            op = op.as_str(),
                            account_handle,
                            chat_id = chat_id_text,
                            reply_to = ?reply_to,
                            message_id = message_id.map(|id| id.0),
                            chunk_idx,
                            chunk_count,
                            text_len,
                            suffix_len,
                            attempt,
                            max_attempts = retry_cfg.max_attempts,
                            error_class = outbound_err.error_class.as_str(),
                            outcome_kind = outbound_err.outcome_kind.as_str(),
                            retry_after_secs = outbound_err.retry_after_secs,
                            delivery_state = outbound_err.delivery_state.map(OutboundDeliveryState::as_str),
                            error = %outbound_err,
                            "telegram outbound send_message gave up"
                        );
                        return Err(anyhow::Error::new(outbound_err));
                    },
                    RetryDecision::SuccessEquivalent => {
                        unreachable!("send_message has no success-equivalent path")
                    },
                },
            }
        }
    }

    async fn edit_message_with_retry<T: TelegramTextTransport>(
        &self,
        transport: &T,
        retry_cfg: OutboundRetryConfig,
        account_handle: &str,
        chat_id: ChatId,
        chat_id_text: &str,
        op: TelegramOutboundOp,
        message_id: MessageId,
        text: &str,
        delivery_state: Option<OutboundDeliveryState>,
        text_len: usize,
        chunk_count: Option<usize>,
    ) -> Result<()> {
        let mut attempt = 1;
        loop {
            match transport
                .edit_message_text_html(chat_id, message_id, text)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => match classify_retry_decision(op, &err, attempt, retry_cfg) {
                    RetryDecision::Retry(delay) => {
                        warn!(
                            event = "telegram.outbound.retrying",
                            op = op.as_str(),
                            account_handle,
                            chat_id = chat_id_text,
                            message_id = message_id.0,
                            text_len,
                            chunk_count,
                            attempt,
                            max_attempts = retry_cfg.max_attempts,
                            error_class = classify_request_error(&err).as_str(),
                            outcome_kind = classify_outcome_kind(op, &err).as_str(),
                            retry_after_secs = retry_after_secs(&err),
                            retry_delay_ms = delay.as_millis() as u64,
                            delivery_state = delivery_state.map(OutboundDeliveryState::as_str),
                            error = %err,
                            "telegram outbound retrying edit_message_text"
                        );
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        attempt += 1;
                    },
                    RetryDecision::SuccessEquivalent => {
                        info!(
                            event = "telegram.outbound.success_equivalent",
                            op = op.as_str(),
                            account_handle,
                            chat_id = chat_id_text,
                            message_id = message_id.0,
                            attempt,
                            max_attempts = retry_cfg.max_attempts,
                            error_class = classify_request_error(&err).as_str(),
                            outcome_kind = classify_outcome_kind(op, &err).as_str(),
                            delivery_state = delivery_state.map(OutboundDeliveryState::as_str),
                            "telegram outbound edit_message_text converged to success-equivalent"
                        );
                        return Ok(());
                    },
                    RetryDecision::GiveUp => {
                        let outbound_err = TelegramOutboundError::from_request_error(
                            op,
                            delivery_state,
                            attempt,
                            retry_cfg.max_attempts,
                            err,
                        );
                        warn!(
                            event = "telegram.outbound.gave_up",
                            op = op.as_str(),
                            account_handle,
                            chat_id = chat_id_text,
                            message_id = message_id.0,
                            text_len,
                            chunk_count,
                            attempt,
                            max_attempts = retry_cfg.max_attempts,
                            error_class = outbound_err.error_class.as_str(),
                            outcome_kind = outbound_err.outcome_kind.as_str(),
                            retry_after_secs = outbound_err.retry_after_secs,
                            delivery_state = outbound_err.delivery_state.map(OutboundDeliveryState::as_str),
                            error = %outbound_err,
                            "telegram outbound edit_message_text gave up"
                        );
                        return Err(anyhow::Error::new(outbound_err));
                    },
                },
            }
        }
    }

    async fn send_text_inner(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
        thread_id: Option<&str>,
        silent: bool,
        lineage_message_id: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let retry_cfg = self.get_retry_config(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, to, "send_text", reply_to);
        let thread_id_text = thread_id.map(str::to_string);
        let thread_id = self.thread_id(account_handle, to, "send_text", thread_id);
        let transport = BotTextTransport { bot };

        // Send typing indicator
        if !silent {
            if let Err(e) = transport.send_typing(chat_id, thread_id).await {
                let reason_code = match &e {
                    TypingRequestError::Request(teloxide::RequestError::Network(err))
                        if err.is_timeout() || err.is_connect() =>
                    {
                        "transport_failed_before_send"
                    },
                    TypingRequestError::Request(teloxide::RequestError::Api(_)) => "api",
                    TypingRequestError::Timeout => "send_typing_timeout",
                    TypingRequestError::Request(_) => "other",
                };
                warn!(
                    event = "telegram.typing.failed",
                    op = "send_text",
                    account_handle,
                    chat_id = to,
                    reason_code,
                    error = %e,
                    "failed to send typing indicator"
                );
            }
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
        let mut sent_message_ids = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let sent = self
                .send_message_with_retry(
                    &transport,
                    retry_cfg,
                    account_handle,
                    chat_id,
                    to,
                    TelegramOutboundOp::SendMessage,
                    chunk,
                    SendMessageOptions {
                        disable_notification: silent,
                        reply_parameters: (i == 0).then(|| rp.clone()).flatten(),
                        thread_id,
                    },
                    Some(if i == 0 {
                        OutboundDeliveryState::FirstChunkUnsent
                    } else {
                        OutboundDeliveryState::PartialSent
                    }),
                    reply_to,
                    Some(i),
                    Some(chunks.len()),
                    None,
                    text.len(),
                    None,
                )
                .await?;
            sent_message_ids.push(sent.0.to_string());
            if first_id.is_none() {
                first_id = Some(sent);
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
        if let Some(sent_id) = first_id {
            self.register_group_visible_outbound_contexts(
                account_handle,
                to,
                lineage_message_id,
                &sent_message_ids,
            );
            self.emit_group_visible_outbound_event(
                account_handle,
                to,
                reply_to,
                lineage_message_id,
                thread_id_text.as_deref(),
                text,
                Some(moltis_channels::ChannelMessageKind::Text),
                &sent_id.0.to_string(),
            )
            .await;
        }
        Ok(first_id)
    }

    async fn send_text_with_suffix_inner(
        &self,
        account_handle: &str,
        to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
        thread_id: Option<&str>,
        lineage_message_id: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let retry_cfg = self.get_retry_config(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, to, "send_text_with_suffix", reply_to);
        let thread_id_text = thread_id.map(str::to_string);
        let thread_id = self.thread_id(account_handle, to, "send_text_with_suffix", thread_id);
        let transport = BotTextTransport { bot };

        // Send typing indicator
        if let Err(e) = transport.send_typing(chat_id, thread_id).await {
            let reason_code = match &e {
                TypingRequestError::Request(teloxide::RequestError::Network(err))
                    if err.is_timeout() || err.is_connect() =>
                {
                    "transport_failed_before_send"
                },
                TypingRequestError::Request(teloxide::RequestError::Api(_)) => "api",
                TypingRequestError::Timeout => "send_typing_timeout",
                TypingRequestError::Request(_) => "other",
            };
            warn!(
                event = "telegram.typing.failed",
                op = "send_text_with_suffix",
                account_handle,
                chat_id = to,
                reason_code,
                error = %e,
                "failed to send typing indicator"
            );
        }

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
        let mut sent_message_ids = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let content = if i == last_idx {
                // Append suffix to the last chunk. If it would exceed the limit,
                // the suffix becomes a separate final message.
                let combined = format!("{chunk}\n\n{suffix_html}");
                if combined.len() <= TELEGRAM_MAX_MESSAGE_LEN {
                    combined
                } else {
                    // Send this chunk first, then the suffix as a separate message.
                    let sent = self
                        .send_message_with_retry(
                            &transport,
                            retry_cfg,
                            account_handle,
                            chat_id,
                            to,
                            TelegramOutboundOp::SendMessage,
                            chunk,
                            SendMessageOptions {
                                disable_notification: false,
                                reply_parameters: (i == 0).then(|| rp.clone()).flatten(),
                                thread_id,
                            },
                            Some(if i == 0 {
                                OutboundDeliveryState::FirstChunkUnsent
                            } else {
                                OutboundDeliveryState::PartialSent
                            }),
                            reply_to,
                            Some(i),
                            Some(chunks.len()),
                            None,
                            text.len(),
                            Some(suffix_html.len()),
                        )
                        .await?;
                    sent_message_ids.push(sent.0.to_string());
                    if first_id.is_none() {
                        first_id = Some(sent);
                    }

                    // Send suffix as the final message (no reply threading).
                    let suffix_sent = self
                        .send_message_with_retry(
                            &transport,
                            retry_cfg,
                            account_handle,
                            chat_id,
                            to,
                            TelegramOutboundOp::SendMessageSuffix,
                            suffix_html,
                            SendMessageOptions {
                                disable_notification: true,
                                reply_parameters: None,
                                thread_id: None,
                            },
                            Some(OutboundDeliveryState::PartialSent),
                            reply_to,
                            Some(chunks.len()),
                            Some(chunks.len() + 1),
                            None,
                            text.len(),
                            Some(suffix_html.len()),
                        )
                        .await?;
                    sent_message_ids.push(suffix_sent.0.to_string());

                    info!(
                        account_handle,
                        chat_id = to,
                        reply_to = ?reply_to,
                        text_len = text.len(),
                        suffix_len = suffix_html.len(),
                        chunk_count = chunks.len(),
                        "telegram outbound text+suffix sent (separate suffix message)"
                    );
                    if let Some(sent_id) = first_id {
                        self.register_group_visible_outbound_contexts(
                            account_handle,
                            to,
                            lineage_message_id,
                            &sent_message_ids,
                        );
                        self.emit_group_visible_outbound_event(
                            account_handle,
                            to,
                            reply_to,
                            lineage_message_id,
                            thread_id_text.as_deref(),
                            text,
                            Some(moltis_channels::ChannelMessageKind::Text),
                            &sent_id.0.to_string(),
                        )
                        .await;
                    }
                    return Ok(first_id);
                }
            } else {
                chunk.clone()
            };
            let sent = self
                .send_message_with_retry(
                    &transport,
                    retry_cfg,
                    account_handle,
                    chat_id,
                    to,
                    TelegramOutboundOp::SendMessage,
                    &content,
                    SendMessageOptions {
                        disable_notification: false,
                        reply_parameters: (i == 0).then(|| rp.clone()).flatten(),
                        thread_id,
                    },
                    Some(if i == 0 {
                        OutboundDeliveryState::FirstChunkUnsent
                    } else {
                        OutboundDeliveryState::PartialSent
                    }),
                    reply_to,
                    Some(i),
                    Some(chunks.len()),
                    None,
                    text.len(),
                    Some(suffix_html.len()),
                )
                .await?;
            sent_message_ids.push(sent.0.to_string());
            if first_id.is_none() {
                first_id = Some(sent);
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
        if let Some(sent_id) = first_id {
            self.register_group_visible_outbound_contexts(
                account_handle,
                to,
                lineage_message_id,
                &sent_message_ids,
            );
            self.emit_group_visible_outbound_event(
                account_handle,
                to,
                reply_to,
                lineage_message_id,
                thread_id_text.as_deref(),
                text,
                Some(moltis_channels::ChannelMessageKind::Text),
                &sent_id.0.to_string(),
            )
            .await;
        }
        Ok(first_id)
    }

    async fn send_media_inner(
        &self,
        account_handle: &str,
        to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
        thread_id: Option<&str>,
        lineage_message_id: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, to, "send_media", reply_to);
        let thread_id_text = thread_id.map(str::to_string);
        let parsed_thread_id = self.thread_id(account_handle, to, "send_media", thread_id);
        let (caption, caption_overflow) = split_caption_for_telegram(&payload.text);
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
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                            let sent_id = sent.id;
                            if !caption_overflow.is_empty() {
                                if let Err(e) = self
                                    .send_caption_overflow(
                                        account_handle,
                                        to,
                                        caption_overflow,
                                        sent_id,
                                    )
                                    .await
                                {
                                    warn!(
                                        event = "telegram.outbound.degraded",
                                        op = "send_media",
                                        account_handle,
                                        chat_id = to,
                                        reason_code = "caption_overflow_send_failed",
                                        error = %e,
                                        "media sent but caption overflow follow-up failed"
                                    );
                                }
                            }
                            self.emit_group_visible_outbound_event(
                                account_handle,
                                to,
                                reply_to,
                                lineage_message_id,
                                thread_id_text.as_deref(),
                                &payload.text,
                                Some(moltis_channels::ChannelMessageKind::Photo),
                                &sent_id.0.to_string(),
                            )
                            .await;
                            return Ok(Some(sent_id));
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
                                if let Some(thread_id) = parsed_thread_id {
                                    req = req.message_thread_id(thread_id);
                                }
                                if !caption.is_empty() {
                                    req = req.caption(caption);
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
                                let sent_id = sent.id;
                                if !caption_overflow.is_empty() {
                                    if let Err(e) = self
                                        .send_caption_overflow(
                                            account_handle,
                                            to,
                                            caption_overflow,
                                            sent_id,
                                        )
                                        .await
                                    {
                                        warn!(
                                            event = "telegram.outbound.degraded",
                                            op = "send_media",
                                            account_handle,
                                            chat_id = to,
                                            reason_code = "caption_overflow_send_failed",
                                            error = %e,
                                            "media sent but caption overflow follow-up failed"
                                        );
                                    }
                                }
                                self.emit_group_visible_outbound_event(
                                    account_handle,
                                    to,
                                    reply_to,
                                    lineage_message_id,
                                    thread_id_text.as_deref(),
                                    &payload.text,
                                    Some(moltis_channels::ChannelMessageKind::Document),
                                    &sent_id.0.to_string(),
                                )
                                .await;
                                return Ok(Some(sent_id));
                            }
                            return Err(e.into());
                        },
                    }
                }

                // Non-image types: send as document/voice/audio depending on mime type.
                if media.mime_type == "audio/ogg" {
                    let input = InputFile::memory(bytes).file_name("voice.ogg");
                    let mut req = bot.send_voice(chat_id, input);
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                    let sent_id = sent.id;
                    if !caption_overflow.is_empty() {
                        if let Err(e) = self
                            .send_caption_overflow(account_handle, to, caption_overflow, sent_id)
                            .await
                        {
                            warn!(
                                event = "telegram.outbound.degraded",
                                op = "send_media",
                                account_handle,
                                chat_id = to,
                                reason_code = "caption_overflow_send_failed",
                                error = %e,
                                "media sent but caption overflow follow-up failed"
                            );
                        }
                    }
                    self.emit_group_visible_outbound_event(
                        account_handle,
                        to,
                        reply_to,
                        lineage_message_id,
                        thread_id_text.as_deref(),
                        &payload.text,
                        Some(moltis_channels::ChannelMessageKind::Voice),
                        &sent_id.0.to_string(),
                    )
                    .await;
                    return Ok(Some(sent_id));
                }
                if media.mime_type.starts_with("audio/") {
                    let input = InputFile::memory(bytes).file_name("audio.mp3");
                    let mut req = bot.send_audio(chat_id, input);
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                    let sent_id = sent.id;
                    if !caption_overflow.is_empty() {
                        if let Err(e) = self
                            .send_caption_overflow(account_handle, to, caption_overflow, sent_id)
                            .await
                        {
                            warn!(
                                event = "telegram.outbound.degraded",
                                op = "send_media",
                                account_handle,
                                chat_id = to,
                                reason_code = "caption_overflow_send_failed",
                                error = %e,
                                "media sent but caption overflow follow-up failed"
                            );
                        }
                    }
                    self.emit_group_visible_outbound_event(
                        account_handle,
                        to,
                        reply_to,
                        lineage_message_id,
                        thread_id_text.as_deref(),
                        &payload.text,
                        Some(moltis_channels::ChannelMessageKind::Audio),
                        &sent_id.0.to_string(),
                    )
                    .await;
                    return Ok(Some(sent_id));
                }

                let input = InputFile::memory(bytes).file_name(filename);
                let mut req = bot.send_document(chat_id, input);
                if let Some(thread_id) = parsed_thread_id {
                    req = req.message_thread_id(thread_id);
                }
                if !caption.is_empty() {
                    req = req.caption(caption);
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
                let sent_id = sent.id;
                if !caption_overflow.is_empty() {
                    if let Err(e) = self
                        .send_caption_overflow(account_handle, to, caption_overflow, sent_id)
                        .await
                    {
                        warn!(
                            event = "telegram.outbound.degraded",
                            op = "send_media",
                            account_handle,
                            chat_id = to,
                            reason_code = "caption_overflow_send_failed",
                            error = %e,
                            "media sent but caption overflow follow-up failed"
                        );
                    }
                }
                self.emit_group_visible_outbound_event(
                    account_handle,
                    to,
                    reply_to,
                    lineage_message_id,
                    thread_id_text.as_deref(),
                    &payload.text,
                    Some(moltis_channels::ChannelMessageKind::Document),
                    &sent_id.0.to_string(),
                )
                .await;
                return Ok(Some(sent_id));
            }

            // URL-based media
            let input = InputFile::url(media.url.parse()?);
            let sent = match media.mime_type.as_str() {
                t if t.starts_with("image/") => {
                    let mut req = bot.send_photo(chat_id, input);
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
                    if let Some(thread_id) = parsed_thread_id {
                        req = req.message_thread_id(thread_id);
                    }
                    if !caption.is_empty() {
                        req = req.caption(caption);
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
            if !caption_overflow.is_empty() {
                if let Err(e) = self
                    .send_caption_overflow(account_handle, to, caption_overflow, sent)
                    .await
                {
                    warn!(
                        event = "telegram.outbound.degraded",
                        op = "send_media",
                        account_handle,
                        chat_id = to,
                        reason_code = "caption_overflow_send_failed",
                        error = %e,
                        "media sent but caption overflow follow-up failed"
                    );
                }
            }
            self.emit_group_visible_outbound_event(
                account_handle,
                to,
                reply_to,
                lineage_message_id,
                thread_id_text.as_deref(),
                &payload.text,
                payload.media.as_ref().map(|media| {
                    if media.mime_type.starts_with("image/") {
                        moltis_channels::ChannelMessageKind::Photo
                    } else if media.mime_type == "audio/ogg" {
                        moltis_channels::ChannelMessageKind::Voice
                    } else if media.mime_type.starts_with("audio/") {
                        moltis_channels::ChannelMessageKind::Audio
                    } else {
                        moltis_channels::ChannelMessageKind::Document
                    }
                }),
                &sent.0.to_string(),
            )
            .await;
            return Ok(Some(sent));
        }

        if !payload.text.is_empty() {
            let sent = self
                .send_text_inner(
                    account_handle,
                    to,
                    &payload.text,
                    reply_to,
                    thread_id,
                    false,
                    lineage_message_id,
                )
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
        thread_id: Option<&str>,
        lineage_message_id: Option<&str>,
    ) -> Result<Option<MessageId>> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        let rp = self.reply_params(account_handle, to, "send_location", reply_to);
        let thread_id_text = thread_id.map(str::to_string);
        let thread_id = self.thread_id(account_handle, to, "send_location", thread_id);
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
            if let Some(thread_id) = thread_id {
                req = req.message_thread_id(thread_id);
            }
            if let Some(ref rp) = rp {
                req = req.reply_parameters(rp.clone());
            }
            req.await?
        } else {
            let mut req = bot.send_location(chat_id, latitude, longitude);
            if let Some(thread_id) = thread_id {
                req = req.message_thread_id(thread_id);
            }
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
        self.emit_group_visible_outbound_event(
            account_handle,
            to,
            reply_to,
            lineage_message_id,
            thread_id_text.as_deref(),
            title.unwrap_or_default(),
            Some(moltis_channels::ChannelMessageKind::Location),
            &sent.id.0.to_string(),
        )
        .await;
        Ok(Some(sent.id))
    }
}

/// Parse a platform message ID string into Telegram `ReplyParameters`.
/// Returns `(params, invalid)` where `invalid=true` means caller provided a `reply_to`
/// but it was not a valid i32 (Telegram message IDs are i32).
fn parse_reply_params_checked(reply_to: Option<&str>) -> (Option<ReplyParameters>, bool) {
    let Some(id) = reply_to else {
        return (None, false);
    };
    match id.parse::<i32>() {
        Ok(id) => (
            Some(ReplyParameters::new(MessageId(id)).allow_sending_without_reply()),
            false,
        ),
        Err(_) => (None, true),
    }
}

fn split_caption_for_telegram(caption: &str) -> (&str, &str) {
    if caption.is_empty() {
        return ("", "");
    }
    let head = crate::markdown::truncate_utf8(caption, crate::markdown::TELEGRAM_CAPTION_LIMIT);
    let tail = &caption[head.len()..];
    (head, tail)
}

impl TelegramOutbound {
    async fn send_caption_overflow(
        &self,
        account_handle: &str,
        to: &str,
        overflow: &str,
        reply_to_message_id: MessageId,
    ) -> Result<()> {
        if overflow.trim().is_empty() {
            return Ok(());
        }
        let reply_to = reply_to_message_id.0.to_string();
        let _ = self
            .send_text_inner(
                account_handle,
                to,
                overflow,
                Some(&reply_to),
                None,
                false,
                Some(&reply_to),
            )
            .await?;
        Ok(())
    }
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
            .send_text_inner(account_handle, to, text, reply_to, None, false, reply_to)
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
            .send_text_inner(account_handle, to, text, reply_to, None, false, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
    ) -> Result<()> {
        let _ = self
            .send_text_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                false,
                target.message_id.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn send_text_to_target_with_ref(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                false,
                target.message_id.as_deref(),
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_by_reply_target_ref_with_ref(
        &self,
        reply_target_ref: &str,
        text: &str,
    ) -> Result<Option<SentMessageRef>> {
        let parsed =
            crate::adapter::telegram_outbound_target_from_reply_target_ref(reply_target_ref)
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid reply_target_ref for telegram send_text")
                })?;
        let sent = self
            .send_text_inner(
                &parsed.target.chan_account_key,
                &parsed.target.chat_id,
                text,
                parsed.target.message_id.as_deref(),
                parsed.target.thread_id.as_deref(),
                false,
                parsed.lineage_message_id.as_deref(),
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_with_suffix_by_reply_target_ref_with_ref(
        &self,
        reply_target_ref: &str,
        text: &str,
        suffix_html: &str,
    ) -> Result<Option<SentMessageRef>> {
        let parsed =
            crate::adapter::telegram_outbound_target_from_reply_target_ref(reply_target_ref)
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid reply_target_ref for telegram send_text_with_suffix")
                })?;
        let sent = self
            .send_text_with_suffix_inner(
                &parsed.target.chan_account_key,
                &parsed.target.chat_id,
                text,
                suffix_html,
                parsed.target.message_id.as_deref(),
                parsed.target.thread_id.as_deref(),
                parsed.lineage_message_id.as_deref(),
            )
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
            .send_text_with_suffix_inner(
                account_handle,
                to,
                text,
                suffix_html,
                reply_to,
                None,
                reply_to,
            )
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
            .send_text_with_suffix_inner(
                account_handle,
                to,
                text,
                suffix_html,
                reply_to,
                None,
                reply_to,
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_with_suffix_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
        suffix_html: &str,
    ) -> Result<()> {
        let _ = self
            .send_text_with_suffix_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                suffix_html,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn send_text_with_suffix_to_target_with_ref(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
        suffix_html: &str,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_with_suffix_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                suffix_html,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_typing(&self, account_handle: &str, to: &str) -> Result<()> {
        let bot = self.get_bot(account_handle)?;
        let chat_id = ChatId(to.parse::<i64>()?);
        send_chat_action_typing(&bot, chat_id, None).await?;
        Ok(())
    }

    async fn send_typing_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
    ) -> Result<()> {
        let bot = self.get_bot(&target.chan_account_key)?;
        let chat_id = ChatId(target.chat_id.parse::<i64>()?);
        let thread_id = self.thread_id(
            &target.chan_account_key,
            &target.chat_id,
            "send_typing",
            target.thread_id.as_deref(),
        );
        send_chat_action_typing(&bot, chat_id, thread_id).await?;
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
            .send_text_inner(account_handle, to, text, reply_to, None, true, reply_to)
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
            .send_text_inner(account_handle, to, text, reply_to, None, true, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_text_silent_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
    ) -> Result<()> {
        let _ = self
            .send_text_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                true,
                target.message_id.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn send_text_silent_to_target_with_ref(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        text: &str,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_text_inner(
                &target.chan_account_key,
                &target.chat_id,
                text,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                true,
                target.message_id.as_deref(),
            )
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
            .send_media_inner(account_handle, to, payload, reply_to, None, reply_to)
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
            .send_media_inner(account_handle, to, payload, reply_to, None, reply_to)
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_media_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        payload: &ReplyPayload,
    ) -> Result<()> {
        let _ = self
            .send_media_inner(
                &target.chan_account_key,
                &target.chat_id,
                payload,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn send_media_to_target_with_ref(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        payload: &ReplyPayload,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_media_inner(
                &target.chan_account_key,
                &target.chat_id,
                payload,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_media_by_reply_target_ref_with_ref(
        &self,
        reply_target_ref: &str,
        payload: &ReplyPayload,
    ) -> Result<Option<SentMessageRef>> {
        let parsed =
            crate::adapter::telegram_outbound_target_from_reply_target_ref(reply_target_ref)
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid reply_target_ref for telegram send_media")
                })?;
        let sent = self
            .send_media_inner(
                &parsed.target.chan_account_key,
                &parsed.target.chat_id,
                payload,
                parsed.target.message_id.as_deref(),
                parsed.target.thread_id.as_deref(),
                parsed.lineage_message_id.as_deref(),
            )
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
            .send_location_inner(
                account_handle,
                to,
                latitude,
                longitude,
                title,
                reply_to,
                None,
                reply_to,
            )
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
            .send_location_inner(
                account_handle,
                to,
                latitude,
                longitude,
                title,
                reply_to,
                None,
                reply_to,
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_location_by_reply_target_ref_with_ref(
        &self,
        reply_target_ref: &str,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let parsed =
            crate::adapter::telegram_outbound_target_from_reply_target_ref(reply_target_ref)
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid reply_target_ref for telegram send_location")
                })?;
        let sent = self
            .send_location_inner(
                &parsed.target.chan_account_key,
                &parsed.target.chat_id,
                latitude,
                longitude,
                title,
                parsed.target.message_id.as_deref(),
                parsed.target.thread_id.as_deref(),
                parsed.lineage_message_id.as_deref(),
            )
            .await?;
        Ok(sent.map(|id| SentMessageRef {
            message_id: id.0.to_string(),
        }))
    }

    async fn send_location_to_target(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .send_location_inner(
                &target.chan_account_key,
                &target.chat_id,
                latitude,
                longitude,
                title,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
            .await?;
        Ok(())
    }

    async fn send_location_to_target_with_ref(
        &self,
        target: &moltis_channels::ChannelReplyTarget,
        latitude: f64,
        longitude: f64,
        title: Option<&str>,
    ) -> Result<Option<SentMessageRef>> {
        let sent = self
            .send_location_inner(
                &target.chan_account_key,
                &target.chat_id,
                latitude,
                longitude,
                title,
                target.message_id.as_deref(),
                target.thread_id.as_deref(),
                target.message_id.as_deref(),
            )
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

        send_chat_action_typing(bot, chat_id, None).await?;

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
        let retry_cfg = self.get_retry_config(account_handle)?;
        let throttle_ms = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            accounts
                .get(account_handle)
                .map(|s| s.config.edit_throttle_ms)
                .unwrap_or(300)
        };

        let transport = BotTextTransport { bot };
        self.send_stream_with_transport(
            &transport,
            retry_cfg,
            account_handle,
            to,
            throttle_ms,
            &mut stream,
        )
        .await
    }
}

impl TelegramOutbound {
    async fn send_stream_with_transport<T: TelegramTextTransport>(
        &self,
        transport: &T,
        retry_cfg: OutboundRetryConfig,
        account_handle: &str,
        to: &str,
        throttle_ms: u64,
        stream: &mut StreamReceiver,
    ) -> Result<()> {
        let chat_id = ChatId(to.parse::<i64>()?);
        let operation = async {
            let msg_id = self
                .send_message_with_retry(
                    transport,
                    retry_cfg,
                    account_handle,
                    chat_id,
                    to,
                    TelegramOutboundOp::SendMessagePlaceholder,
                    "…",
                    SendMessageOptions {
                        disable_notification: false,
                        reply_parameters: None,
                        thread_id: None,
                    },
                    Some(OutboundDeliveryState::PlaceholderUnsent),
                    None,
                    None,
                    None,
                    None,
                    0,
                    None,
                )
                .await?;

            let mut accumulated = String::new();
            let mut last_edit = tokio::time::Instant::now();
            let throttle = std::time::Duration::from_millis(throttle_ms);
            let mut edit_degraded = false;

            while let Some(event) = stream.recv().await {
                match event {
                    StreamEvent::Delta(delta) => {
                        accumulated.push_str(&delta);
                        if !edit_degraded && last_edit.elapsed() >= throttle {
                            let html = markdown::markdown_to_telegram_html(&accumulated);
                            let display = markdown::truncate_utf8(&html, TELEGRAM_MAX_MESSAGE_LEN);
                            if let Err(err) = self
                                .edit_message_with_retry(
                                    transport,
                                    retry_cfg,
                                    account_handle,
                                    chat_id,
                                    to,
                                    TelegramOutboundOp::EditMessageText,
                                    msg_id,
                                    &display,
                                    Some(OutboundDeliveryState::PlaceholderSent),
                                    accumulated.len(),
                                    None,
                                )
                                .await
                            {
                                edit_degraded = true;
                                warn!(
                                    event = "telegram.outbound.degraded",
                                    op = TelegramOutboundOp::EditMessageText.as_str(),
                                    account_handle,
                                    chat_id = to,
                                    message_id = msg_id.0,
                                    outcome_kind = err
                                        .downcast_ref::<TelegramOutboundError>()
                                        .map(|e| e.outcome_kind.as_str())
                                        .unwrap_or("unknown"),
                                    delivery_state = err
                                        .downcast_ref::<TelegramOutboundError>()
                                        .and_then(|e| e.delivery_state)
                                        .map(OutboundDeliveryState::as_str)
                                        .unwrap_or("none"),
                                    error = %err,
                                    "telegram outbound streaming edit degraded after retries; suppressing further incremental edits"
                                );
                            }
                            last_edit = tokio::time::Instant::now();
                        }
                    },
                    StreamEvent::Done => {
                        break;
                    },
                    StreamEvent::Error(_e) => {
                        warn!(
                            event = "telegram.stream.error",
                            op = "send_stream",
                            account_handle,
                            chat_id = to,
                            reason_code = "stream_error",
                            "streaming response failed"
                        );
                        accumulated
                            .push_str("\n\n⚠️ Something went wrong while generating the response.");
                        break;
                    },
                }
            }

            if !accumulated.is_empty() {
                let html = markdown::markdown_to_telegram_html(&accumulated);
                let chunks = markdown::chunk_message(&html, TELEGRAM_MAX_MESSAGE_LEN);

                self.edit_message_with_retry(
                    transport,
                    retry_cfg,
                    account_handle,
                    chat_id,
                    to,
                    TelegramOutboundOp::EditMessageTextFinal,
                    msg_id,
                    &chunks[0],
                    Some(OutboundDeliveryState::PlaceholderSent),
                    accumulated.len(),
                    Some(chunks.len()),
                )
                .await?;

                for (idx, chunk) in chunks[1..].iter().enumerate() {
                    let _ = self
                        .send_message_with_retry(
                            transport,
                            retry_cfg,
                            account_handle,
                            chat_id,
                            to,
                            TelegramOutboundOp::SendMessageStreamChunk,
                            chunk,
                            SendMessageOptions {
                                disable_notification: false,
                                reply_parameters: None,
                                thread_id: None,
                            },
                            Some(OutboundDeliveryState::PartialSent),
                            None,
                            Some(idx + 1),
                            Some(chunks.len()),
                            Some(msg_id),
                            accumulated.len(),
                            None,
                        )
                        .await?;
                }
            }

            Ok(())
        };

        let typing_loop = async {
            let mut typing_failed = false;
            send_typing_keepalive(
                transport,
                account_handle,
                to,
                chat_id,
                None,
                "send_stream",
                &mut typing_failed,
            )
            .await;

            let mut keepalive = tokio::time::interval_at(
                tokio::time::Instant::now() + TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
                TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
            );
            keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                keepalive.tick().await;
                send_typing_keepalive(
                    transport,
                    account_handle,
                    to,
                    chat_id,
                    None,
                    "send_stream",
                    &mut typing_failed,
                )
                .await;
            }
        };

        tokio::pin!(operation);
        tokio::pin!(typing_loop);

        tokio::select! {
            result = &mut operation => result,
            _ = &mut typing_loop => unreachable!("telegram stream typing loop must stay pending until the operation completes"),
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        async_trait::async_trait,
        axum::{Json, Router, body::Bytes, extract::State, routing::post},
        moltis_channels::ChannelReplyTarget,
        std::{
            collections::{HashMap, VecDeque},
            sync::{Arc, Mutex},
        },
        teloxide::requests::Requester,
        teloxide::types::Seconds,
        tokio::sync::mpsc,
        tracing_subscriber::fmt::MakeWriter,
    };

    #[derive(Default)]
    struct RecordingBridge {
        requests: Arc<Mutex<Vec<crate::adapter::TgInboundRequest>>>,
    }

    #[async_trait]
    impl crate::adapter::TelegramCoreBridge for RecordingBridge {
        async fn handle_inbound(&self, request: crate::adapter::TgInboundRequest) {
            self.requests.lock().unwrap().push(request);
        }

        async fn dispatch_command(
            &self,
            _command: &str,
            _target: crate::adapter::TgFollowUpTarget,
        ) -> anyhow::Result<String> {
            unreachable!("dispatch_command not used in outbound tests")
        }

        async fn request_voice_transcription(
            &self,
            _audio_data: &[u8],
            _format: &str,
        ) -> anyhow::Result<String> {
            unreachable!("request_voice_transcription not used in outbound tests")
        }

        async fn voice_transcription_available(&self) -> bool {
            false
        }

        async fn update_location(
            &self,
            _target: crate::adapter::TgFollowUpTarget,
            _latitude: f64,
            _longitude: f64,
        ) -> bool {
            false
        }
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

    #[derive(Debug)]
    enum MockSendResult {
        Ok(MessageId),
        Err(teloxide::RequestError),
    }

    #[derive(Debug)]
    enum MockEditResult {
        Ok,
        Err(teloxide::RequestError),
    }

    #[derive(Default)]
    struct MockTextTransport {
        send_results: Mutex<VecDeque<MockSendResult>>,
        edit_results: Mutex<VecDeque<MockEditResult>>,
        send_calls: Mutex<usize>,
        edit_calls: Mutex<usize>,
        typing_calls: Mutex<usize>,
        last_send_text: Mutex<Option<String>>,
        last_edit_text: Mutex<Option<String>>,
        last_typing_thread_id: Mutex<Option<i32>>,
        last_send_thread_id: Mutex<Option<i32>>,
    }

    impl MockTextTransport {
        fn with_send_results(results: Vec<MockSendResult>) -> Self {
            Self {
                send_results: Mutex::new(results.into()),
                ..Default::default()
            }
        }

        fn with_edit_results(results: Vec<MockEditResult>) -> Self {
            Self {
                edit_results: Mutex::new(results.into()),
                ..Default::default()
            }
        }

        fn send_calls(&self) -> usize {
            *self.send_calls.lock().unwrap()
        }

        fn edit_calls(&self) -> usize {
            *self.edit_calls.lock().unwrap()
        }

        fn typing_calls(&self) -> usize {
            *self.typing_calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl TelegramTextTransport for MockTextTransport {
        async fn send_typing(
            &self,
            _chat_id: ChatId,
            thread_id: Option<ThreadId>,
        ) -> std::result::Result<(), TypingRequestError> {
            *self.typing_calls.lock().unwrap() += 1;
            *self.last_typing_thread_id.lock().unwrap() = thread_id.map(|id| id.0.0);
            Ok(())
        }

        async fn send_message_html(
            &self,
            _chat_id: ChatId,
            _text: &str,
            _options: SendMessageOptions,
        ) -> std::result::Result<MessageId, teloxide::RequestError> {
            *self.send_calls.lock().unwrap() += 1;
            *self.last_send_text.lock().unwrap() = Some(_text.to_string());
            *self.last_send_thread_id.lock().unwrap() = _options.thread_id.map(|id| id.0.0);
            match self.send_results.lock().unwrap().pop_front() {
                Some(MockSendResult::Ok(id)) => Ok(id),
                Some(MockSendResult::Err(err)) => Err(err),
                None => panic!("missing scripted send result"),
            }
        }

        async fn edit_message_text_html(
            &self,
            _chat_id: ChatId,
            _message_id: MessageId,
            _text: &str,
        ) -> std::result::Result<(), teloxide::RequestError> {
            *self.edit_calls.lock().unwrap() += 1;
            *self.last_edit_text.lock().unwrap() = Some(_text.to_string());
            match self.edit_results.lock().unwrap().pop_front() {
                Some(MockEditResult::Ok) => Ok(()),
                Some(MockEditResult::Err(err)) => Err(err),
                None => panic!("missing scripted edit result"),
            }
        }
    }

    fn empty_outbound() -> TelegramOutbound {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        TelegramOutbound { accounts }
    }

    fn outbound_with_bot(account_handle: &str, bot: teloxide::Bot) -> TelegramOutbound {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        {
            let mut map = accounts.write().unwrap();
            map.insert(
                account_handle.to_string(),
                crate::state::AccountState {
                    bot,
                    bot_user_id: None,
                    bot_username: None,
                    account_handle: account_handle.to_string(),
                    config: crate::config::TelegramAccountConfig::default(),
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }
        outbound
    }

    fn outbound_with_bot_and_accounts(
        account_handle: &str,
        bot: teloxide::Bot,
    ) -> (TelegramOutbound, AccountStateMap) {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        {
            let mut map = accounts.write().unwrap();
            map.insert(
                account_handle.to_string(),
                crate::state::AccountState {
                    bot,
                    bot_user_id: None,
                    bot_username: None,
                    account_handle: account_handle.to_string(),
                    config: crate::config::TelegramAccountConfig::default(),
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }
        (outbound, accounts)
    }

    fn retry_cfg(max_attempts: u32) -> OutboundRetryConfig {
        OutboundRetryConfig {
            max_attempts,
            base_delay_ms: 0,
            max_delay_ms: 0,
        }
    }

    #[test]
    fn parse_reply_params_checked_reports_invalid_reply_to() {
        let (ok, invalid_ok) = super::parse_reply_params_checked(Some("42"));
        assert!(ok.is_some());
        assert!(!invalid_ok);

        let (none, invalid) = super::parse_reply_params_checked(Some("not-a-number"));
        assert!(none.is_none());
        assert!(invalid);
    }

    #[tokio::test]
    async fn group_visible_outbound_uses_identity_display_name_for_bot_speaker() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        let bridge = Arc::new(RecordingBridge::default());

        crate::state::replace_telegram_identity_links(
            &accounts,
            vec![crate::config::TelegramIdentityLink {
                agent_id: "risk".into(),
                display_name: Some("风险助手".into()),
                telegram_user_id: Some(100),
                telegram_user_name: Some("risk_bot_cn".into()),
                telegram_display_name: Some("风险助手中文".into()),
            }],
        );

        {
            let mut map = accounts.write().unwrap();
            map.insert(
                "telegram:100".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-a"),
                    bot_user_id: Some(UserId(100)),
                    bot_username: Some("risk_bot_cn".into()),
                    account_handle: "telegram:100".into(),
                    config: crate::config::TelegramAccountConfig {
                        agent_id: Some("risk".into()),
                        chan_user_id: Some(100),
                        chan_user_name: Some("risk_bot_cn".into()),
                        chan_nickname: Some("风险助手中文".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
            map.insert(
                "telegram:200".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-b"),
                    bot_user_id: Some(UserId(200)),
                    bot_username: Some("listener_bot".into()),
                    account_handle: "telegram:200".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(200),
                        chan_user_name: Some("listener_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: Some(
                        Arc::clone(&bridge) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }

        crate::state::shared_group_runtime(&accounts)
            .lock()
            .unwrap()
            .register_participant("-1001", "telegram:200");

        outbound
            .emit_group_visible_outbound_event(
                "telegram:100",
                "-1001",
                None,
                None,
                None,
                "done",
                Some(moltis_channels::ChannelMessageKind::Text),
                "50",
            )
            .await;

        let requests = bridge.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].inbound.body.text, "风险助手(bot): done");
    }

    #[tokio::test]
    async fn group_visible_outbound_dispatch_fuse_downgrades_without_root_context() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        let bridge = Arc::new(RecordingBridge::default());

        {
            let mut map = accounts.write().unwrap();
            map.insert(
                "telegram:100".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-a"),
                    bot_user_id: Some(UserId(100)),
                    bot_username: Some("source_bot".into()),
                    account_handle: "telegram:100".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(100),
                        chan_user_name: Some("source_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
            map.insert(
                "telegram:200".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-b"),
                    bot_user_id: Some(UserId(200)),
                    bot_username: Some("listener_bot".into()),
                    account_handle: "telegram:200".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(200),
                        chan_user_name: Some("listener_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: Some(
                        Arc::clone(&bridge) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }

        crate::state::shared_group_runtime(&accounts)
            .lock()
            .unwrap()
            .register_participant("-1001", "telegram:200");

        outbound
            .emit_group_visible_outbound_event(
                "telegram:100",
                "-1001",
                None,
                None,
                None,
                "@listener_bot take this",
                Some(moltis_channels::ChannelMessageKind::Text),
                "50",
            )
            .await;

        let requests = bridge.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].inbound.mode,
            crate::adapter::TgInboundMode::RecordOnly
        );
        assert!(requests[0].inbound.private_source.addressed);
    }

    #[test]
    fn register_group_visible_outbound_contexts_propagates_root_to_all_sent_chunk_ids() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };

        {
            let binding = crate::state::shared_group_runtime(&accounts);
            let mut runtime = binding.lock().unwrap();
            runtime.ensure_external_root_dispatch("-1001", "m100");
        }

        outbound.register_group_visible_outbound_contexts(
            "telegram:100",
            "-1001",
            Some("m100"),
            &["m101".to_string(), "m102".to_string(), "m103".to_string()],
        );

        let binding = crate::state::shared_group_runtime(&accounts);
        let mut runtime = binding.lock().unwrap();
        for message_id in ["m101", "m102", "m103"] {
            let context = runtime.message_context("-1001", message_id).unwrap();
            assert_eq!(context.root_message_id, "m100");
            assert_eq!(
                context.managed_author_account_handle.as_deref(),
                Some("telegram:100")
            );
        }
    }

    #[test]
    fn budget_exhaustion_logs_warn_then_info_with_fixed_reason_code() {
        let (logs, ()) = capture_json_logs(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
                let outbound = TelegramOutbound {
                    accounts: Arc::clone(&accounts),
                };
                let bridge = Arc::new(RecordingBridge::default());

                {
                    let mut map = accounts.write().unwrap();
                    map.insert(
                        "telegram:100".into(),
                        crate::state::AccountState {
                            bot: teloxide::Bot::new("token-a"),
                            bot_user_id: Some(UserId(100)),
                            bot_username: Some("source_bot".into()),
                            account_handle: "telegram:100".into(),
                            config: crate::config::TelegramAccountConfig {
                                chan_user_id: Some(100),
                                chan_user_name: Some("source_bot".into()),
                                ..Default::default()
                            },
                            outbound: Arc::new(TelegramOutbound {
                                accounts: Arc::clone(&accounts),
                            }),
                            cancel: tokio_util::sync::CancellationToken::new(),
                            supervisor: Arc::new(std::sync::Mutex::new(None)),
                            message_log: None,
                            event_sink: None,
                            core_bridge: None,
                            polling: Arc::new(std::sync::Mutex::new(
                                crate::state::PollingRuntimeState::new(90),
                            )),
                            otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                        },
                    );
                    map.insert(
                        "telegram:200".into(),
                        crate::state::AccountState {
                            bot: teloxide::Bot::new("token-b"),
                            bot_user_id: Some(UserId(200)),
                            bot_username: Some("alpha_bot".into()),
                            account_handle: "telegram:200".into(),
                            config: crate::config::TelegramAccountConfig {
                                chan_user_id: Some(200),
                                chan_user_name: Some("alpha_bot".into()),
                                ..Default::default()
                            },
                            outbound: Arc::new(TelegramOutbound {
                                accounts: Arc::clone(&accounts),
                            }),
                            cancel: tokio_util::sync::CancellationToken::new(),
                            supervisor: Arc::new(std::sync::Mutex::new(None)),
                            message_log: None,
                            event_sink: None,
                            core_bridge: Some(
                                Arc::clone(&bridge) as Arc<dyn crate::adapter::TelegramCoreBridge>
                            ),
                            polling: Arc::new(std::sync::Mutex::new(
                                crate::state::PollingRuntimeState::new(90),
                            )),
                            otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                        },
                    );
                }

                {
                    let shared = crate::state::shared_group_runtime(&accounts);
                    let mut group_runtime = shared.lock().unwrap();
                    group_runtime.set_bot_dispatch_cycle_budget(1);
                    group_runtime.register_participant("-1001", "telegram:200");
                    group_runtime.ensure_external_root_dispatch("-1001", "m-root");
                    group_runtime.register_sent_message_contexts(
                        "-1001",
                        &["50".to_string(), "51".to_string(), "52".to_string()],
                        "telegram:100",
                        "m-root",
                    );
                }

                outbound
                    .emit_group_visible_outbound_event(
                        "telegram:100",
                        "-1001",
                        None,
                        None,
                        None,
                        "@alpha_bot one",
                        Some(moltis_channels::ChannelMessageKind::Text),
                        "50",
                    )
                    .await;
                outbound
                    .emit_group_visible_outbound_event(
                        "telegram:100",
                        "-1001",
                        None,
                        None,
                        None,
                        "@alpha_bot two",
                        Some(moltis_channels::ChannelMessageKind::Text),
                        "51",
                    )
                    .await;
                outbound
                    .emit_group_visible_outbound_event(
                        "telegram:100",
                        "-1001",
                        None,
                        None,
                        None,
                        "@alpha_bot three",
                        Some(moltis_channels::ChannelMessageKind::Text),
                        "52",
                    )
                    .await;
            });
        });

        let fuse_logs = logs
            .iter()
            .filter(|entry| {
                entry["fields"]["event"] == "telegram.group.dispatch_fuse"
                    && entry["fields"]["reason_code"] == "root_dispatch_budget_exceeded"
            })
            .collect::<Vec<_>>();
        assert_eq!(fuse_logs.len(), 2);
        assert_eq!(fuse_logs[0]["level"], "WARN");
        assert_eq!(fuse_logs[1]["level"], "INFO");
    }

    #[tokio::test]
    async fn group_visible_outbound_uses_stable_target_order_and_downgrades_after_budget_exhaustion()
     {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        let bridge_a = Arc::new(RecordingBridge::default());
        let bridge_b = Arc::new(RecordingBridge::default());

        {
            let mut map = accounts.write().unwrap();
            map.insert(
                "telegram:100".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-a"),
                    bot_user_id: Some(UserId(100)),
                    bot_username: Some("source_bot".into()),
                    account_handle: "telegram:100".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(100),
                        chan_user_name: Some("source_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
            map.insert(
                "telegram:200".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-b"),
                    bot_user_id: Some(UserId(200)),
                    bot_username: Some("alpha_bot".into()),
                    account_handle: "telegram:200".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(200),
                        chan_user_name: Some("alpha_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: Some(
                        Arc::clone(&bridge_a) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
            map.insert(
                "telegram:300".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-c"),
                    bot_user_id: Some(UserId(300)),
                    bot_username: Some("beta_bot".into()),
                    account_handle: "telegram:300".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(300),
                        chan_user_name: Some("beta_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: Some(
                        Arc::clone(&bridge_b) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }

        {
            let shared = crate::state::shared_group_runtime(&accounts);
            let mut runtime = shared.lock().unwrap();
            runtime.set_bot_dispatch_cycle_budget(1);
            runtime.register_participant("-1001", "telegram:300");
            runtime.register_participant("-1001", "telegram:200");
            runtime.ensure_external_root_dispatch("-1001", "m-root");
            runtime.register_sent_message_contexts(
                "-1001",
                &["50".to_string()],
                "telegram:100",
                "m-root",
            );
        }

        outbound
            .emit_group_visible_outbound_event(
                "telegram:100",
                "-1001",
                None,
                None,
                None,
                "@alpha_bot @beta_bot take this",
                Some(moltis_channels::ChannelMessageKind::Text),
                "50",
            )
            .await;

        let requests_a = bridge_a.requests.lock().unwrap();
        let requests_b = bridge_b.requests.lock().unwrap();
        assert_eq!(requests_a.len(), 1);
        assert_eq!(requests_b.len(), 1);
        assert_eq!(
            requests_a[0].inbound.mode,
            crate::adapter::TgInboundMode::Dispatch
        );
        assert_eq!(
            requests_b[0].inbound.mode,
            crate::adapter::TgInboundMode::RecordOnly
        );
    }

    #[test]
    fn split_caption_for_telegram_limits_head_and_preserves_text() {
        let long = "a".repeat(crate::markdown::TELEGRAM_CAPTION_LIMIT + 10);
        let (head, tail) = super::split_caption_for_telegram(&long);
        assert!(head.len() <= crate::markdown::TELEGRAM_CAPTION_LIMIT);
        assert_eq!(format!("{head}{tail}"), long);
        assert!(!tail.is_empty());
    }

    #[tokio::test]
    async fn send_typing_propagates_send_chat_action_failure() {
        let bot = teloxide::Bot::new("123:ABC").set_api_url("http://127.0.0.1:1/".parse().unwrap());
        let outbound = outbound_with_bot("acct", bot);
        let err = ChannelOutbound::send_typing(&outbound, "acct", "1")
            .await
            .expect_err("send_typing must propagate failures");
        assert!(
            err.to_string().to_lowercase().contains("error")
                || err.to_string().to_lowercase().contains("connect")
                || err.to_string().to_lowercase().contains("refused")
        );
    }

    #[tokio::test]
    async fn send_typing_failure_does_not_change_account_runtime_state() {
        let bot = teloxide::Bot::new("123:ABC").set_api_url("http://127.0.0.1:1/".parse().unwrap());
        let (outbound, accounts) = outbound_with_bot_and_accounts("acct", bot);
        {
            let map = accounts.read().unwrap();
            let state = map.get("acct").unwrap();
            let mut polling = state.polling.lock().unwrap();
            polling.polling_state = crate::state::PollingState::Running;
            polling.current_reason_code = None;
        }

        let _err = ChannelOutbound::send_typing(&outbound, "acct", "1")
            .await
            .expect_err("send_typing must fail against the mock-unreachable API url");

        let map = accounts.read().unwrap();
        let state = map.get("acct").unwrap();
        let polling = state.polling.lock().unwrap();
        assert_eq!(polling.polling_state, crate::state::PollingState::Running);
        assert_eq!(polling.current_reason_code, None);
    }

    #[tokio::test]
    async fn send_reply_propagates_send_chat_action_failure() {
        let bot = teloxide::Bot::new("123:ABC").set_api_url("http://127.0.0.1:1/".parse().unwrap());
        let outbound = empty_outbound();
        let err = outbound
            .send_reply(
                &bot,
                "1",
                &ReplyPayload {
                    text: "hello".to_string(),
                    media: None,
                    reply_to_message_id: None,
                    silent: false,
                },
            )
            .await
            .expect_err("send_reply must propagate typing failures");
        assert!(
            err.to_string().to_lowercase().contains("error")
                || err.to_string().to_lowercase().contains("connect")
                || err.to_string().to_lowercase().contains("refused")
                || err.to_string().to_lowercase().contains("timed out")
        );
    }

    #[tokio::test]
    async fn send_typing_times_out_quickly() {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: cannot bind test listener: {e}");
                return;
            },
        };
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new()
            .route("/{*path}", post(slow_typing_api))
            .with_state(SlowTypingApi);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve slow typing api");
        });

        let bot =
            teloxide::Bot::new("123:ABC").set_api_url(format!("http://{addr}/").parse().unwrap());
        let outbound = outbound_with_bot("acct", bot);
        let err = ChannelOutbound::send_typing(&outbound, "acct", "1")
            .await
            .expect_err("send_typing must timeout");
        assert!(
            err.to_string().to_lowercase().contains("timed out"),
            "expected typing timeout error, got {err}"
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    async fn network_request_error() -> teloxide::RequestError {
        teloxide::Bot::new("123:ABC")
            .set_api_url("http://127.0.0.1:1".parse().unwrap())
            .send_message(ChatId(1), "hello")
            .await
            .unwrap_err()
    }

    #[derive(Clone, Default)]
    struct SlowTypingApi;

    async fn slow_typing_api(
        State(_state): State<SlowTypingApi>,
        body: Bytes,
    ) -> Json<serde_json::Value> {
        let raw_body = String::from_utf8_lossy(&body);
        if raw_body.contains("\"action\":\"typing\"") {
            tokio::time::sleep(TELEGRAM_TYPING_REQUEST_TIMEOUT * 2).await;
        }
        Json(serde_json::json!({
            "ok": true,
            "result": true
        }))
    }

    #[derive(Clone, Default)]
    struct TelegramSendApi {
        next_message_id: Arc<Mutex<i32>>,
    }

    async fn telegram_send_api(
        State(state): State<TelegramSendApi>,
        body: Bytes,
    ) -> Json<serde_json::Value> {
        let raw_body = String::from_utf8_lossy(&body);
        if raw_body.contains("\"action\":\"typing\"") {
            return Json(serde_json::json!({
                "ok": true,
                "result": true
            }));
        }

        let mut next_message_id = state.next_message_id.lock().unwrap();
        let message_id = *next_message_id;
        *next_message_id += 1;
        Json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": message_id,
                "date": 0,
                "chat": {
                    "id": -1001,
                    "type": "supergroup",
                    "title": "ops"
                },
                "text": "ok"
            }
        }))
    }

    fn invalid_json_error() -> teloxide::RequestError {
        teloxide::RequestError::InvalidJson {
            source: serde_json::from_str::<serde_json::Value>("not-json").unwrap_err(),
            raw: Box::<str>::from("not-json"),
        }
    }

    #[test]
    fn classify_retry_after_as_definitive_failure() {
        let err = teloxide::RequestError::RetryAfter(Seconds::from_seconds(2));
        assert_eq!(
            classify_outcome_kind(TelegramOutboundOp::SendMessage, &err),
            OutboundOutcomeKind::DefinitiveFailure
        );
        assert!(matches!(
            classify_retry_decision(TelegramOutboundOp::SendMessage, &err, 1, retry_cfg(3),),
            RetryDecision::Retry(_)
        ));
    }

    #[tokio::test]
    async fn classify_send_message_network_error_as_unknown_outcome() {
        let err = network_request_error().await;
        assert_eq!(
            classify_outcome_kind(TelegramOutboundOp::SendMessage, &err),
            OutboundOutcomeKind::UnknownOutcome
        );
    }

    #[test]
    fn invalid_json_unknown_outcome_is_not_replayed() {
        let err = invalid_json_error();
        assert_eq!(
            classify_outcome_kind(TelegramOutboundOp::SendMessage, &err),
            OutboundOutcomeKind::UnknownOutcome
        );
        assert_eq!(
            classify_retry_decision(TelegramOutboundOp::SendMessage, &err, 1, retry_cfg(3)),
            RetryDecision::GiveUp
        );
    }

    #[tokio::test]
    async fn send_text_by_reply_target_ref_with_explicit_lineage_preserves_root_for_unthreaded_group_send()
     {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new()
            .route("/{*path}", post(telegram_send_api))
            .with_state(TelegramSendApi {
                next_message_id: Arc::new(Mutex::new(501)),
            });
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve telegram send api");
        });

        let bot =
            teloxide::Bot::new("123:ABC").set_api_url(format!("http://{addr}/").parse().unwrap());
        let (outbound, accounts) = outbound_with_bot_and_accounts("telegram:100", bot);

        {
            let shared = crate::state::shared_group_runtime(&accounts);
            let mut runtime = shared.lock().unwrap();
            runtime.ensure_external_root_dispatch("-1001", "m100");
        }

        let reply_target_ref = serde_json::json!({
            "v": 1,
            "type": "telegram",
            "accountKey": "telegram:100",
            "chatId": "-1001",
            "messageId": serde_json::Value::Null,
            "lineageMessageId": "m100"
        })
        .to_string();

        let sent = ChannelOutbound::send_text_by_reply_target_ref_with_ref(
            &outbound,
            &reply_target_ref,
            "@peer continue",
        )
        .await
        .unwrap()
        .expect("sent ref");
        assert_eq!(sent.message_id, "501");

        let shared = crate::state::shared_group_runtime(&accounts);
        let mut runtime = shared.lock().unwrap();
        let context = runtime.message_context("-1001", "501");
        assert!(
            context.is_some(),
            "expected sent message to keep root context for explicit lineage"
        );
        let context = context.unwrap();
        assert_eq!(context.root_message_id, "m100");
        assert_eq!(
            context.managed_author_account_handle.as_deref(),
            Some("telegram:100")
        );

        let _ = shutdown_tx.send(());
        server.await.expect("server join");
    }

    #[tokio::test]
    async fn group_visible_outbound_still_fans_out_after_transient_chat_capacity_eviction() {
        let accounts: AccountStateMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        let bridge = Arc::new(RecordingBridge::default());

        {
            let mut map = accounts.write().unwrap();
            map.insert(
                "telegram:100".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-a"),
                    bot_user_id: Some(UserId(100)),
                    bot_username: Some("source_bot".into()),
                    account_handle: "telegram:100".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(100),
                        chan_user_name: Some("source_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: None,
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
            map.insert(
                "telegram:200".into(),
                crate::state::AccountState {
                    bot: teloxide::Bot::new("token-b"),
                    bot_user_id: Some(UserId(200)),
                    bot_username: Some("listener_bot".into()),
                    account_handle: "telegram:200".into(),
                    config: crate::config::TelegramAccountConfig {
                        chan_user_id: Some(200),
                        chan_user_name: Some("listener_bot".into()),
                        ..Default::default()
                    },
                    outbound: Arc::new(TelegramOutbound {
                        accounts: Arc::clone(&accounts),
                    }),
                    cancel: tokio_util::sync::CancellationToken::new(),
                    supervisor: Arc::new(std::sync::Mutex::new(None)),
                    message_log: None,
                    event_sink: None,
                    core_bridge: Some(
                        Arc::clone(&bridge) as Arc<dyn crate::adapter::TelegramCoreBridge>
                    ),
                    polling: Arc::new(std::sync::Mutex::new(
                        crate::state::PollingRuntimeState::new(90),
                    )),
                    otp: std::sync::Mutex::new(crate::otp::OtpState::new(300)),
                },
            );
        }

        {
            let shared = crate::state::shared_group_runtime(&accounts);
            let mut runtime = shared.lock().unwrap();
            runtime.register_participant("-1001", "telegram:200");
            runtime.ensure_external_root_dispatch("-1001", "m-root");
            for index in 0..2048 {
                let chat_id = format!("chat-fill-{index}");
                let message_id = format!("m-{index}");
                runtime.ensure_external_root_dispatch(&chat_id, &message_id);
            }
        }

        outbound
            .emit_group_visible_outbound_event(
                "telegram:100",
                "-1001",
                None,
                None,
                None,
                "@listener_bot still there?",
                Some(moltis_channels::ChannelMessageKind::Text),
                "50",
            )
            .await;

        let requests = bridge.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].inbound.mode,
            crate::adapter::TgInboundMode::RecordOnly
        );
    }

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

    #[tokio::test]
    async fn telegram_outbound_send_text_retries_retryable_failure_then_succeeds() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_send_results(vec![
            MockSendResult::Err(teloxide::RequestError::RetryAfter(Seconds::from_seconds(0))),
            MockSendResult::Ok(MessageId(42)),
        ]);

        let sent = outbound
            .send_message_with_retry(
                &transport,
                retry_cfg(3),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::SendMessage,
                "hello",
                SendMessageOptions {
                    disable_notification: false,
                    reply_parameters: None,
                    thread_id: None,
                },
                Some(OutboundDeliveryState::FirstChunkUnsent),
                None,
                Some(0),
                Some(1),
                None,
                5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(sent, MessageId(42));
        assert_eq!(transport.send_calls(), 2);
    }

    #[tokio::test]
    async fn telegram_outbound_send_message_preserves_thread_id() {
        let outbound = empty_outbound();
        let transport =
            MockTextTransport::with_send_results(vec![MockSendResult::Ok(MessageId(42))]);

        let sent = outbound
            .send_message_with_retry(
                &transport,
                retry_cfg(1),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::SendMessage,
                "hello",
                SendMessageOptions {
                    disable_notification: false,
                    reply_parameters: None,
                    thread_id: Some(ThreadId(MessageId(7))),
                },
                Some(OutboundDeliveryState::FirstChunkUnsent),
                None,
                Some(0),
                Some(1),
                None,
                5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(sent, MessageId(42));
        assert_eq!(*transport.last_send_thread_id.lock().unwrap(), Some(7));
    }

    #[tokio::test]
    async fn telegram_outbound_send_text_network_error_retries_then_succeeds() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_send_results(vec![
            MockSendResult::Err(network_request_error().await),
            MockSendResult::Ok(MessageId(7)),
        ]);

        let sent = outbound
            .send_message_with_retry(
                &transport,
                retry_cfg(3),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::SendMessage,
                "hello",
                SendMessageOptions {
                    disable_notification: false,
                    reply_parameters: None,
                    thread_id: None,
                },
                Some(OutboundDeliveryState::FirstChunkUnsent),
                None,
                Some(0),
                Some(1),
                None,
                5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(sent, MessageId(7));
        assert_eq!(transport.send_calls(), 2);
    }

    #[tokio::test]
    async fn outbound_max_attempts_one_disables_retry_for_network_error() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_send_results(vec![MockSendResult::Err(
            network_request_error().await,
        )]);

        let err = outbound
            .send_message_with_retry(
                &transport,
                retry_cfg(1),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::SendMessage,
                "hello",
                SendMessageOptions {
                    disable_notification: false,
                    reply_parameters: None,
                    thread_id: None,
                },
                Some(OutboundDeliveryState::FirstChunkUnsent),
                None,
                Some(0),
                Some(1),
                None,
                5,
                None,
            )
            .await
            .unwrap_err();

        let outbound_err = err.downcast_ref::<TelegramOutboundError>().unwrap();
        assert_eq!(outbound_err.attempts, 1);
        assert_eq!(outbound_err.max_attempts, 1);
        assert_eq!(transport.send_calls(), 1);
    }

    #[tokio::test]
    async fn telegram_outbound_send_text_gives_up_on_non_retryable_failure() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_send_results(vec![MockSendResult::Err(
            teloxide::RequestError::Api(teloxide::ApiError::BotBlocked),
        )]);

        let err = outbound
            .send_message_with_retry(
                &transport,
                retry_cfg(3),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::SendMessage,
                "hello",
                SendMessageOptions {
                    disable_notification: false,
                    reply_parameters: None,
                    thread_id: None,
                },
                Some(OutboundDeliveryState::PartialSent),
                None,
                Some(1),
                Some(2),
                None,
                5,
                None,
            )
            .await
            .unwrap_err();

        let outbound_err = err.downcast_ref::<TelegramOutboundError>().unwrap();
        assert_eq!(
            outbound_err.outcome_kind,
            OutboundOutcomeKind::NonRetryableFailure
        );
        assert_eq!(
            outbound_err.delivery_state,
            Some(OutboundDeliveryState::PartialSent)
        );
        assert_eq!(transport.send_calls(), 1);
    }

    #[tokio::test]
    async fn message_not_modified_after_retry_is_treated_as_success_equivalent() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_edit_results(vec![
            MockEditResult::Err(network_request_error().await),
            MockEditResult::Err(teloxide::RequestError::Api(
                teloxide::ApiError::MessageNotModified,
            )),
        ]);

        outbound
            .edit_message_with_retry(
                &transport,
                retry_cfg(3),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::EditMessageText,
                MessageId(99),
                "hello",
                Some(OutboundDeliveryState::PlaceholderSent),
                5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(transport.edit_calls(), 2);
    }

    #[tokio::test]
    async fn unknown_outcome_edit_message_allows_controlled_retry_in_phase1() {
        let outbound = empty_outbound();
        let transport = MockTextTransport::with_edit_results(vec![
            MockEditResult::Err(network_request_error().await),
            MockEditResult::Ok,
        ]);

        outbound
            .edit_message_with_retry(
                &transport,
                retry_cfg(3),
                "acct",
                ChatId(1),
                "1",
                TelegramOutboundOp::EditMessageText,
                MessageId(5),
                "hello",
                Some(OutboundDeliveryState::PlaceholderSent),
                5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(transport.edit_calls(), 2);
    }

    #[tokio::test]
    async fn telegram_outbound_stream_edit_retries_then_degrades() {
        let outbound = empty_outbound();
        let transport = MockTextTransport {
            send_results: Mutex::new(vec![MockSendResult::Ok(MessageId(11))].into()),
            edit_results: Mutex::new(
                vec![
                    MockEditResult::Err(teloxide::RequestError::Api(
                        teloxide::ApiError::BotBlocked,
                    )),
                    MockEditResult::Ok,
                ]
                .into(),
            ),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(StreamEvent::Delta("hello".to_string()))
            .await
            .unwrap();
        tx.send(StreamEvent::Delta(" world".to_string()))
            .await
            .unwrap();
        tx.send(StreamEvent::Done).await.unwrap();
        drop(tx);

        outbound
            .send_stream_with_transport(&transport, retry_cfg(3), "acct", "1", 0, &mut rx)
            .await
            .unwrap();

        assert_eq!(
            transport.send_calls(),
            1,
            "no extra send_message during degraded edits"
        );
        assert_eq!(
            transport.edit_calls(),
            2,
            "one failed incremental edit, then one final edit after degradation"
        );
    }

    #[tokio::test]
    async fn telegram_outbound_stream_typing_stays_active_until_stream_finishes() {
        let outbound = empty_outbound();
        let transport = MockTextTransport {
            send_results: Mutex::new(vec![MockSendResult::Ok(MessageId(61))].into()),
            edit_results: Mutex::new(vec![MockEditResult::Ok].into()),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(8);

        let task = tokio::spawn(async move {
            outbound
                .send_stream_with_transport(
                    &transport,
                    retry_cfg(3),
                    "acct",
                    "1",
                    u64::MAX,
                    &mut rx,
                )
                .await
                .map(|_| transport)
        });

        tokio::task::yield_now().await;
        tx.send(StreamEvent::Delta("hello".to_string()))
            .await
            .unwrap();
        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        tx.send(StreamEvent::Done).await.unwrap();
        drop(tx);

        let transport = task.await.unwrap().unwrap();
        assert!(
            transport.typing_calls() >= 2,
            "typing should remain active after the first delta until the stream fully finishes"
        );

        let final_typings = transport.typing_calls();
        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        assert_eq!(transport.typing_calls(), final_typings);
    }

    #[derive(Default)]
    struct BlockingTypingTransport {
        send_calls: Mutex<usize>,
        typing_started: tokio::sync::Notify,
    }

    #[async_trait]
    impl TelegramTextTransport for BlockingTypingTransport {
        async fn send_typing(
            &self,
            _chat_id: ChatId,
            _thread_id: Option<ThreadId>,
        ) -> std::result::Result<(), TypingRequestError> {
            self.typing_started.notify_waiters();
            std::future::pending::<()>().await;
            Ok(())
        }

        async fn send_message_html(
            &self,
            _chat_id: ChatId,
            _text: &str,
            _options: SendMessageOptions,
        ) -> std::result::Result<MessageId, teloxide::RequestError> {
            *self.send_calls.lock().unwrap() += 1;
            Ok(MessageId(71))
        }

        async fn edit_message_text_html(
            &self,
            _chat_id: ChatId,
            _message_id: MessageId,
            _text: &str,
        ) -> std::result::Result<(), teloxide::RequestError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn telegram_outbound_stream_send_is_not_blocked_by_slow_typing_request() {
        let outbound = empty_outbound();
        let transport = Arc::new(BlockingTypingTransport::default());
        let typing_started_transport = Arc::clone(&transport);
        let typing_started = async move {
            typing_started_transport.typing_started.notified().await;
        };
        let observed_transport = Arc::clone(&transport);
        let (tx, mut rx) = mpsc::channel(8);

        let task = tokio::spawn(async move {
            outbound
                .send_stream_with_transport(
                    transport.as_ref(),
                    retry_cfg(3),
                    "acct",
                    "1",
                    u64::MAX,
                    &mut rx,
                )
                .await
        });

        tokio::time::timeout(std::time::Duration::from_millis(50), typing_started)
            .await
            .expect("typing keepalive must start");
        tokio::time::timeout(std::time::Duration::from_millis(50), async {
            loop {
                if *observed_transport.send_calls.lock().unwrap() > 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("placeholder send must not wait for typing");
        assert_eq!(*observed_transport.send_calls.lock().unwrap(), 1);

        tx.send(StreamEvent::Done).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();
    }

    #[derive(Default)]
    struct RecordingTypingOutbound {
        typing_targets: Mutex<Vec<ChannelReplyTarget>>,
    }

    #[async_trait]
    impl ChannelOutbound for RecordingTypingOutbound {
        async fn send_text(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> Result<()> {
            panic!("run_with_targeted_typing_loop must use structured typing targets");
        }

        async fn send_typing_to_target(&self, target: &ChannelReplyTarget) -> Result<()> {
            self.typing_targets.lock().unwrap().push(target.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_with_targeted_typing_loop_preserves_thread_id() {
        let recorder = Arc::new(RecordingTypingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = recorder.clone();
        let target = TgPrivateTarget {
            account_handle: "telegram:acct".into(),
            chat_id: "-1001".into(),
            message_id: None,
            thread_id: Some("77".into()),
        };

        run_with_targeted_typing_loop(
            outbound,
            target,
            "test_typing_thread",
            std::time::Duration::from_millis(5),
            async {
                tokio::time::sleep(std::time::Duration::from_millis(12)).await;
            },
        )
        .await;

        let typing_targets = recorder.typing_targets.lock().unwrap();
        assert!(
            !typing_targets.is_empty(),
            "typing loop must emit at least one typing target"
        );
        assert_eq!(typing_targets[0].thread_id.as_deref(), Some("77"));
    }

    #[tokio::test]
    async fn spawn_targeted_typing_loop_until_preserves_thread_id() {
        let recorder = Arc::new(RecordingTypingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = recorder.clone();
        let target = TgPrivateTarget {
            account_handle: "telegram:acct".into(),
            chat_id: "-1001".into(),
            message_id: None,
            thread_id: Some("77".into()),
        };
        let finish = Arc::new(tokio::sync::Notify::new());
        let finish_wait = Arc::clone(&finish);

        spawn_targeted_typing_loop_until(
            outbound,
            target,
            "test_typing_thread_background",
            std::time::Duration::from_millis(5),
            async move {
                finish_wait.notified().await;
            },
        );

        tokio::time::timeout(std::time::Duration::from_millis(50), async {
            loop {
                if !recorder.typing_targets.lock().unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("typing loop must emit a structured target");
        finish.notify_waiters();
        tokio::task::yield_now().await;

        let typing_targets = recorder.typing_targets.lock().unwrap();
        assert_eq!(typing_targets[0].thread_id.as_deref(), Some("77"));
    }

    #[tokio::test]
    async fn telegram_outbound_stream_final_edit_failure_returns_error() {
        let outbound = empty_outbound();
        let transport = MockTextTransport {
            send_results: Mutex::new(vec![MockSendResult::Ok(MessageId(21))].into()),
            edit_results: Mutex::new(
                vec![MockEditResult::Err(teloxide::RequestError::Api(
                    teloxide::ApiError::BotBlocked,
                ))]
                .into(),
            ),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(StreamEvent::Delta("hello".to_string()))
            .await
            .unwrap();
        tx.send(StreamEvent::Done).await.unwrap();
        drop(tx);

        let err = outbound
            .send_stream_with_transport(&transport, retry_cfg(3), "acct", "1", u64::MAX, &mut rx)
            .await
            .unwrap_err();
        let outbound_err = err.downcast_ref::<TelegramOutboundError>().unwrap();

        assert_eq!(outbound_err.op, TelegramOutboundOp::EditMessageTextFinal);
        assert_eq!(
            outbound_err.delivery_state,
            Some(OutboundDeliveryState::PlaceholderSent)
        );
    }

    #[tokio::test]
    async fn telegram_outbound_partial_chunk_failure_logs_partial_sent() {
        let outbound = empty_outbound();
        let large_text = "a".repeat(TELEGRAM_MAX_MESSAGE_LEN + 32);
        let transport = MockTextTransport {
            send_results: Mutex::new(
                vec![
                    MockSendResult::Ok(MessageId(31)),
                    MockSendResult::Err(teloxide::RequestError::Api(
                        teloxide::ApiError::BotBlocked,
                    )),
                ]
                .into(),
            ),
            edit_results: Mutex::new(vec![MockEditResult::Ok].into()),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(StreamEvent::Delta(large_text)).await.unwrap();
        tx.send(StreamEvent::Done).await.unwrap();
        drop(tx);

        let err = outbound
            .send_stream_with_transport(&transport, retry_cfg(3), "acct", "1", u64::MAX, &mut rx)
            .await
            .unwrap_err();
        let outbound_err = err.downcast_ref::<TelegramOutboundError>().unwrap();

        assert_eq!(outbound_err.op, TelegramOutboundOp::SendMessageStreamChunk);
        assert_eq!(
            outbound_err.delivery_state,
            Some(OutboundDeliveryState::PartialSent)
        );
    }

    #[tokio::test]
    async fn telegram_outbound_stream_error_does_not_expose_internal_error_text() {
        let outbound = empty_outbound();
        let transport = MockTextTransport {
            send_results: Mutex::new(vec![MockSendResult::Ok(MessageId(51))].into()),
            edit_results: Mutex::new(vec![MockEditResult::Ok, MockEditResult::Ok].into()),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(StreamEvent::Delta("hello".to_string()))
            .await
            .unwrap();
        tx.send(StreamEvent::Error("boom".to_string()))
            .await
            .unwrap();
        drop(tx);

        outbound
            .send_stream_with_transport(&transport, retry_cfg(3), "acct", "1", 0, &mut rx)
            .await
            .unwrap();

        let last = transport
            .last_edit_text
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
        assert!(
            !last.contains("boom"),
            "telegram user-visible stream output must not include internal error text"
        );
        assert!(
            last.contains("Something went wrong while generating the response"),
            "expected sanitized failure hint in final output"
        );
    }
}

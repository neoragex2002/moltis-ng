use std::{future::Future, sync::Arc, time::Instant};

use {
    secrecy::ExposeSecret,
    teloxide::{
        ApiError, RequestError,
        prelude::*,
        types::{AllowedUpdate, BotCommand, UpdateKind},
    },
    tokio_util::sync::CancellationToken,
    tracing::{debug, error, info, warn},
};

use moltis_channels::{ChannelEventSink, message_log::MessageLog};

use crate::{
    config::TelegramAccountConfig,
    handlers,
    outbound::TelegramOutbound,
    state::{AccountState, AccountStateMap, PollingRuntimeState, PollingState},
};

const INBOUND_MAX_ATTEMPTS: u8 = 3;
const DEFAULT_STALE_THRESHOLD_SECS: u64 = 90;
const SHORT_BACKOFF_SECS: u64 = 5;
const CONFLICT_BACKOFF_SECS: u64 = 15;
const AUTH_BACKOFF_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryAction {
    StopBatch { attempt: u8 },
    Quarantine { attempt: u8 },
}

fn record_retry_attempt(
    attempts_by_update_id: &mut std::collections::HashMap<u32, u8>,
    update_id: u32,
) -> RetryAction {
    let attempt = attempts_by_update_id
        .entry(update_id)
        .and_modify(|n| *n = n.saturating_add(1))
        .or_insert(1);
    if *attempt >= INBOUND_MAX_ATTEMPTS {
        RetryAction::Quarantine { attempt: *attempt }
    } else {
        RetryAction::StopBatch { attempt: *attempt }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBotIdentity {
    pub chan_user_id: u64,
    /// Telegram `getMe.username` (without `@`).
    pub chan_user_name: Option<String>,
    /// Telegram display name (first + last).
    pub chan_nickname: Option<String>,
}

pub async fn probe_bot_identity(token: &str) -> anyhow::Result<TelegramBotIdentity> {
    let client = teloxide::net::default_reqwest_settings()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let bot = teloxide::Bot::with_client(token, client);
    let me = bot.get_me().await?;
    let first = me.first_name.clone();
    let last = me.last_name.clone().unwrap_or_default();
    let nick = format!("{first} {last}").trim().to_string();
    Ok(TelegramBotIdentity {
        chan_user_id: me.id.0,
        chan_user_name: me.username.clone(),
        chan_nickname: (!nick.is_empty()).then_some(nick),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollingAttemptOutcome {
    Retryable {
        reason_code: &'static str,
        backoff_secs: u64,
    },
    StoppedByOperator,
}

fn build_bot(token: &str) -> anyhow::Result<teloxide::Bot> {
    let client = teloxide::net::default_reqwest_settings()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    Ok(teloxide::Bot::with_client(token, client))
}

async fn run_cancellable<T, F>(cancel: &CancellationToken, future: F) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        _ = cancel.cancelled() => None,
        result = future => Some(result),
    }
}

fn classify_request_error(err: &RequestError) -> &'static str {
    match err {
        RequestError::Api(ApiError::TerminatedByOtherGetUpdates) => "token_conflict",
        RequestError::Api(ApiError::InvalidToken) => "auth_failed",
        RequestError::Network(_) => "network",
        RequestError::InvalidJson { .. } => "invalid_json",
        RequestError::RetryAfter(_) => "retry_after",
        RequestError::Api(_) => "api",
        RequestError::Io(_) => "io",
        _ => "other",
    }
}

fn classify_startup_error(stage: &'static str, err: &RequestError) -> &'static str {
    match err {
        RequestError::Api(ApiError::InvalidToken) => "auth_failed",
        _ => stage,
    }
}

fn backoff_secs_for_error(err: &RequestError) -> u64 {
    match err {
        RequestError::Api(ApiError::TerminatedByOtherGetUpdates) => CONFLICT_BACKOFF_SECS,
        RequestError::Api(ApiError::InvalidToken) => AUTH_BACKOFF_SECS,
        RequestError::RetryAfter(secs) => u64::from(secs.seconds()).max(1),
        _ => SHORT_BACKOFF_SECS,
    }
}

fn mark_running(poll_state: &std::sync::Mutex<PollingRuntimeState>) {
    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
    s.polling_state = PollingState::Running;
    s.polling_started_at = std::time::Instant::now();
    s.current_reason_code = None;
    s.current_backoff_secs = 0;
    s.last_poll_exit_reason_code = None;
}

fn mark_reconnecting(
    poll_state: &std::sync::Mutex<PollingRuntimeState>,
    reason_code: &'static str,
    backoff_secs: u64,
) {
    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
    s.polling_state = PollingState::Reconnecting;
    s.current_reason_code = Some(reason_code);
    s.current_backoff_secs = backoff_secs;
    s.last_poll_exit_reason_code = Some(reason_code);
}

fn mark_stopped_by_operator(poll_state: &std::sync::Mutex<PollingRuntimeState>) {
    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
    s.polling_state = PollingState::StoppedByOperator;
    s.current_reason_code = Some("stopped_by_operator");
    s.current_backoff_secs = 0;
    s.last_poll_exit_reason_code = Some("stopped_by_operator");
}

fn update_next_offset(poll_state: &std::sync::Mutex<PollingRuntimeState>, offset: i32) {
    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
    s.next_update_offset = offset;
}

fn update_account_identity(
    accounts: &AccountStateMap,
    account_handle: &str,
    bot_user_id: teloxide::types::UserId,
    bot_username: Option<String>,
) {
    let mut map = accounts.write().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = map.get_mut(account_handle) {
        state.bot_user_id = Some(bot_user_id);
        state.bot_username = bot_username;
    }
}

fn note_failure_window(
    reason_code: &'static str,
    now: Instant,
    first_failure_at: &mut Option<Instant>,
    outage_reason_code: &mut Option<&'static str>,
) {
    if first_failure_at.is_none() {
        *first_failure_at = Some(now);
        *outage_reason_code = Some(reason_code);
    }
}

async fn connect_polling_attempt(
    account_handle: &str,
    bot: &teloxide::Bot,
    accounts: &AccountStateMap,
    poll_state: &std::sync::Mutex<PollingRuntimeState>,
    cancel: &CancellationToken,
) -> PollingAttemptOutcome {
    let me = match run_cancellable(cancel, bot.get_me().send()).await {
        Some(Ok(me)) => me,
        Some(Err(err)) => {
            return PollingAttemptOutcome::Retryable {
                reason_code: classify_startup_error("get_me_failed", &err),
                backoff_secs: backoff_secs_for_error(&err),
            };
        },
        None => return PollingAttemptOutcome::StoppedByOperator,
    };
    update_account_identity(accounts, account_handle, me.id, me.username.clone());

    match run_cancellable(cancel, bot.delete_webhook().send()).await {
        Some(Ok(_)) => {},
        Some(Err(err)) => {
            return PollingAttemptOutcome::Retryable {
                reason_code: classify_startup_error("delete_webhook_failed", &err),
                backoff_secs: backoff_secs_for_error(&err),
            };
        },
        None => return PollingAttemptOutcome::StoppedByOperator,
    }

    let commands = vec![
        BotCommand::new("new", "Start a new session"),
        BotCommand::new("sessions", "List and switch sessions"),
        BotCommand::new("model", "Switch provider/model"),
        BotCommand::new("sandbox", "Toggle sandbox and choose image"),
        BotCommand::new("clear", "Clear session history"),
        BotCommand::new("compact", "Compact session (summarize)"),
        BotCommand::new("context", "Show session context info"),
        BotCommand::new("help", "Show available commands"),
    ];
    if let Some(Err(err)) = run_cancellable(cancel, bot.set_my_commands(commands).send()).await {
        warn!(account_handle, "failed to register bot commands: {err}");
    }

    info!(
        event = "telegram.polling.connected",
        account_handle,
        runtime_state = PollingState::Running.as_str(),
        reason_code = "none",
        backoff_secs = 0u64,
        username = ?me.username,
        "telegram bot connected (webhook cleared)"
    );
    mark_running(poll_state);
    PollingAttemptOutcome::Retryable {
        reason_code: "connected",
        backoff_secs: 0,
    }
}

async fn run_polling_loop(
    account_handle: &str,
    bot: &teloxide::Bot,
    accounts: &AccountStateMap,
    poll_state: &std::sync::Mutex<PollingRuntimeState>,
    cancel: &CancellationToken,
) -> PollingAttemptOutcome {
    let mut offset = {
        let s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
        s.next_update_offset
    };
    let mut attempts_by_update_id: std::collections::HashMap<u32, u8> =
        std::collections::HashMap::new();

    loop {
        let result = match run_cancellable(
            cancel,
            bot.get_updates()
                .offset(offset)
                .timeout(30)
                .allowed_updates(vec![
                    AllowedUpdate::Message,
                    AllowedUpdate::EditedMessage,
                    AllowedUpdate::CallbackQuery,
                ])
                .send(),
        )
        .await
        {
            Some(result) => result,
            None => return PollingAttemptOutcome::StoppedByOperator,
        };

        match result {
            Ok(updates) => {
                {
                    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                    s.last_poll_ok_at = Some(std::time::Instant::now());
                }
                debug!(
                    account_handle,
                    count = updates.len(),
                    "got telegram updates"
                );
                for update in updates {
                    let update_id = update.id.0;
                    let outcome = match update.kind {
                        UpdateKind::Message(msg) => {
                            debug!(
                                account_handle,
                                chat_id = msg.chat.id.0,
                                "received telegram message"
                            );
                            handlers::handle_message_direct(msg, bot, account_handle, accounts)
                                .await
                        },
                        UpdateKind::EditedMessage(msg) => {
                            debug!(
                                account_handle,
                                chat_id = msg.chat.id.0,
                                "received telegram edited message"
                            );
                            handlers::handle_edited_location(msg, account_handle, accounts).await
                        },
                        UpdateKind::CallbackQuery(query) => {
                            debug!(
                                account_handle,
                                callback_data = ?query.data,
                                "received telegram callback query"
                            );
                            handlers::handle_callback_query(query, bot, account_handle, accounts)
                                .await
                        },
                        other => {
                            info!(
                                event = "telegram.update.ignored",
                                account_handle,
                                update_id,
                                reason_code = "unsupported_kind",
                                kind = ?other,
                                "telegram update ignored (unsupported kind)"
                            );
                            Ok(())
                        },
                    };

                    let is_retryable = outcome
                        .as_ref()
                        .err()
                        .and_then(|e| e.downcast_ref::<handlers::RetryableUpdateError>())
                        .map(|e| e.reason_code);
                    match (outcome, is_retryable) {
                        (Ok(()), _) => {
                            attempts_by_update_id.remove(&update_id);
                            offset = update.id.as_offset();
                            update_next_offset(poll_state, offset);
                            let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                            let now = std::time::Instant::now();
                            s.last_update_finished_at = Some(now);
                            s.last_retryable_failure_at = None;
                            s.last_retryable_failure_reason_code = None;
                        },
                        (Err(_err), Some(reason_code)) => {
                            match record_retry_attempt(&mut attempts_by_update_id, update_id) {
                                RetryAction::Quarantine { attempt } => {
                                    warn!(
                                        event = "telegram.update.quarantined",
                                        account_handle,
                                        update_id,
                                        reason_code = "quarantined_after_retries",
                                        last_failure_reason_code = reason_code,
                                        attempts = attempt,
                                        "telegram update quarantine after retry budget exhausted"
                                    );
                                    attempts_by_update_id.remove(&update_id);
                                    offset = update.id.as_offset();
                                    update_next_offset(poll_state, offset);
                                    let mut s =
                                        poll_state.lock().unwrap_or_else(|e| e.into_inner());
                                    let now = std::time::Instant::now();
                                    s.last_update_finished_at = Some(now);
                                    s.last_retryable_failure_at = None;
                                    s.last_retryable_failure_reason_code = None;
                                },
                                RetryAction::StopBatch { attempt } => {
                                    warn!(
                                        event = "telegram.update.retryable_failed",
                                        account_handle,
                                        update_id,
                                        reason_code,
                                        attempts = attempt,
                                        max_attempts = INBOUND_MAX_ATTEMPTS,
                                        "telegram update failed before retry barrier; stopping batch and retrying later"
                                    );
                                    {
                                        let mut s =
                                            poll_state.lock().unwrap_or_else(|e| e.into_inner());
                                        s.last_retryable_failure_at =
                                            Some(std::time::Instant::now());
                                        s.last_retryable_failure_reason_code = Some(reason_code);
                                    }
                                    if run_cancellable(
                                        cancel,
                                        tokio::time::sleep(std::time::Duration::from_secs(1)),
                                    )
                                    .await
                                    .is_none()
                                    {
                                        return PollingAttemptOutcome::StoppedByOperator;
                                    }
                                    break;
                                },
                            }
                        },
                        (Err(_err), None) => {
                            error!(
                                event = "telegram.update.handler_failed",
                                account_handle,
                                update_id,
                                reason_code = "handler_failed",
                                "error handling telegram update"
                            );
                            attempts_by_update_id.remove(&update_id);
                            offset = update.id.as_offset();
                            update_next_offset(poll_state, offset);
                            let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                            let now = std::time::Instant::now();
                            s.last_update_finished_at = Some(now);
                            s.last_retryable_failure_at = None;
                            s.last_retryable_failure_reason_code = None;
                        },
                    }
                }
            },
            Err(err) => {
                return PollingAttemptOutcome::Retryable {
                    reason_code: classify_request_error(&err),
                    backoff_secs: backoff_secs_for_error(&err),
                };
            },
        }
    }
}

/// Start polling for a single bot account.
///
/// Registers a long-lived supervisor that keeps reconnecting until the
/// account is explicitly stopped by the operator.
pub async fn start_polling(
    account_handle: String,
    config: TelegramAccountConfig,
    accounts: AccountStateMap,
    message_log: Option<Arc<dyn MessageLog>>,
    event_sink: Option<Arc<dyn ChannelEventSink>>,
) -> anyhow::Result<()> {
    let bot = build_bot(config.token.expose_secret())?;

    let cancel = CancellationToken::new();
    let supervisor = Arc::new(std::sync::Mutex::new(None));

    let outbound = Arc::new(TelegramOutbound {
        accounts: Arc::clone(&accounts),
    });

    let otp_cooldown = config.otp_cooldown_secs;
    let polling = Arc::new(std::sync::Mutex::new(PollingRuntimeState::new(
        DEFAULT_STALE_THRESHOLD_SECS,
    )));
    let state = AccountState {
        bot: bot.clone(),
        bot_user_id: config.chan_user_id.map(teloxide::types::UserId),
        bot_username: config.chan_user_name.clone(),
        account_handle: account_handle.clone(),
        config,
        outbound,
        cancel: cancel.clone(),
        supervisor: Arc::clone(&supervisor),
        message_log,
        event_sink,
        polling: Arc::clone(&polling),
        otp: std::sync::Mutex::new(crate::otp::OtpState::new(otp_cooldown)),
    };

    {
        let mut map = accounts.write().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(&account_handle) {
            return Err(anyhow::anyhow!("account already started: {account_handle}"));
        }
        map.insert(account_handle.clone(), state);
    }

    let aid = account_handle.clone();
    let poll_accounts = Arc::clone(&accounts);
    let poll_state = Arc::clone(&polling);
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        info!(
            event = "telegram.polling.supervisor_started",
            account_handle = aid,
            runtime_state = PollingState::Reconnecting.as_str(),
            reason_code = "startup",
            backoff_secs = 0u64,
            "starting telegram polling supervisor"
        );
        let mut consecutive_failures: u64 = 0;
        let mut first_failure_at: Option<Instant> = None;
        let mut last_summary_at: Option<Instant> = None;
        let mut outage_reason_code: Option<&'static str> = None;

        loop {
            if cancel_clone.is_cancelled() {
                mark_stopped_by_operator(&poll_state);
                break;
            }

            match connect_polling_attempt(&aid, &bot, &poll_accounts, &poll_state, &cancel_clone)
                .await
            {
                PollingAttemptOutcome::StoppedByOperator => {
                    mark_stopped_by_operator(&poll_state);
                    break;
                },
                PollingAttemptOutcome::Retryable {
                    reason_code,
                    backoff_secs,
                } if reason_code == "connected" => {
                    if consecutive_failures > 0 {
                        let downtime_secs =
                            first_failure_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        info!(
                            event = "telegram.polling.recovered",
                            account_handle = aid,
                            runtime_state = PollingState::Running.as_str(),
                            reason_code = outage_reason_code.unwrap_or("recovered"),
                            backoff_secs = 0u64,
                            failures = consecutive_failures,
                            downtime_secs,
                            "telegram polling recovered"
                        );
                        consecutive_failures = 0;
                        first_failure_at = None;
                        last_summary_at = None;
                        outage_reason_code = None;
                    }
                    match run_polling_loop(&aid, &bot, &poll_accounts, &poll_state, &cancel_clone)
                        .await
                    {
                        PollingAttemptOutcome::StoppedByOperator => {
                            mark_stopped_by_operator(&poll_state);
                            break;
                        },
                        PollingAttemptOutcome::Retryable {
                            reason_code,
                            backoff_secs,
                        } => {
                            mark_reconnecting(&poll_state, reason_code, backoff_secs);
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            note_failure_window(
                                reason_code,
                                Instant::now(),
                                &mut first_failure_at,
                                &mut outage_reason_code,
                            );
                            let now = Instant::now();
                            let should_log = last_summary_at
                                .map(|t| {
                                    now.duration_since(t) >= std::time::Duration::from_secs(60)
                                })
                                .unwrap_or(true);
                            if should_log {
                                last_summary_at = Some(now);
                                let downtime_secs =
                                    first_failure_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                                warn!(
                                    event = "telegram.polling.degraded",
                                    account_handle = aid,
                                    runtime_state = PollingState::Reconnecting.as_str(),
                                    reason_code,
                                    backoff_secs,
                                    consecutive_failures,
                                    downtime_secs,
                                    "telegram polling degraded"
                                );
                            }
                            if run_cancellable(
                                &cancel_clone,
                                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)),
                            )
                            .await
                            .is_none()
                            {
                                mark_stopped_by_operator(&poll_state);
                                break;
                            }
                        },
                    }
                },
                PollingAttemptOutcome::Retryable {
                    reason_code,
                    backoff_secs,
                } => {
                    mark_reconnecting(&poll_state, reason_code, backoff_secs);
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    note_failure_window(
                        reason_code,
                        Instant::now(),
                        &mut first_failure_at,
                        &mut outage_reason_code,
                    );
                    let now = Instant::now();
                    let should_log = last_summary_at
                        .map(|t| now.duration_since(t) >= std::time::Duration::from_secs(60))
                        .unwrap_or(true);
                    if should_log {
                        last_summary_at = Some(now);
                        let downtime_secs =
                            first_failure_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        warn!(
                            event = "telegram.polling.degraded",
                            account_handle = aid,
                            runtime_state = PollingState::Reconnecting.as_str(),
                            reason_code,
                            backoff_secs,
                            consecutive_failures,
                            downtime_secs,
                            "telegram polling degraded"
                        );
                    }
                    if run_cancellable(
                        &cancel_clone,
                        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)),
                    )
                    .await
                    .is_none()
                    {
                        mark_stopped_by_operator(&poll_state);
                        break;
                    }
                },
            }
        }
    });

    {
        let mut supervisor_slot = supervisor.lock().unwrap_or_else(|e| e.into_inner());
        *supervisor_slot = Some(handle);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_retry_attempt_stops_then_quarantines_on_budget() {
        let mut attempts: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();

        assert_eq!(
            record_retry_attempt(&mut attempts, 1),
            RetryAction::StopBatch { attempt: 1 }
        );
        assert_eq!(
            record_retry_attempt(&mut attempts, 1),
            RetryAction::StopBatch { attempt: 2 }
        );
        assert_eq!(
            record_retry_attempt(&mut attempts, 1),
            RetryAction::Quarantine { attempt: 3 }
        );
        assert_eq!(
            attempts.get(&1).copied(),
            Some(3),
            "caller owns removal after quarantine"
        );
    }

    #[test]
    fn classify_request_error_marks_conflict_and_auth_as_retryable_reasons() {
        let conflict = RequestError::Api(ApiError::TerminatedByOtherGetUpdates);
        let auth = RequestError::Api(ApiError::InvalidToken);
        assert_eq!(classify_request_error(&conflict), "token_conflict");
        assert_eq!(classify_request_error(&auth), "auth_failed");
    }

    #[test]
    fn backoff_secs_for_error_uses_reason_specific_values() {
        let conflict = RequestError::Api(ApiError::TerminatedByOtherGetUpdates);
        let auth = RequestError::Api(ApiError::InvalidToken);
        let retry_after = RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(17));
        let api = RequestError::Api(ApiError::BotBlocked);

        assert_eq!(backoff_secs_for_error(&conflict), CONFLICT_BACKOFF_SECS);
        assert_eq!(backoff_secs_for_error(&auth), AUTH_BACKOFF_SECS);
        assert_eq!(backoff_secs_for_error(&retry_after), 17);
        assert_eq!(backoff_secs_for_error(&api), SHORT_BACKOFF_SECS);
    }

    #[test]
    fn note_failure_window_keeps_original_outage_reason_until_recovery() {
        let now = Instant::now();
        let mut first_failure_at = None;
        let mut outage_reason_code = None;

        note_failure_window(
            "delete_webhook_failed",
            now,
            &mut first_failure_at,
            &mut outage_reason_code,
        );
        note_failure_window(
            "get_me_failed",
            now + std::time::Duration::from_secs(5),
            &mut first_failure_at,
            &mut outage_reason_code,
        );

        assert_eq!(outage_reason_code, Some("delete_webhook_failed"));
        assert_eq!(first_failure_at, Some(now));
    }
}

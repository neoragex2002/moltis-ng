use std::{sync::Arc, time::Instant};

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

/// Start polling for a single bot account.
///
/// Spawns a background task that processes updates until the returned
/// `CancellationToken` is cancelled.
pub async fn start_polling(
    account_handle: String,
    config: TelegramAccountConfig,
    accounts: AccountStateMap,
    message_log: Option<Arc<dyn MessageLog>>,
    event_sink: Option<Arc<dyn ChannelEventSink>>,
) -> anyhow::Result<CancellationToken> {
    // Build bot with a client timeout longer than the long-polling timeout (30s)
    // so the HTTP client doesn't abort the request before Telegram responds.
    let client = teloxide::net::default_reqwest_settings()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let bot = teloxide::Bot::with_client(config.token.expose_secret(), client);

    // Verify credentials and get bot username.
    let me = bot.get_me().await?;
    let bot_username = me.username.clone();
    let bot_user_id = Some(me.id);

    // Delete any existing webhook so long polling works.
    bot.delete_webhook().send().await?;

    // Register slash commands for autocomplete in Telegram clients.
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
    if let Err(e) = bot.set_my_commands(commands).await {
        warn!(account_handle, "failed to register bot commands: {e}");
    }

    info!(
        account_handle,
        username = ?bot_username,
        "telegram bot connected (webhook cleared)"
    );

    let cancel = CancellationToken::new();

    let outbound = Arc::new(TelegramOutbound {
        accounts: Arc::clone(&accounts),
    });

    let otp_cooldown = config.otp_cooldown_secs;
    let polling = Arc::new(std::sync::Mutex::new(PollingRuntimeState::new(90)));
    let state = AccountState {
        bot: bot.clone(),
        bot_user_id,
        bot_username,
        account_handle: account_handle.clone(),
        config,
        outbound,
        cancel: cancel.clone(),
        message_log,
        event_sink,
        polling: Arc::clone(&polling),
        otp: std::sync::Mutex::new(crate::otp::OtpState::new(otp_cooldown)),
    };

    {
        let mut map = accounts.write().unwrap_or_else(|e| e.into_inner());
        map.insert(account_handle.clone(), state);
    }

    let cancel_clone = cancel.clone();
    let aid = account_handle.clone();
    let poll_accounts = Arc::clone(&accounts);
    let poll_state = Arc::clone(&polling);
    tokio::spawn(async move {
        info!(
            account_handle = aid,
            "starting telegram manual polling loop"
        );
        {
            let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
            s.polling_state = PollingState::Running;
            s.polling_started_at = std::time::Instant::now();
            s.last_poll_exit_reason_code = None;
        }
        let mut offset: i32 = 0;
        let mut consecutive_failures: u64 = 0;
        let mut first_failure_at: Option<Instant> = None;
        let mut last_summary_at: Option<Instant> = None;
        let mut attempts_by_update_id: std::collections::HashMap<u32, u8> =
            std::collections::HashMap::new();

        loop {
            if cancel_clone.is_cancelled() {
                info!(account_handle = aid, "telegram polling stopped");
                {
                    let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                    s.polling_state = PollingState::Stopping;
                    s.last_poll_exit_reason_code = Some("cancelled");
                }
                break;
            }

            let result = bot
                .get_updates()
                .offset(offset)
                .timeout(30)
                .allowed_updates(vec![
                    AllowedUpdate::Message,
                    AllowedUpdate::EditedMessage,
                    AllowedUpdate::CallbackQuery,
                ])
                .await;

            match result {
                Ok(updates) => {
                    {
                        let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                        s.last_poll_ok_at = Some(std::time::Instant::now());
                    }
                    if consecutive_failures > 0 {
                        let downtime_secs =
                            first_failure_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        info!(
                            event = "telegram.polling.recovered",
                            account_handle = aid,
                            failures = consecutive_failures,
                            downtime_secs,
                            "telegram getUpdates recovered"
                        );
                        consecutive_failures = 0;
                        first_failure_at = None;
                        last_summary_at = None;
                    }
                    debug!(
                        account_handle = aid,
                        count = updates.len(),
                        "got telegram updates"
                    );
                    for update in updates {
                        let update_id = update.id.0;
                        let outcome = match update.kind {
                            UpdateKind::Message(msg) => {
                                debug!(
                                    account_handle = aid,
                                    chat_id = msg.chat.id.0,
                                    "received telegram message"
                                );
                                match handlers::handle_message_direct(
                                    msg,
                                    &bot,
                                    &aid,
                                    &poll_accounts,
                                )
                                .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(e) => Err(e),
                                }
                            },
                            UpdateKind::EditedMessage(msg) => {
                                debug!(
                                    account_handle = aid,
                                    chat_id = msg.chat.id.0,
                                    "received telegram edited message"
                                );
                                match handlers::handle_edited_location(msg, &aid, &poll_accounts)
                                    .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(e) => Err(e),
                                }
                            },
                            UpdateKind::CallbackQuery(query) => {
                                debug!(
                                    account_handle = aid,
                                    callback_data = ?query.data,
                                    "received telegram callback query"
                                );
                                match handlers::handle_callback_query(
                                    query,
                                    &bot,
                                    &aid,
                                    &poll_accounts,
                                )
                                .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(e) => Err(e),
                                }
                            },
                            other => {
                                info!(
                                    event = "telegram.update.ignored",
                                    account_handle = aid,
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
                                let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                                let now = std::time::Instant::now();
                                s.last_update_finished_at = Some(now);
                                s.last_retryable_failure_at = None;
                                s.last_retryable_failure_reason_code = None;
                            },
                            (Err(_e), Some(reason_code)) => {
                                match record_retry_attempt(&mut attempts_by_update_id, update_id) {
                                    RetryAction::Quarantine { attempt } => {
                                        warn!(
                                            event = "telegram.update.quarantined",
                                            account_handle = aid,
                                            update_id,
                                            reason_code = "quarantined_after_retries",
                                            last_failure_reason_code = reason_code,
                                            attempts = attempt,
                                            "telegram update quarantine after retry budget exhausted"
                                        );
                                        attempts_by_update_id.remove(&update_id);
                                        offset = update.id.as_offset();
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
                                            account_handle = aid,
                                            update_id,
                                            reason_code,
                                            attempts = attempt,
                                            max_attempts = INBOUND_MAX_ATTEMPTS,
                                            "telegram update failed before retry barrier; stopping batch and retrying later"
                                        );
                                        {
                                            let mut s = poll_state
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            s.last_retryable_failure_at =
                                                Some(std::time::Instant::now());
                                            s.last_retryable_failure_reason_code =
                                                Some(reason_code);
                                        }
                                        let backoff_ms = match attempt {
                                            1 => 200,
                                            2 => 500,
                                            _ => 1000,
                                        };
                                        tokio::time::sleep(std::time::Duration::from_millis(
                                            backoff_ms,
                                        ))
                                        .await;
                                        break;
                                    },
                                }
                            },
                            (Err(_e), None) => {
                                error!(
                                    event = "telegram.update.handler_failed",
                                    account_handle = aid,
                                    update_id,
                                    reason_code = "handler_failed",
                                    "error handling telegram update"
                                );
                                attempts_by_update_id.remove(&update_id);
                                offset = update.id.as_offset();
                                let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                                let now = std::time::Instant::now();
                                s.last_update_finished_at = Some(now);
                                s.last_retryable_failure_at = None;
                                s.last_retryable_failure_reason_code = None;
                            },
                        }
                    }
                },
                Err(e) => {
                    // Detect conflict error: another bot instance is running with the same token.
                    let is_conflict =
                        matches!(&e, RequestError::Api(ApiError::TerminatedByOtherGetUpdates));

                    if is_conflict {
                        warn!(
                            account_id = aid,
                            "telegram bot disabled: another instance is already running with this token"
                        );
                        {
                            let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
                            s.polling_state = PollingState::Exited;
                            s.last_poll_exit_reason_code = Some("disabled_token_conflict");
                        }

                        // Request the gateway to disable this channel.
                        let event_sink = {
                            let accounts = poll_accounts.read().unwrap_or_else(|e| e.into_inner());
                            accounts.get(&aid).and_then(|s| s.event_sink.clone())
                        };
                        if let Some(sink) = event_sink {
                            sink.request_disable_account(
                                "telegram",
                                &aid,
                                "Another bot instance is already running with this token",
                            )
                            .await;
                        }

                        // Cancel this polling loop and exit.
                        cancel_clone.cancel();
                        break;
                    }

                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if first_failure_at.is_none() {
                        first_failure_at = Some(Instant::now());
                    }
                    let now = Instant::now();
                    let should_log = last_summary_at
                        .map(|t| now.duration_since(t) >= std::time::Duration::from_secs(60))
                        .unwrap_or(true);
                    if should_log {
                        last_summary_at = Some(now);
                        let downtime_secs =
                            first_failure_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        let reason_code = match &e {
                            RequestError::Network(_) => "network",
                            RequestError::InvalidJson { .. } => "invalid_json",
                            RequestError::RetryAfter(_) => "retry_after",
                            RequestError::Api(_) => "api",
                            RequestError::Io(_) => "io",
                            _ => "other",
                        };
                        warn!(
                            event = "telegram.polling.degraded",
                            account_handle = aid,
                            consecutive_failures,
                            downtime_secs,
                            backoff_secs = 5u64,
                            reason_code,
                            "telegram getUpdates failed"
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                },
            }
        }

        {
            let mut s = poll_state.lock().unwrap_or_else(|e| e.into_inner());
            if s.polling_state != PollingState::Exited {
                s.polling_state = PollingState::Exited;
                if s.last_poll_exit_reason_code.is_none() {
                    s.last_poll_exit_reason_code = Some("exited");
                }
            }
        }
    });

    Ok(cancel)
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
}

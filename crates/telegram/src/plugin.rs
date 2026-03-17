use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Instant,
};

use {
    anyhow::Result,
    async_trait::async_trait,
    secrecy::ExposeSecret,
    teloxide::prelude::Requester,
    tracing::{info, warn},
};

use moltis_channels::{
    ChannelEventSink,
    message_log::MessageLog,
    plugin::{ChannelHealthSnapshot, ChannelOutbound, ChannelPlugin, ChannelStatus},
};

use crate::{
    bot,
    config::{TelegramAccountConfig, TelegramBusAccountSnapshot},
    outbound::TelegramOutbound,
    state::AccountStateMap,
};

/// Cache TTL for probe results (3 seconds).
const PROBE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// Telegram channel plugin.
pub struct TelegramPlugin {
    accounts: AccountStateMap,
    outbound: TelegramOutbound,
    message_log: Option<Arc<dyn MessageLog>>,
    event_sink: Option<Arc<dyn ChannelEventSink>>,
    probe_cache: RwLock<HashMap<String, (ChannelHealthSnapshot, Instant)>>,
}

impl TelegramPlugin {
    pub fn new() -> Self {
        let accounts: AccountStateMap = Arc::new(RwLock::new(HashMap::new()));
        let outbound = TelegramOutbound {
            accounts: Arc::clone(&accounts),
        };
        Self {
            accounts,
            outbound,
            message_log: None,
            event_sink: None,
            probe_cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_message_log(mut self, log: Arc<dyn MessageLog>) -> Self {
        self.message_log = Some(log);
        self
    }

    pub fn with_event_sink(mut self, sink: Arc<dyn ChannelEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Get a shared reference to the outbound sender (for use outside the plugin).
    pub fn shared_outbound(&self) -> Arc<dyn moltis_channels::ChannelOutbound> {
        Arc::new(TelegramOutbound {
            accounts: Arc::clone(&self.accounts),
        })
    }

    pub fn account_handles(&self) -> Vec<String> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts.keys().cloned().collect()
    }

    fn invalidate_probe_cache(&self, account_handle: &str) {
        if let Ok(mut cache) = self.probe_cache.write() {
            cache.remove(account_handle);
        }
    }

    /// Get the config for a specific account (serialized to JSON).
    pub fn account_config(&self, account_handle: &str) -> Option<serde_json::Value> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_handle)
            .and_then(|s| serde_json::to_value(&s.config).ok())
    }

    /// Return safe (non-secret) identity + group-bus snapshot for all accounts.
    pub fn bus_accounts_snapshot(&self) -> Vec<TelegramBusAccountSnapshot> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .iter()
            .map(|(account_handle, s)| TelegramBusAccountSnapshot {
                account_handle: account_handle.clone(),
                chan_user_name: s.bot_username.clone(),
                relay_chain_enabled: s.config.relay_chain_enabled,
                relay_hop_limit: s.config.relay_hop_limit,
                epoch_relay_budget: s.config.epoch_relay_budget,
                relay_strictness: s.config.relay_strictness.clone(),
                group_session_transcript_format: s.config.group_session_transcript_format.clone(),
            })
            .collect()
    }

    /// Update the in-memory config for an account without restarting the
    /// polling loop.  Use for allowlist changes that don't need
    /// re-authentication or bot restart.
    pub fn update_account_config(
        &self,
        account_handle: &str,
        config: serde_json::Value,
    ) -> Result<()> {
        let tg_config: TelegramAccountConfig = serde_json::from_value(config)?;
        let mut accounts = self.accounts.write().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = accounts.get_mut(account_handle) {
            state.config = tg_config;
            drop(accounts);
            self.invalidate_probe_cache(account_handle);
            Ok(())
        } else {
            Err(anyhow::anyhow!("account not found: {account_handle}"))
        }
    }

    /// List pending OTP challenges for a specific account.
    pub fn pending_otp_challenges(
        &self,
        account_handle: &str,
    ) -> Vec<crate::otp::OtpChallengeInfo> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_handle)
            .map(|s| {
                let otp = s.otp.lock().unwrap_or_else(|e| e.into_inner());
                otp.list_pending()
            })
            .unwrap_or_default()
    }
}

impl Default for TelegramPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for TelegramPlugin {
    fn id(&self) -> &str {
        "telegram"
    }

    fn name(&self) -> &str {
        "Telegram"
    }

    async fn start_account(
        &mut self,
        account_handle: &str,
        config: serde_json::Value,
    ) -> Result<()> {
        let tg_config: TelegramAccountConfig = serde_json::from_value(config)?;

        if tg_config.token.expose_secret().is_empty() {
            return Err(anyhow::anyhow!("telegram bot token is required"));
        }

        info!(account_handle, "starting telegram account");

        bot::start_polling(
            account_handle.to_string(),
            tg_config,
            Arc::clone(&self.accounts),
            self.message_log.clone(),
            self.event_sink.clone(),
        )
        .await?;
        self.invalidate_probe_cache(account_handle);

        Ok(())
    }

    async fn stop_account(&mut self, account_handle: &str) -> Result<()> {
        let runtime = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            accounts
                .get(account_handle)
                .map(|s| (s.cancel.clone(), std::sync::Arc::clone(&s.supervisor)))
        };

        if let Some((cancel, supervisor)) = runtime {
            info!(account_handle, "stopping telegram account");
            cancel.cancel();
            let join = {
                let mut slot = supervisor.lock().unwrap_or_else(|e| e.into_inner());
                slot.take()
            };
            if let Some(join) = join {
                if let Err(err) = join.await {
                    warn!(account_handle, "telegram supervisor join failed: {err}");
                }
            }
            {
                let mut accounts = self.accounts.write().unwrap_or_else(|e| e.into_inner());
                accounts.remove(account_handle);
            }
            self.invalidate_probe_cache(account_handle);
        } else {
            warn!(account_handle, "telegram account not found");
        }

        Ok(())
    }

    fn outbound(&self) -> Option<&dyn ChannelOutbound> {
        Some(&self.outbound)
    }

    fn status(&self) -> Option<&dyn ChannelStatus> {
        Some(self)
    }
}

#[async_trait]
impl ChannelStatus for TelegramPlugin {
    async fn probe(&self, account_handle: &str) -> Result<ChannelHealthSnapshot> {
        // Return cached result if fresh enough.
        if let Ok(cache) = self.probe_cache.read()
            && let Some((snap, ts)) = cache.get(account_handle)
            && ts.elapsed() < PROBE_CACHE_TTL
        {
            return Ok(snap.clone());
        }

        let bot_and_polling = {
            let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
            accounts
                .get(account_handle)
                .map(|s| (s.bot.clone(), Arc::clone(&s.polling)))
        };

        let result = match bot_and_polling {
            Some((bot, polling)) => {
                let auth_ok = bot.get_me().await.is_ok();
                let now = std::time::Instant::now();
                let (connected, details) = {
                    let s = polling.lock().unwrap_or_else(|e| e.into_inner());
                    compute_polling_probe_details(now, auth_ok, &s)
                };
                ChannelHealthSnapshot {
                    connected,
                    chan_account_key: account_handle.to_string(),
                    details: Some(details),
                }
            },
            None => ChannelHealthSnapshot {
                connected: false,
                chan_account_key: account_handle.to_string(),
                details: Some(
                    "runtime_state=stopped_by_operator reason_code=account_not_started backoff_secs=0 last_poll_ok_secs_ago=None last_retryable_failure_reason_code=None blocked_by_update_retry=false"
                        .into(),
                ),
            },
        };

        if let Ok(mut cache) = self.probe_cache.write() {
            cache.insert(account_handle.to_string(), (result.clone(), Instant::now()));
        }

        Ok(result)
    }
}

fn compute_polling_probe_details(
    now: std::time::Instant,
    auth_ok: bool,
    s: &crate::state::PollingRuntimeState,
) -> (bool, String) {
    let stale = s
        .last_poll_ok_at
        .map(|last_poll| now.duration_since(last_poll).as_secs() > s.stale_threshold_secs)
        .unwrap_or(true);
    let last_poll_ok_secs_ago = s.last_poll_ok_at.map(|t| now.duration_since(t).as_secs());
    let update_processing_blocked = match (s.last_retryable_failure_at, s.last_update_finished_at) {
        (Some(_failed_at), None) => true,
        (Some(failed_at), Some(finished_at)) => finished_at < failed_at,
        (None, _) => false,
    };
    let runtime_state = s.polling_state.as_str();
    let reason_code = s
        .current_reason_code
        .or(s.last_poll_exit_reason_code)
        .unwrap_or("none");
    let connected = auth_ok
        && runtime_state == "running"
        && s.last_poll_ok_at.is_some()
        && !stale
        && !update_processing_blocked;
    let details = format!(
        "runtime_state={runtime_state} reason_code={reason_code} backoff_secs={} last_poll_ok_secs_ago={:?} last_retryable_failure_reason_code={:?} blocked_by_update_retry={}",
        s.current_backoff_secs,
        last_poll_ok_secs_ago,
        s.last_retryable_failure_reason_code,
        update_processing_blocked
    );
    (connected, details)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{otp::OtpState, outbound::TelegramOutbound, state::AccountState},
        moltis_channels::gating::DmPolicy,
        secrecy::{ExposeSecret, Secret},
        tokio_util::sync::CancellationToken,
    };

    /// Build a minimal `AccountState` for unit tests (no network calls).
    fn test_account_state(accounts: &AccountStateMap, cancel: CancellationToken) -> AccountState {
        AccountState {
            bot: teloxide::Bot::new("test:fake_token_for_unit_tests"),
            bot_user_id: None,
            bot_username: Some("test_bot".into()),
            account_handle: "telegram:test".into(),
            config: TelegramAccountConfig {
                token: Secret::new("test:fake_token_for_unit_tests".into()),
                ..Default::default()
            },
            outbound: Arc::new(TelegramOutbound {
                accounts: Arc::clone(accounts),
            }),
            cancel,
            supervisor: Arc::new(std::sync::Mutex::new(None)),
            message_log: None,
            event_sink: None,
            polling: Arc::new(std::sync::Mutex::new(
                crate::state::PollingRuntimeState::new(90),
            )),
            otp: std::sync::Mutex::new(OtpState::new(300)),
        }
    }

    #[test]
    fn update_account_config_updates_allowlist() {
        let plugin = TelegramPlugin::new();
        let cancel = CancellationToken::new();
        {
            let mut map = plugin.accounts.write().unwrap();
            map.insert("test".into(), test_account_state(&plugin.accounts, cancel));
        }

        // Initially empty allowlist.
        {
            let map = plugin.accounts.read().unwrap();
            assert!(map.get("test").unwrap().config.allowlist.is_empty());
        }

        // Update config with a populated allowlist.
        let new_config = serde_json::json!({
            "token": "test:fake_token_for_unit_tests",
            "dm_policy": "allowlist",
            "allowlist": ["alice", "bob"],
        });
        plugin.update_account_config("test", new_config).unwrap();

        // Verify the change is immediately visible.
        let map = plugin.accounts.read().unwrap();
        let state = map.get("test").unwrap();
        assert_eq!(state.config.dm_policy, DmPolicy::Allowlist);
        assert_eq!(state.config.allowlist, vec!["alice", "bob"]);
    }

    /// Security: `update_account_config` must NOT cancel the polling
    /// CancellationToken.  Cancelling it restarts the bot polling loop with
    /// offset 0, causing Telegram to re-deliver the OTP code message.  The
    /// re-delivered message would pass access control (user is now on the
    /// allowlist) and get forwarded to the LLM.
    #[test]
    fn security_update_config_does_not_cancel_polling() {
        let plugin = TelegramPlugin::new();
        let cancel = CancellationToken::new();
        let cancel_witness = cancel.clone();

        {
            let mut map = plugin.accounts.write().unwrap();
            map.insert("test".into(), test_account_state(&plugin.accounts, cancel));
        }

        let new_config = serde_json::json!({
            "token": "test:fake_token_for_unit_tests",
            "allowlist": ["new_user"],
        });
        plugin.update_account_config("test", new_config).unwrap();

        assert!(
            !cancel_witness.is_cancelled(),
            "update_account_config must NOT cancel the polling token — \
             cancelling restarts the bot and causes Telegram to re-deliver messages"
        );
    }

    /// Security: after a hot config update, the access control check must
    /// immediately reflect the new allowlist.  This simulates the exact
    /// sequence that happens during OTP self-approval.
    #[test]
    fn security_config_update_immediately_affects_access_control() {
        use {crate::access, moltis_common::types::ChatType};

        let plugin = TelegramPlugin::new();
        let cancel = CancellationToken::new();
        {
            let mut map = plugin.accounts.write().unwrap();
            let mut state = test_account_state(&plugin.accounts, cancel);
            state.config.dm_policy = DmPolicy::Allowlist;
            state.config.allowlist = vec![];
            map.insert("test".into(), state);
        }

        // Before approval: user is denied.
        {
            let map = plugin.accounts.read().unwrap();
            let config = &map.get("test").unwrap().config;
            assert!(
                access::check_access(config, &ChatType::Dm, "12345", Some("alice"), None, false)
                    .is_err()
            );
        }

        // OTP approval adds user to allowlist via update_account_config.
        let new_config = serde_json::json!({
            "token": "test:fake_token_for_unit_tests",
            "dm_policy": "allowlist",
            "allowlist": ["alice"],
        });
        plugin.update_account_config("test", new_config).unwrap();

        // After approval: user is allowed.
        {
            let map = plugin.accounts.read().unwrap();
            let config = &map.get("test").unwrap().config;
            assert!(
                access::check_access(config, &ChatType::Dm, "12345", Some("alice"), None, false)
                    .is_ok(),
                "approved user must pass access control immediately after config update"
            );
        }
    }

    #[test]
    fn update_account_config_nonexistent_account_errors() {
        let plugin = TelegramPlugin::new();
        let result = plugin.update_account_config("nonexistent", serde_json::json!({"token": "t"}));
        assert!(result.is_err());
    }

    #[test]
    fn update_account_config_preserves_otp_state() {
        let plugin = TelegramPlugin::new();
        let cancel = CancellationToken::new();
        {
            let mut map = plugin.accounts.write().unwrap();
            map.insert("test".into(), test_account_state(&plugin.accounts, cancel));
        }

        // Create a pending OTP challenge.
        {
            let map = plugin.accounts.read().unwrap();
            let state = map.get("test").unwrap();
            let mut otp = state.otp.lock().unwrap();
            otp.initiate("12345", Some("alice".into()), None);
        }

        // Update config.
        let new_config = serde_json::json!({
            "token": "test:fake_token_for_unit_tests",
            "allowlist": ["alice"],
        });
        plugin.update_account_config("test", new_config).unwrap();

        // OTP challenge must still be pending (state was not wiped).
        let map = plugin.accounts.read().unwrap();
        let state = map.get("test").unwrap();
        let otp = state.otp.lock().unwrap();
        assert!(
            otp.has_pending("12345"),
            "config update must preserve in-flight OTP challenges"
        );
    }

    #[test]
    fn update_account_config_preserves_bot_token() {
        let plugin = TelegramPlugin::new();
        let cancel = CancellationToken::new();
        {
            let mut map = plugin.accounts.write().unwrap();
            map.insert("test".into(), test_account_state(&plugin.accounts, cancel));
        }

        // Update config with a new allowlist but same token.
        let new_config = serde_json::json!({
            "token": "test:fake_token_for_unit_tests",
            "allowlist": ["alice"],
        });
        plugin.update_account_config("test", new_config).unwrap();

        // Bot instance itself is untouched (same object in memory).
        let map = plugin.accounts.read().unwrap();
        let state = map.get("test").unwrap();
        assert_eq!(
            state.config.token.expose_secret(),
            "test:fake_token_for_unit_tests"
        );
    }

    #[test]
    fn probe_connected_reflects_polling_liveness_not_just_auth() {
        let now = std::time::Instant::now();
        let mut s = crate::state::PollingRuntimeState::new(90);
        s.polling_state = crate::state::PollingState::Running;
        s.polling_started_at = now - std::time::Duration::from_secs(200);
        s.current_reason_code = None;
        s.current_backoff_secs = 0;

        let (connected0, details0) = super::compute_polling_probe_details(now, true, &s);
        assert!(
            !connected0,
            "polling with no successful poll yet must report disconnected"
        );
        assert!(details0.contains("runtime_state=running"));
        assert!(details0.contains("reason_code=none"));

        s.last_poll_ok_at = Some(now - std::time::Duration::from_secs(200));
        let (connected, details) = super::compute_polling_probe_details(now, true, &s);
        assert!(!connected, "stale polling must report disconnected");
        assert!(details.contains("blocked_by_update_retry=false"));

        s.last_poll_ok_at = Some(now);
        let (connected2, details2) = super::compute_polling_probe_details(now, true, &s);
        assert!(connected2, "fresh polling should report connected");
        assert!(details2.contains("runtime_state=running"));
        assert!(details2.contains("blocked_by_update_retry=false"));

        let (connected3, _details3) = super::compute_polling_probe_details(now, false, &s);
        assert!(
            !connected3,
            "auth_ok=false must report disconnected even if polling is fresh"
        );
    }

    #[test]
    fn probe_disconnects_when_retryable_failure_is_newer_than_last_completed_update() {
        let now = std::time::Instant::now();
        let mut s = crate::state::PollingRuntimeState::new(90);
        s.polling_state = crate::state::PollingState::Running;
        s.last_poll_ok_at = Some(now);
        s.last_update_finished_at = Some(now - std::time::Duration::from_secs(30));
        s.last_retryable_failure_at = Some(now - std::time::Duration::from_secs(5));
        s.last_retryable_failure_reason_code = Some("get_file_failed");
        s.current_reason_code = None;

        let (connected, details) = super::compute_polling_probe_details(now, true, &s);
        assert!(
            !connected,
            "fresh polls must still report disconnected while a retry barrier is blocking updates"
        );
        assert!(details.contains("blocked_by_update_retry=true"));
        assert!(details.contains("last_retryable_failure_reason_code=Some(\"get_file_failed\")"));

        s.last_update_finished_at = Some(now);
        let (connected_after_recovery, details_after_recovery) =
            super::compute_polling_probe_details(now, true, &s);
        assert!(
            connected_after_recovery,
            "completed update progress after the retryable failure should restore connectivity"
        );
        assert!(details_after_recovery.contains("blocked_by_update_retry=false"));
    }

    #[test]
    fn probe_details_order_starts_with_runtime_reason_and_backoff() {
        let now = std::time::Instant::now();
        let mut s = crate::state::PollingRuntimeState::new(90);
        s.current_reason_code = Some("network");
        s.current_backoff_secs = 5;

        let (_connected, details) = super::compute_polling_probe_details(now, false, &s);
        assert!(details.starts_with(
            "runtime_state=reconnecting reason_code=network backoff_secs=5 last_poll_ok_secs_ago=None"
        ));
    }
}

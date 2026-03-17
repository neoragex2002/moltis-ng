use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use moltis_channels::{ChannelEventSink, message_log::MessageLog};

use crate::{config::TelegramAccountConfig, otp::OtpState, outbound::TelegramOutbound};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollingState {
    Running,
    Reconnecting,
    StoppedByOperator,
}

impl PollingState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Reconnecting => "reconnecting",
            Self::StoppedByOperator => "stopped_by_operator",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PollingRuntimeState {
    pub polling_state: PollingState,
    pub polling_started_at: std::time::Instant,
    pub last_poll_ok_at: Option<std::time::Instant>,
    pub last_update_finished_at: Option<std::time::Instant>,
    pub last_retryable_failure_at: Option<std::time::Instant>,
    pub last_retryable_failure_reason_code: Option<&'static str>,
    pub last_poll_exit_reason_code: Option<&'static str>,
    pub current_reason_code: Option<&'static str>,
    pub current_backoff_secs: u64,
    pub next_update_offset: i32,
    pub stale_threshold_secs: u64,
}

impl PollingRuntimeState {
    pub fn new(stale_threshold_secs: u64) -> Self {
        Self {
            polling_state: PollingState::Reconnecting,
            polling_started_at: std::time::Instant::now(),
            last_poll_ok_at: None,
            last_update_finished_at: None,
            last_retryable_failure_at: None,
            last_retryable_failure_reason_code: None,
            last_poll_exit_reason_code: None,
            current_reason_code: Some("startup"),
            current_backoff_secs: 0,
            next_update_offset: 0,
            stale_threshold_secs,
        }
    }
}

/// Shared account state map.
pub type AccountStateMap = Arc<RwLock<HashMap<String, AccountState>>>;

/// Per-account runtime state.
pub struct AccountState {
    pub bot: teloxide::Bot,
    /// Bot user id returned by `get_me()` (used for stable reply-to-bot checks).
    pub bot_user_id: Option<teloxide::types::UserId>,
    pub bot_username: Option<String>,
    pub account_handle: String,
    pub config: TelegramAccountConfig,
    pub outbound: Arc<TelegramOutbound>,
    pub cancel: CancellationToken,
    pub supervisor: Arc<Mutex<Option<JoinHandle<()>>>>,
    pub message_log: Option<Arc<dyn MessageLog>>,
    pub event_sink: Option<Arc<dyn ChannelEventSink>>,
    pub polling: Arc<Mutex<PollingRuntimeState>>,
    /// In-memory OTP challenges for self-approval (std::sync::Mutex because
    /// all OTP operations are synchronous HashMap lookups, never held across
    /// `.await` points).
    pub otp: Mutex<OtpState>,
}

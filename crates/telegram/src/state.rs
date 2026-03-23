use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::Instant,
};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use moltis_channels::{ChannelEventSink, message_log::MessageLog};

use crate::{
    adapter::TelegramCoreBridge,
    config::{TelegramAccountConfig, TelegramIdentityLink},
    otp::OtpState,
    outbound::TelegramOutbound,
};

struct GroupRuntimeDedupeEntry {
    inserted_at: Instant,
}

#[derive(Default)]
struct GroupRuntimeDedupeCache {
    entries: HashMap<String, GroupRuntimeDedupeEntry>,
}

impl GroupRuntimeDedupeCache {
    const TTL: std::time::Duration = std::time::Duration::from_secs(600);
    const MAX_ENTRIES: usize = 8192;

    fn check_and_insert(&mut self, key: &str) -> bool {
        self.evict_expired();
        if self.entries.contains_key(key) {
            return true;
        }
        if self.entries.len() >= Self::MAX_ENTRIES
            && let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, value)| value.inserted_at)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest_key);
        }
        self.entries.insert(
            key.to_string(),
            GroupRuntimeDedupeEntry {
                inserted_at: Instant::now(),
            },
        );
        false
    }

    fn evict_expired(&mut self) {
        let cutoff = Instant::now() - Self::TTL;
        self.entries.retain(|_, value| value.inserted_at > cutoff);
    }
}

struct GroupRuntimeAuthorEntry {
    account_handle: String,
    updated_at: Instant,
}

pub struct TelegramGroupRuntime {
    participants_by_chat: HashMap<String, HashSet<String>>,
    message_authors: HashMap<(String, String), GroupRuntimeAuthorEntry>,
    dedupe: GroupRuntimeDedupeCache,
}

impl Default for TelegramGroupRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramGroupRuntime {
    const AUTHOR_TTL: std::time::Duration = std::time::Duration::from_secs(86400);
    const MAX_AUTHORS: usize = 16384;

    pub fn new() -> Self {
        Self {
            participants_by_chat: HashMap::new(),
            message_authors: HashMap::new(),
            dedupe: GroupRuntimeDedupeCache::default(),
        }
    }

    pub fn register_participant(&mut self, chat_id: &str, account_handle: &str) {
        self.participants_by_chat
            .entry(chat_id.to_string())
            .or_default()
            .insert(account_handle.to_string());
    }

    pub fn participants_for_chat(&self, chat_id: &str) -> Vec<String> {
        self.participants_by_chat
            .get(chat_id)
            .map(|participants| participants.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn register_message_author(
        &mut self,
        chat_id: &str,
        message_id: &str,
        account_handle: &str,
    ) {
        self.evict_expired_authors();
        if self.message_authors.len() >= Self::MAX_AUTHORS
            && let Some(oldest_key) = self
                .message_authors
                .iter()
                .min_by_key(|(_, value)| value.updated_at)
                .map(|(key, _)| key.clone())
        {
            self.message_authors.remove(&oldest_key);
        }
        self.message_authors.insert(
            (chat_id.to_string(), message_id.to_string()),
            GroupRuntimeAuthorEntry {
                account_handle: account_handle.to_string(),
                updated_at: Instant::now(),
            },
        );
    }

    pub fn message_author(&mut self, chat_id: &str, message_id: &str) -> Option<String> {
        self.evict_expired_authors();
        self.message_authors
            .get(&(chat_id.to_string(), message_id.to_string()))
            .map(|entry| entry.account_handle.clone())
    }

    pub fn check_and_insert_action(&mut self, key: &str) -> bool {
        self.dedupe.check_and_insert(key)
    }

    fn evict_expired_authors(&mut self) {
        let cutoff = Instant::now() - Self::AUTHOR_TTL;
        self.message_authors
            .retain(|_, value| value.updated_at > cutoff);
    }
}

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
    pub core_bridge: Option<Arc<dyn TelegramCoreBridge>>,
    pub polling: Arc<Mutex<PollingRuntimeState>>,
    /// In-memory OTP challenges for self-approval (std::sync::Mutex because
    /// all OTP operations are synchronous HashMap lookups, never held across
    /// `.await` points).
    pub otp: Mutex<OtpState>,
}

pub fn effective_bot_user_id(state: &AccountState) -> Option<u64> {
    state
        .bot_user_id
        .map(|id| id.0)
        .or(state.config.chan_user_id)
}

pub fn effective_bot_username(state: &AccountState) -> Option<String> {
    state
        .bot_username
        .clone()
        .or_else(|| state.config.chan_user_name.clone())
}

pub fn shared_identity_links(accounts: &AccountStateMap) -> Arc<RwLock<Vec<TelegramIdentityLink>>> {
    static STORE: OnceLock<Mutex<HashMap<usize, Arc<RwLock<Vec<TelegramIdentityLink>>>>>> =
        OnceLock::new();
    let store = STORE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = Arc::as_ptr(accounts) as usize;
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        store
            .entry(key)
            .or_insert_with(|| Arc::new(RwLock::new(Vec::new()))),
    )
}

pub fn telegram_identity_links_snapshot(accounts: &AccountStateMap) -> Vec<TelegramIdentityLink> {
    shared_identity_links(accounts)
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn replace_telegram_identity_links(
    accounts: &AccountStateMap,
    links: Vec<TelegramIdentityLink>,
) {
    let shared = shared_identity_links(accounts);
    let mut guard = shared.write().unwrap_or_else(|e| e.into_inner());
    *guard = links;
}

use std::{
    collections::{BTreeSet, HashMap},
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupLocalMessageRef {
    pub account_handle: String,
    pub chat_id: String,
    pub message_id: String,
}

impl GroupLocalMessageRef {
    pub fn new(account_handle: &str, chat_id: &str, message_id: &str) -> Self {
        Self {
            account_handle: account_handle.to_string(),
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
        }
    }

    pub fn log_value(&self) -> String {
        format!(
            "{}|{}|{}",
            self.account_handle, self.chat_id, self.message_id
        )
    }
}

struct GroupRuntimeAuthorBindingEntry {
    managed_author_account_handle: String,
    updated_at: Instant,
}

struct GroupChatRuntime {
    author_bindings: HashMap<String, GroupRuntimeAuthorBindingEntry>,
    dedupe: GroupRuntimeDedupeCache,
    updated_at: Instant,
}

impl GroupChatRuntime {
    const AUTHOR_BINDING_TTL: std::time::Duration = std::time::Duration::from_secs(86400);
    const MAX_AUTHOR_BINDINGS: usize = 16384;

    fn new(now: Instant) -> Self {
        Self {
            author_bindings: HashMap::new(),
            dedupe: GroupRuntimeDedupeCache::default(),
            updated_at: now,
        }
    }

    fn touch(&mut self, now: Instant) {
        self.updated_at = now;
    }

    fn evict_expired(&mut self, now: Instant) {
        let author_cutoff = now - Self::AUTHOR_BINDING_TTL;
        self.author_bindings
            .retain(|_, value| value.updated_at > author_cutoff);

        self.dedupe.evict_expired();
    }

    fn author_binding_key(account_handle: &str, message_id: &str) -> String {
        format!("{account_handle}|{message_id}")
    }

    fn insert_author_binding(
        &mut self,
        account_handle: &str,
        message_id: &str,
        managed_author_account_handle: &str,
        now: Instant,
    ) {
        self.evict_expired(now);
        if self.author_bindings.len() >= Self::MAX_AUTHOR_BINDINGS
            && let Some(oldest_key) = self
                .author_bindings
                .iter()
                .min_by_key(|(_, value)| value.updated_at)
                .map(|(key, _)| key.clone())
        {
            self.author_bindings.remove(&oldest_key);
        }
        self.author_bindings.insert(
            Self::author_binding_key(account_handle, message_id),
            GroupRuntimeAuthorBindingEntry {
                managed_author_account_handle: managed_author_account_handle.to_string(),
                updated_at: now,
            },
        );
        self.touch(now);
    }

    fn message_author(
        &mut self,
        account_handle: &str,
        message_id: &str,
        now: Instant,
    ) -> Option<String> {
        self.evict_expired(now);
        let author = self
            .author_bindings
            .get_mut(&Self::author_binding_key(account_handle, message_id))
            .map(|binding| {
                binding.updated_at = now;
                binding.managed_author_account_handle.clone()
            })?;
        self.touch(now);
        Some(author)
    }
}

pub struct TelegramGroupRuntime {
    participants: HashMap<String, BTreeSet<String>>,
    chats: HashMap<String, GroupChatRuntime>,
}

impl Default for TelegramGroupRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramGroupRuntime {
    const MAX_CHATS: usize = 2048;

    pub fn new() -> Self {
        Self {
            participants: HashMap::new(),
            chats: HashMap::new(),
        }
    }

    pub fn register_participant(&mut self, chat_id: &str, account_handle: &str) {
        self.participants
            .entry(chat_id.to_string())
            .or_default()
            .insert(account_handle.to_string());
    }

    pub fn participants_for_chat(&mut self, chat_id: &str) -> Vec<String> {
        self.participants
            .get(chat_id)
            .map(|participants| participants.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn register_message_author(
        &mut self,
        observer_account_handle: &str,
        chat_id: &str,
        message_ids: &[String],
        managed_author_account_handle: &str,
    ) -> bool {
        if message_ids.is_empty() {
            return false;
        }
        let now = Instant::now();
        let chat = self.chat_mut(chat_id, now);
        for message_id in message_ids {
            chat.insert_author_binding(
                observer_account_handle,
                message_id,
                managed_author_account_handle,
                now,
            );
        }
        true
    }

    pub fn message_author(
        &mut self,
        observer_account_handle: &str,
        chat_id: &str,
        message_id: &str,
    ) -> Option<String> {
        let now = Instant::now();
        self.evict_expired_chats(now);
        let chat = self.chats.get_mut(chat_id)?;
        chat.evict_expired(now);
        chat.message_author(observer_account_handle, message_id, now)
    }

    pub fn check_and_insert_action(&mut self, chat_id: &str, key: &str) -> bool {
        let now = Instant::now();
        let chat = self.chat_mut(chat_id, now);
        chat.dedupe.check_and_insert(key)
    }

    fn chat_mut(&mut self, chat_id: &str, now: Instant) -> &mut GroupChatRuntime {
        self.evict_expired_chats(now);
        if self.chats.len() >= Self::MAX_CHATS
            && !self.chats.contains_key(chat_id)
            && let Some(oldest_key) = self
                .chats
                .iter()
                .min_by_key(|(_, value)| value.updated_at)
                .map(|(key, _)| key.clone())
        {
            self.chats.remove(&oldest_key);
        }
        let chat = self
            .chats
            .entry(chat_id.to_string())
            .or_insert_with(|| GroupChatRuntime::new(now));
        chat.evict_expired(now);
        chat.touch(now);
        chat
    }

    fn evict_expired_chats(&mut self, now: Instant) {
        let _ = now;
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

pub fn shared_group_runtime(accounts: &AccountStateMap) -> Arc<Mutex<TelegramGroupRuntime>> {
    static STORE: OnceLock<Mutex<HashMap<usize, Arc<Mutex<TelegramGroupRuntime>>>>> =
        OnceLock::new();
    let store = STORE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = Arc::as_ptr(accounts) as usize;
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        store
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(TelegramGroupRuntime::new()))),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participants_for_chat_are_stably_sorted() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.register_participant("chat-1", "bot-c");
        runtime.register_participant("chat-1", "bot-a");
        runtime.register_participant("chat-1", "bot-b");

        assert_eq!(
            runtime.participants_for_chat("chat-1"),
            vec![
                "bot-a".to_string(),
                "bot-b".to_string(),
                "bot-c".to_string()
            ]
        );
    }

    #[test]
    fn author_bindings_are_scoped_by_local_message_ref() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.register_message_author(
            "telegram:100",
            "chat-1",
            &["m100".to_string()],
            "telegram:source-a",
        );
        runtime.register_message_author(
            "telegram:200",
            "chat-1",
            &["m100".to_string()],
            "telegram:source-b",
        );

        assert_eq!(
            runtime.message_author("telegram:100", "chat-1", "m100"),
            Some("telegram:source-a".to_string())
        );
        assert_eq!(
            runtime.message_author("telegram:200", "chat-1", "m100"),
            Some("telegram:source-b".to_string())
        );
        assert_eq!(
            runtime.message_author("telegram:300", "chat-1", "m100"),
            None
        );
    }

    #[test]
    fn namespaced_dedupe_separates_outbound_plan_then_inbound_collision() {
        let mut runtime = TelegramGroupRuntime::new();

        let outbound_key = "telegram.group.outbound_plan|source:telegram:100|target:telegram:200|chat:chat-1|message:1953";
        let inbound_key = "telegram.group.inbound|observer:telegram:200|chat:chat-1|message:1953";

        assert!(!runtime.check_and_insert_action("chat-1", outbound_key));
        assert!(!runtime.check_and_insert_action("chat-1", inbound_key));
        assert!(runtime.check_and_insert_action("chat-1", outbound_key));
        assert!(runtime.check_and_insert_action("chat-1", inbound_key));
    }

    #[test]
    fn namespaced_dedupe_separates_inbound_then_outbound_plan_collision() {
        let mut runtime = TelegramGroupRuntime::new();
        let inbound_key = "telegram.group.inbound|observer:telegram:200|chat:chat-1|message:1955";
        let outbound_key = "telegram.group.outbound_plan|source:telegram:100|target:telegram:200|chat:chat-1|message:1955";

        assert!(!runtime.check_and_insert_action("chat-1", inbound_key));
        assert!(!runtime.check_and_insert_action("chat-1", outbound_key));
        assert!(runtime.check_and_insert_action("chat-1", inbound_key));
        assert!(runtime.check_and_insert_action("chat-1", outbound_key));
    }

    #[test]
    fn chat_inactivity_does_not_drop_participants() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.register_participant("chat-1", "bot-a");
        runtime.register_participant("chat-1", "bot-b");
        runtime.register_message_author("telegram:100", "chat-1", &["m-root".to_string()], "bot-a");

        let cutoff = Instant::now()
            - std::time::Duration::from_secs(86400)
            - std::time::Duration::from_secs(1);
        runtime.chats.get_mut("chat-1").unwrap().updated_at = cutoff;

        assert_eq!(
            runtime.participants_for_chat("chat-1"),
            vec!["bot-a".to_string(), "bot-b".to_string()]
        );
    }

    #[test]
    fn chat_capacity_eviction_does_not_drop_participants() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.register_participant("chat-1", "bot-a");
        runtime.register_message_author("telegram:100", "chat-1", &["m-root".to_string()], "bot-a");

        for index in 0..TelegramGroupRuntime::MAX_CHATS {
            let chat_id = format!("chat-fill-{index}");
            let message_id = format!("m-{index}");
            runtime.register_message_author(
                "telegram:fill",
                &chat_id,
                &[message_id],
                "telegram:fill",
            );
        }

        assert_eq!(
            runtime.participants_for_chat("chat-1"),
            vec!["bot-a".to_string()]
        );
    }
}

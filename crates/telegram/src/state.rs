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
pub struct GroupMessageContextSnapshot {
    pub managed_author_account_handle: Option<String>,
    pub root_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRootBudgetSnapshot {
    pub used: u32,
    pub budget: u32,
    pub warned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDispatchAdmission {
    pub managed_author_account_handle: Option<String>,
    pub root_message_id: String,
    pub allowed: bool,
    pub used: u32,
    pub budget: u32,
    pub first_budget_exceeded: bool,
}

struct GroupRuntimeMessageContextEntry {
    managed_author_account_handle: Option<String>,
    root_message_id: String,
    updated_at: Instant,
}

struct GroupRuntimeRootBudgetEntry {
    used: u32,
    budget: u32,
    warned: bool,
    updated_at: Instant,
}

struct GroupChatRuntime {
    message_contexts: HashMap<String, GroupRuntimeMessageContextEntry>,
    root_budgets: HashMap<String, GroupRuntimeRootBudgetEntry>,
    dedupe: GroupRuntimeDedupeCache,
    updated_at: Instant,
}

impl GroupChatRuntime {
    const MESSAGE_CONTEXT_TTL: std::time::Duration = std::time::Duration::from_secs(86400);
    const ROOT_BUDGET_TTL: std::time::Duration = std::time::Duration::from_secs(86400);
    const MAX_MESSAGE_CONTEXTS: usize = 16384;
    const MAX_ROOT_BUDGETS: usize = 4096;

    fn new(now: Instant) -> Self {
        Self {
            message_contexts: HashMap::new(),
            root_budgets: HashMap::new(),
            dedupe: GroupRuntimeDedupeCache::default(),
            updated_at: now,
        }
    }

    fn touch(&mut self, now: Instant) {
        self.updated_at = now;
    }

    fn evict_expired(&mut self, now: Instant) {
        let message_cutoff = now - Self::MESSAGE_CONTEXT_TTL;
        self.message_contexts
            .retain(|_, value| value.updated_at > message_cutoff);

        let root_cutoff = now - Self::ROOT_BUDGET_TTL;
        self.root_budgets
            .retain(|_, value| value.updated_at > root_cutoff);

        self.dedupe.evict_expired();
    }

    fn insert_message_context(
        &mut self,
        message_id: &str,
        managed_author_account_handle: Option<String>,
        root_message_id: &str,
        now: Instant,
    ) {
        self.evict_expired(now);
        if self.message_contexts.len() >= Self::MAX_MESSAGE_CONTEXTS
            && let Some(oldest_key) = self
                .message_contexts
                .iter()
                .min_by_key(|(_, value)| value.updated_at)
                .map(|(key, _)| key.clone())
        {
            self.message_contexts.remove(&oldest_key);
        }
        self.message_contexts.insert(
            message_id.to_string(),
            GroupRuntimeMessageContextEntry {
                managed_author_account_handle,
                root_message_id: root_message_id.to_string(),
                updated_at: now,
            },
        );
        self.touch(now);
    }

    fn ensure_root_budget(&mut self, root_message_id: &str, budget: u32, now: Instant) {
        self.evict_expired(now);
        if self.root_budgets.len() >= Self::MAX_ROOT_BUDGETS
            && let Some(oldest_key) = self
                .root_budgets
                .iter()
                .min_by_key(|(_, value)| value.updated_at)
                .map(|(key, _)| key.clone())
        {
            self.root_budgets.remove(&oldest_key);
        }
        self.root_budgets
            .entry(root_message_id.to_string())
            .and_modify(|entry| entry.updated_at = now)
            .or_insert(GroupRuntimeRootBudgetEntry {
                used: 0,
                budget,
                warned: false,
                updated_at: now,
            });
        self.touch(now);
    }
}

pub struct TelegramGroupRuntime {
    bot_dispatch_cycle_budget: u32,
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
            bot_dispatch_cycle_budget: 128,
            participants: HashMap::new(),
            chats: HashMap::new(),
        }
    }

    pub fn set_bot_dispatch_cycle_budget(&mut self, budget: u32) {
        self.bot_dispatch_cycle_budget = budget;
    }

    pub fn bot_dispatch_cycle_budget(&self) -> u32 {
        self.bot_dispatch_cycle_budget
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

    pub fn ensure_external_root_dispatch(
        &mut self,
        chat_id: &str,
        root_message_id: &str,
    ) -> String {
        let now = Instant::now();
        let budget = self.bot_dispatch_cycle_budget;
        let chat = self.chat_mut(chat_id, now);
        chat.ensure_root_budget(root_message_id, budget, now);
        chat.insert_message_context(root_message_id, None, root_message_id, now);
        root_message_id.to_string()
    }

    pub fn register_sent_message_contexts(
        &mut self,
        chat_id: &str,
        message_ids: &[String],
        managed_author_account_handle: &str,
        root_message_id: &str,
    ) -> bool {
        if message_ids.is_empty() {
            return false;
        }
        let now = Instant::now();
        let chat = self.chat_mut(chat_id, now);
        if !chat.root_budgets.contains_key(root_message_id) {
            return false;
        }
        for message_id in message_ids {
            chat.insert_message_context(
                message_id,
                Some(managed_author_account_handle.to_string()),
                root_message_id,
                now,
            );
        }
        true
    }

    pub fn message_author(&mut self, chat_id: &str, message_id: &str) -> Option<String> {
        self.message_context(chat_id, message_id)
            .and_then(|context| context.managed_author_account_handle)
    }

    pub fn message_context(
        &mut self,
        chat_id: &str,
        message_id: &str,
    ) -> Option<GroupMessageContextSnapshot> {
        let now = Instant::now();
        self.evict_expired_chats(now);
        let chat = self.chats.get_mut(chat_id)?;
        chat.evict_expired(now);
        let snapshot = {
            let context = chat.message_contexts.get_mut(message_id)?;
            context.updated_at = now;
            GroupMessageContextSnapshot {
                managed_author_account_handle: context.managed_author_account_handle.clone(),
                root_message_id: context.root_message_id.clone(),
            }
        };
        chat.touch(now);
        Some(snapshot)
    }

    pub fn root_budget_snapshot(
        &mut self,
        chat_id: &str,
        root_message_id: &str,
    ) -> Option<GroupRootBudgetSnapshot> {
        let now = Instant::now();
        self.evict_expired_chats(now);
        let chat = self.chats.get_mut(chat_id)?;
        chat.evict_expired(now);
        let snapshot = {
            let budget = chat.root_budgets.get_mut(root_message_id)?;
            budget.updated_at = now;
            GroupRootBudgetSnapshot {
                used: budget.used,
                budget: budget.budget,
                warned: budget.warned,
            }
        };
        chat.touch(now);
        Some(snapshot)
    }

    pub fn inherited_root_message_id(
        &mut self,
        chat_id: &str,
        reply_to_message_id: &str,
    ) -> Option<String> {
        self.message_context(chat_id, reply_to_message_id)
            .map(|context| context.root_message_id)
    }

    pub fn admit_managed_dispatch(
        &mut self,
        chat_id: &str,
        message_id: &str,
    ) -> Option<GroupDispatchAdmission> {
        let now = Instant::now();
        self.evict_expired_chats(now);
        let chat = self.chats.get_mut(chat_id)?;
        chat.evict_expired(now);
        let (managed_author_account_handle, root_message_id) = {
            let context = chat.message_contexts.get_mut(message_id)?;
            context.updated_at = now;
            (
                context.managed_author_account_handle.clone(),
                context.root_message_id.clone(),
            )
        };
        let budget = chat.root_budgets.get_mut(&root_message_id)?;
        budget.updated_at = now;
        if budget.used < budget.budget {
            budget.used += 1;
            let admission = GroupDispatchAdmission {
                managed_author_account_handle,
                root_message_id,
                allowed: true,
                used: budget.used,
                budget: budget.budget,
                first_budget_exceeded: false,
            };
            chat.touch(now);
            return Some(admission);
        }

        let first_budget_exceeded = !budget.warned;
        budget.warned = true;
        let admission = GroupDispatchAdmission {
            managed_author_account_handle,
            root_message_id,
            allowed: false,
            used: budget.used,
            budget: budget.budget,
            first_budget_exceeded,
        };
        chat.touch(now);
        Some(admission)
    }

    pub fn check_and_insert_action(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let mut parts = key.split('|');
        let chat_segment = parts.find(|segment| segment.starts_with("chat:"));
        let Some(chat_segment) = chat_segment else {
            return false;
        };
        let chat_id = chat_segment.trim_start_matches("chat:");
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
    fn external_root_dispatch_is_lazy_and_non_charging() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.set_bot_dispatch_cycle_budget(4);

        assert!(runtime.message_context("chat-1", "m100").is_none());
        assert!(runtime.root_budget_snapshot("chat-1", "m100").is_none());

        let root_message_id = runtime.ensure_external_root_dispatch("chat-1", "m100");
        assert_eq!(root_message_id, "m100");

        let context = runtime.message_context("chat-1", "m100").unwrap();
        assert_eq!(context.root_message_id, "m100");
        assert_eq!(context.managed_author_account_handle, None);

        let budget = runtime.root_budget_snapshot("chat-1", "m100").unwrap();
        assert_eq!(budget.used, 0);
        assert_eq!(budget.budget, 4);
    }

    #[test]
    fn managed_dispatch_consumes_budget_once_and_chunk_ids_share_root() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.set_bot_dispatch_cycle_budget(1);
        runtime.ensure_external_root_dispatch("chat-1", "m100");
        runtime.register_sent_message_contexts(
            "chat-1",
            &["m101".to_string(), "m102".to_string(), "m103".to_string()],
            "bot-a",
            "m100",
        );

        let chunk_context = runtime.message_context("chat-1", "m103").unwrap();
        assert_eq!(chunk_context.root_message_id, "m100");
        assert_eq!(
            chunk_context.managed_author_account_handle.as_deref(),
            Some("bot-a")
        );

        let first = runtime.admit_managed_dispatch("chat-1", "m101").unwrap();
        assert!(first.allowed);
        assert_eq!(first.root_message_id, "m100");
        assert_eq!(first.used, 1);
        assert_eq!(first.budget, 1);

        let second = runtime.admit_managed_dispatch("chat-1", "m102").unwrap();
        assert!(!second.allowed);
        assert_eq!(second.root_message_id, "m100");
        assert_eq!(second.used, 1);
        assert_eq!(second.budget, 1);
    }

    #[test]
    fn budget_exhaustion_only_marks_first_overflow_once() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.set_bot_dispatch_cycle_budget(1);
        runtime.ensure_external_root_dispatch("chat-1", "m100");
        runtime.register_sent_message_contexts("chat-1", &["m101".to_string()], "bot-a", "m100");

        assert!(
            runtime
                .admit_managed_dispatch("chat-1", "m101")
                .unwrap()
                .allowed
        );

        let first_overflow = runtime.admit_managed_dispatch("chat-1", "m101").unwrap();
        assert!(!first_overflow.allowed);
        assert!(first_overflow.first_budget_exceeded);

        let second_overflow = runtime.admit_managed_dispatch("chat-1", "m101").unwrap();
        assert!(!second_overflow.allowed);
        assert!(!second_overflow.first_budget_exceeded);
    }

    #[test]
    fn chat_inactivity_does_not_drop_participants() {
        let mut runtime = TelegramGroupRuntime::new();
        runtime.register_participant("chat-1", "bot-a");
        runtime.register_participant("chat-1", "bot-b");
        runtime.ensure_external_root_dispatch("chat-1", "m-root");

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
        runtime.ensure_external_root_dispatch("chat-1", "m-root");

        for index in 0..TelegramGroupRuntime::MAX_CHATS {
            let chat_id = format!("chat-fill-{index}");
            let message_id = format!("m-{index}");
            runtime.ensure_external_root_dispatch(&chat_id, &message_id);
        }

        assert_eq!(
            runtime.participants_for_chat("chat-1"),
            vec!["bot-a".to_string()]
        );
    }
}

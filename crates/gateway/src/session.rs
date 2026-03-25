use std::sync::Arc;

use {
    async_trait::async_trait,
    serde_json::Value,
    tracing::{info, warn},
};

use {
    moltis_common::hooks::HookRegistry,
    moltis_projects::ProjectStore,
    moltis_sessions::{
        SessionKey,
        key::ParsedSessionKey,
        metadata::SqliteSessionMetadata,
        state_store::SessionStateStore,
        store::SessionStore,
    },
    moltis_tools::sandbox::SandboxRouter,
};

use crate::services::{ServiceResult, SessionService};

const DEFAULT_AGENT_ID: &str = "default";

/// Filter out empty assistant messages from history before sending to the UI.
///
/// Empty assistant messages are persisted in the session JSONL for LLM history
/// coherence (so the model sees a complete user→assistant turn), but they
/// should not be shown in the web UI or sent to channels.
fn filter_ui_history(messages: Vec<Value>) -> Vec<Value> {
    messages
        .into_iter()
        .filter(|msg| {
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                return true;
            }
            // Keep assistant messages that have non-empty content.
            msg.get("content")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        })
        .collect()
}

/// Extract text content from a single message Value.
fn message_text(msg: &Value) -> Option<String> {
    let text = if let Some(s) = msg.get("content").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(blocks) = msg.get("content").and_then(|v| v.as_array()) {
        blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        return None;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Truncate a string to `max` chars, appending "…" if truncated.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(max)])
    }
}

/// Extract preview from a single message (used for first-message preview in chat).
pub(crate) fn extract_preview_from_value(msg: &Value) -> Option<String> {
    message_text(msg).map(|t| truncate_preview(&t, 200))
}

/// Build a preview by combining user and assistant messages until we
/// have enough text (target ~80 chars). Skips tool_result messages.
fn extract_preview(history: &[Value]) -> Option<String> {
    const TARGET: usize = 80;
    const MAX: usize = 200;

    let mut combined = String::new();
    for msg in history {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = message_text(msg) else {
            continue;
        };
        if !combined.is_empty() {
            combined.push_str(" — ");
        }
        combined.push_str(&text);
        if combined.len() >= TARGET {
            break;
        }
    }
    if combined.is_empty() {
        return None;
    }
    Some(truncate_preview(&combined, MAX))
}

pub(crate) fn sandbox_router_key_for_entry(
    entry: &moltis_sessions::metadata::SessionEntry,
) -> String {
    entry.session_key.clone()
}

fn is_main_session_key(session_key: &str) -> bool {
    matches!(
        SessionKey::parse(session_key),
        Ok(ParsedSessionKey::Agent { bucket_key, .. }) if bucket_key == "main"
    )
}

fn session_kind_for_entry(entry: &moltis_sessions::metadata::SessionEntry) -> &'static str {
    if entry.channel_binding.is_some() {
        return "channel";
    }
    match SessionKey::parse(&entry.session_key) {
        Ok(ParsedSessionKey::System { .. }) => "system",
        _ => "agent",
    }
}

fn fallback_display_name(entry: &moltis_sessions::metadata::SessionEntry) -> String {
    if let Some(label) = entry.label.as_deref().filter(|label| !label.trim().is_empty()) {
        return label.to_string();
    }

    if entry.channel_binding.is_some()
        && let Some(target) = entry
            .channel_binding
            .as_deref()
            .and_then(moltis_telegram::adapter::channel_target_from_binding)
    {
        let account_label = target.account_key.clone();
        let peer_label = if target.chat_id.starts_with('-') {
            format!("grp:{}", target.chat_id)
        } else {
            format!("dm:{}", target.chat_id)
        };
        return format!("TG {account_label} · {peer_label}");
    }

    match SessionKey::parse(&entry.session_key) {
        Ok(ParsedSessionKey::Agent { bucket_key, .. }) if bucket_key == "main" => "Main".into(),
        Ok(ParsedSessionKey::Agent { bucket_key, .. }) if bucket_key.starts_with("chat-") => {
            "Chat".into()
        },
        Ok(ParsedSessionKey::Agent { bucket_key, .. }) => bucket_key,
        Ok(ParsedSessionKey::System { service_id, bucket_key }) => {
            if service_id == "cron" && bucket_key == "heartbeat" {
                "Heartbeat".into()
            } else if let Some(job_key) = bucket_key.strip_prefix("job-") {
                format!("Cron · {job_key}")
            } else {
                format!("{service_id} · {bucket_key}")
            }
        },
        Err(_) => entry.session_id.clone(),
    }
}

fn can_rename(entry: &moltis_sessions::metadata::SessionEntry) -> bool {
    session_kind_for_entry(entry) == "agent" && !is_main_session_key(&entry.session_key)
}

fn can_delete(entry: &moltis_sessions::metadata::SessionEntry) -> bool {
    match session_kind_for_entry(entry) {
        "channel" => true,
        "system" => false,
        _ => !is_main_session_key(&entry.session_key),
    }
}

fn can_fork(entry: &moltis_sessions::metadata::SessionEntry) -> bool {
    session_kind_for_entry(entry) != "system"
}

fn can_clear(entry: &moltis_sessions::metadata::SessionEntry) -> bool {
    session_kind_for_entry(entry) == "agent" && is_main_session_key(&entry.session_key)
}

fn session_row(
    entry: &moltis_sessions::metadata::SessionEntry,
    active_channel: bool,
) -> Value {
    let channel_target = entry
        .channel_binding
        .as_deref()
        .and_then(moltis_telegram::adapter::channel_target_from_binding);
    let channel = channel_target
        .as_ref()
        .map(|target| serde_json::json!({ "type": target.channel_type }));

    serde_json::json!({
        "id": entry.session_id,
        "sessionId": entry.session_id,
        "sessionKey": entry.session_key,
        "label": entry.label,
        "displayName": fallback_display_name(entry),
        "sessionKind": session_kind_for_entry(entry),
        "canRename": can_rename(entry),
        "canDelete": can_delete(entry),
        "canFork": can_fork(entry),
        "canClear": can_clear(entry),
        "model": entry.model,
        "createdAt": entry.created_at,
        "updatedAt": entry.updated_at,
        "messageCount": entry.message_count,
        "lastSeenMessageCount": entry.last_seen_message_count,
        "projectId": entry.project_id,
        "archived": entry.archived,
        "sandboxEnabled": entry.sandbox_enabled,
        "sandboxImage": entry.sandbox_image,
        "worktreeBranch": entry.worktree_branch,
        "channel": channel,
        "activeChannel": active_channel,
        "parentSessionId": entry.parent_session_id,
        "forkPoint": entry.fork_point,
        "mcpDisabled": entry.mcp_disabled,
        "preview": entry.preview,
        "version": entry.version,
    })
}

fn new_branch_session_ids(parent_session_key: &str) -> Result<(String, String), String> {
    let ParsedSessionKey::Agent { agent_id, .. } = SessionKey::parse(parent_session_key)
        .map_err(|_| "fork requires a canonical agent session_key".to_string())?
    else {
        return Err("fork requires an agent session, not a system session".to_string());
    };

    let opaque = uuid::Uuid::new_v4().simple().to_string();
    let session_id = format!("sess_{opaque}");
    let session_key = SessionKey::agent(&agent_id, &format!("chat-{opaque}")).0;
    Ok((session_id, session_key))
}

/// Live session service backed by JSONL store + SQLite metadata.
pub struct LiveSessionService {
    store: Arc<SessionStore>,
    metadata: Arc<SqliteSessionMetadata>,
    sandbox_router: Option<Arc<SandboxRouter>>,
    project_store: Option<Arc<dyn ProjectStore>>,
    hook_registry: Option<Arc<HookRegistry>>,
    state_store: Option<Arc<SessionStateStore>>,
    browser_service: Option<Arc<dyn crate::services::BrowserService>>,
}

impl LiveSessionService {
    pub fn new(store: Arc<SessionStore>, metadata: Arc<SqliteSessionMetadata>) -> Self {
        Self {
            store,
            metadata,
            sandbox_router: None,
            project_store: None,
            hook_registry: None,
            state_store: None,
            browser_service: None,
        }
    }

    pub fn with_sandbox_router(mut self, router: Arc<SandboxRouter>) -> Self {
        self.sandbox_router = Some(router);
        self
    }

    pub fn with_project_store(mut self, store: Arc<dyn ProjectStore>) -> Self {
        self.project_store = Some(store);
        self
    }

    pub fn with_hooks(mut self, registry: Arc<HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    pub fn with_state_store(mut self, store: Arc<SessionStateStore>) -> Self {
        self.state_store = Some(store);
        self
    }

    pub fn with_browser_service(
        mut self,
        browser: Arc<dyn crate::services::BrowserService>,
    ) -> Self {
        self.browser_service = Some(browser);
        self
    }

    async fn ensure_home_entry(&self) -> Result<moltis_sessions::metadata::SessionEntry, String> {
        let session_key = SessionKey::main(DEFAULT_AGENT_ID).0;
        if let Some(active_session_id) = self.metadata.get_active_session_id(&session_key).await
            && let Some(entry) = self.metadata.get(&active_session_id).await
        {
            return Ok(entry);
        }

        let entry = self
            .metadata
            .create(&session_key, None)
            .await
            .map_err(|e| e.to_string())?;
        self.metadata
            .set_active_session_id(&session_key, &entry.session_id)
            .await;
        self.metadata
            .get(&entry.session_id)
            .await
            .ok_or_else(|| "home session disappeared after creation".to_string())
    }
}

#[async_trait]
impl SessionService for LiveSessionService {
    async fn home(&self) -> ServiceResult {
        let entry = self.ensure_home_entry().await?;
        Ok(session_row(&entry, false))
    }

    async fn create(&self, params: Value) -> ServiceResult {
        let opaque = uuid::Uuid::new_v4().simple().to_string();
        let session_id = format!("sess_{opaque}");
        let session_key = SessionKey::agent(DEFAULT_AGENT_ID, &format!("chat-{opaque}")).0;
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from);
        self.metadata
            .upsert(&session_id, &session_key, label)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(project_id) = params
            .get("projectId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from)
        {
            self.metadata
                .set_project_id(&session_id, Some(project_id))
                .await;
        }
        let entry = self
            .metadata
            .get(&session_id)
            .await
            .ok_or_else(|| format!("created session '{session_id}' not found"))?;
        Ok(session_row(&entry, false))
    }

    async fn list(&self) -> ServiceResult {
        let all = self.metadata.list().await;

        let mut entries: Vec<Value> = Vec::with_capacity(all.len());
        for e in all {
            // Check if this session is the active one for its channel binding.
            let active_channel = if e.channel_binding.is_some() {
                self.metadata
                    .get_active_session_id(&e.session_key)
                    .await
                    .is_some_and(|active_session_id| active_session_id == e.session_id)
            } else {
                false
            };

            entries.push(session_row(&e, active_channel));
        }
        Ok(serde_json::json!(entries))
    }

    async fn preview(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let messages = self
            .store
            .read_last_n(session_id, limit)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "messages": filter_ui_history(messages) }))
    }

    async fn resolve(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;

        let entry = self
            .metadata
            .get(session_id)
            .await
            .ok_or_else(|| format!("session '{session_id}' not found"))?;
        let history = self
            .store
            .read(session_id)
            .await
            .map_err(|e| e.to_string())?;

        // Recompute preview from combined messages every time resolve runs,
        // so sessions get the latest multi-message preview algorithm.
        if !history.is_empty() {
            let new_preview = extract_preview(&history);
            if new_preview.as_deref() != entry.preview.as_deref() {
                self.metadata
                    .set_preview(session_id, new_preview.as_deref())
                    .await;
            }
        }

        Ok(serde_json::json!({
            "entry": session_row(&entry, false),
            "history": filter_ui_history(history),
        }))
    }

    async fn patch(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from);
        let model = params
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from);

        let entry = self
            .metadata
            .get(session_id)
            .await
            .ok_or_else(|| format!("session '{session_id}' not found"))?;
        if label.is_some() {
            if entry.channel_binding.is_some() {
                return Err("cannot rename a channel-bound session".to_string());
            }
            let _ = self
                .metadata
                .upsert(session_id, &entry.session_key, label)
                .await;
        }
        if model.is_some() {
            self.metadata.set_model(session_id, model).await;
        }
        if params.get("projectId").is_some() {
            let project_id = params
                .get("projectId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            self.metadata.set_project_id(session_id, project_id).await;
        }
        // Update worktree_branch if provided.
        if params.get("worktreeBranch").is_some() {
            let worktree_branch = params
                .get("worktreeBranch")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            self.metadata
                .set_worktree_branch(session_id, worktree_branch)
                .await;
        }

        // Update sandbox_image if provided.
        if params.get("sandboxImage").is_some() {
            if let Some(ref router) = self.sandbox_router {
                if router.config().scope_key != moltis_tools::sandbox::SandboxScopeKey::SessionId {
                    return Err(
                        "sandbox_image override is not supported when tools.exec.sandbox.scope_key != \"session_id\""
                            .to_string(),
                    );
                }
            }
            let sandbox_image = params
                .get("sandboxImage")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            self.metadata
                .set_sandbox_image(session_id, sandbox_image.clone())
                .await;
            // Push image override to sandbox router.
            if let Some(ref router) = self.sandbox_router {
                if let Some(ref img) = sandbox_image {
                    router.set_image_override(session_id, img.clone()).await;
                } else {
                    router.remove_image_override(session_id).await;
                }
            }
        }

        // Update mcp_disabled if provided.
        if params.get("mcpDisabled").is_some() {
            let mcp_disabled = params.get("mcpDisabled").and_then(|v| v.as_bool());
            self.metadata
                .set_mcp_disabled(session_id, mcp_disabled)
                .await;
        }

        // Update sandbox_enabled if provided.
        if params.get("sandboxEnabled").is_some() {
            let sandbox_enabled = params.get("sandboxEnabled").and_then(|v| v.as_bool());
            self.metadata
                .set_sandbox_enabled(session_id, sandbox_enabled)
                .await;
            // Push override to sandbox router.
            if let Some(ref router) = self.sandbox_router {
                if let Some(enabled) = sandbox_enabled {
                    router.set_override(session_id, enabled).await;
                } else {
                    router.remove_override(session_id).await;
                }
            }
        }

        let entry = self
            .metadata
            .get(session_id)
            .await
            .ok_or_else(|| format!("session '{session_id}' not found after update"))?;
        Ok(session_row(&entry, false))
    }

    async fn reset(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;

        self.store
            .clear(session_id)
            .await
            .map_err(|e| e.to_string())?;
        self.metadata.touch(session_id, 0).await;
        self.metadata.set_preview(session_id, None).await;

        Ok(serde_json::json!({}))
    }

    async fn delete(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;

        let deleted_entry = self.metadata.get(session_id).await;
        if deleted_entry.as_ref().is_some_and(|entry| is_main_session_key(&entry.session_key)) {
            return Err("cannot delete the main session".to_string());
        }
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Check for worktree cleanup before deleting metadata.
        if let Some(entry) = deleted_entry.as_ref()
            && entry.worktree_branch.is_some()
            && let Some(ref project_id) = entry.project_id
            && let Some(ref project_store) = self.project_store
            && let Ok(Some(project)) = project_store.get(project_id).await
        {
            let project_dir = &project.directory;
            let wt_dir = project_dir.join(".moltis-worktrees").join(session_id);

            // Safety checks unless force is set.
            if !force
                && wt_dir.exists()
                && let Ok(true) =
                    moltis_projects::WorktreeManager::has_uncommitted_changes(&wt_dir).await
            {
                return Err(
                    "worktree has uncommitted changes; use force: true to delete anyway"
                        .to_string(),
                );
            }

            // Run teardown command if configured.
            if let Some(ref cmd) = project.teardown_command
                && wt_dir.exists()
                && let Err(e) = moltis_projects::WorktreeManager::run_teardown(
                    &wt_dir,
                    cmd,
                    project_dir,
                    session_id,
                )
                .await
            {
                tracing::warn!("worktree teardown failed: {e}");
            }

            if let Err(e) = moltis_projects::WorktreeManager::cleanup(project_dir, session_id).await
            {
                tracing::warn!("worktree cleanup failed: {e}");
            }
        }

        self.store
            .clear(session_id)
            .await
            .map_err(|e| e.to_string())?;

        // Clean up sandbox resources for this session.
        let deleted_session_key = deleted_entry.as_ref().map(|entry| entry.session_key.clone());
        let deleted_effective_sandbox_key = self.sandbox_router.as_ref().and_then(|r| {
            r.effective_sandbox_key(session_id, deleted_session_key.as_deref())
                .ok()
        });
        if let Some(ref router) = self.sandbox_router {
            if router.config().scope_key == moltis_tools::sandbox::SandboxScopeKey::SessionId {
                if let Err(e) = router
                    .cleanup_session(session_id, deleted_session_key.as_deref())
                    .await
                {
                    tracing::warn!("sandbox cleanup for session {session_id}: {e}");
                }
            } else {
                // Shared scope_key: only clean up per-session override state here.
                router.cleanup_session_state(session_id).await;
            }
        }

        // Cascade-delete session state.
        if let Some(ref state_store) = self.state_store
            && let Err(e) = state_store.delete_session(session_id).await
        {
            tracing::warn!("session state cleanup for {session_id}: {e}");
        }

        self.metadata.remove(session_id).await;

        // Shared scopes: if TTL is disabled, cleanup the shared sandbox when no remaining
        // sandboxed sessions reference this effective key.
        if let Some(ref router) = self.sandbox_router
            && router.config().scope_key != moltis_tools::sandbox::SandboxScopeKey::SessionId
            && router.config().idle_ttl_secs == 0
            && let Some(ref deleted_key) = deleted_effective_sandbox_key
        {
            let remaining_entries = self.metadata.list().await;
            let mut remaining_refs = 0usize;
            for entry in remaining_entries {
                let Ok(effective_key) = router
                    .effective_sandbox_key(&entry.session_id, Some(&entry.session_key))
                else {
                    continue;
                };
                if effective_key != *deleted_key {
                    continue;
                }
                if router
                    .is_sandboxed(&entry.session_id, Some(&entry.session_key))
                    .await
                    .unwrap_or(false)
                {
                    remaining_refs += 1;
                }
            }
            if remaining_refs == 0 {
                if let Err(e) = router.cleanup_effective_key(deleted_key).await {
                    tracing::warn!(
                        effective_key = %deleted_key,
                        error = %e,
                        "sandbox cleanup for effective key failed"
                    );
                }
            }
        }

        // Dispatch SessionEnd hook (read-only).
        if let Some(ref hooks) = self.hook_registry {
            let channel_target = deleted_entry
                .as_ref()
                .and_then(|entry| entry.channel_binding.as_deref())
                .and_then(moltis_telegram::adapter::channel_target_from_binding);
            let session_key = deleted_entry
                .as_ref()
                .map(|entry| entry.session_key.clone());
            let payload = moltis_common::hooks::HookPayload::SessionEnd {
                session_id: session_id.to_string(),
                session_key,
                channel_target,
            };
            if let Err(e) = hooks.dispatch(&payload).await {
                warn!(session = %session_id, error = %e, "SessionEnd hook failed");
            }
        }

        Ok(serde_json::json!({}))
    }

    async fn compact(&self, _params: Value) -> ServiceResult {
        Ok(serde_json::json!({}))
    }

    async fn fork(&self, params: Value) -> ServiceResult {
        let parent_session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from);

        let messages = self
            .store
            .read(parent_session_id)
            .await
            .map_err(|e| e.to_string())?;
        let msg_count = messages.len();

        let fork_point = params
            .get("forkPoint")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(msg_count);

        if fork_point > msg_count {
            return Err(format!(
                "forkPoint {fork_point} exceeds message count {msg_count}"
            ));
        }

        let parent_entry = self
            .metadata
            .get(parent_session_id)
            .await
            .ok_or_else(|| format!("session '{parent_session_id}' not found"))?;
        let (new_session_id, new_session_key) = new_branch_session_ids(&parent_entry.session_key)?;
        let forked_messages: Vec<Value> = messages[..fork_point].to_vec();

        self.store
            .replace_history(&new_session_id, forked_messages)
            .await
            .map_err(|e| e.to_string())?;

        let _entry = self
            .metadata
            .upsert(&new_session_id, &new_session_key, label)
            .await
            .map_err(|e| e.to_string())?;

        self.metadata.touch(&new_session_id, fork_point as u32).await;

        // Inherit model, project, and mcp_disabled from parent.
        if parent_entry.model.is_some() {
            self.metadata
                .set_model(&new_session_id, parent_entry.model.clone())
                .await;
        }
        if parent_entry.project_id.is_some() {
            self.metadata
                .set_project_id(&new_session_id, parent_entry.project_id.clone())
                .await;
        }
        if parent_entry.mcp_disabled.is_some() {
            self.metadata
                .set_mcp_disabled(&new_session_id, parent_entry.mcp_disabled)
                .await;
        }

        // Set parent relationship.
        self.metadata
            .set_parent(
                &new_session_id,
                Some(parent_session_id.to_string()),
                Some(fork_point as u32),
            )
            .await;

        // Re-fetch after all mutations to get the final version.
        let final_entry = self
            .metadata
            .get(&new_session_id)
            .await
            .ok_or_else(|| format!("forked session '{new_session_id}' not found after creation"))?;
        Ok(serde_json::json!({
            "sessionId": final_entry.session_id,
            "sessionKey": final_entry.session_key,
            "id": final_entry.session_id,
            "label": final_entry.label,
            "forkPoint": fork_point,
            "messageCount": fork_point,
            "version": final_entry.version,
        }))
    }

    async fn branches(&self, params: Value) -> ServiceResult {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'sessionId' parameter".to_string())?;

        let children = self.metadata.list_children(session_id).await;
        let items: Vec<Value> = children
            .into_iter()
            .map(|e| session_row(&e, false))
            .collect();
        Ok(serde_json::json!(items))
    }

    async fn search(&self, params: Value) -> ServiceResult {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if query.is_empty() {
            return Ok(serde_json::json!([]));
        }

        let max = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        let results = self
            .store
            .search(query, max)
            .await
            .map_err(|e| e.to_string())?;

        let enriched: Vec<Value> = {
            let mut out = Vec::with_capacity(results.len());
            for r in results {
                let label = self
                    .metadata
                    .get(&r.session_id)
                    .await
                    .map(|e| fallback_display_name(&e));
                out.push(serde_json::json!({
                    "sessionId": r.session_id,
                    "snippet": r.snippet,
                    "role": r.role,
                    "messageIndex": r.message_index,
                    "displayName": label,
                }));
            }
            out
        };

        Ok(serde_json::json!(enriched))
    }

    async fn mark_seen(&self, key: &str) {
        self.metadata.mark_seen(key).await;
    }

    async fn clear_all(&self) -> ServiceResult {
        let all = self.metadata.list().await;
        let mut deleted = 0u32;

        for entry in &all {
            if entry.channel_binding.is_some() {
                continue;
            }
            match SessionKey::parse(&entry.session_key) {
                Ok(ParsedSessionKey::Agent { bucket_key, .. }) if bucket_key == "main" => {
                    continue;
                },
                Ok(ParsedSessionKey::System { .. }) => {
                    continue;
                },
                _ => {},
            }

            // Reuse delete logic via params.
            let params = serde_json::json!({ "sessionId": entry.session_id, "force": true });
            if let Err(e) = self.delete(params).await {
                warn!(session = %entry.session_id, error = %e, "clear_all: failed to delete session");
                continue;
            }
            deleted += 1;
        }

        // Close all browser containers since all user sessions are being cleared.
        if let Some(ref browser) = self.browser_service {
            info!("closing all browser sessions after clear_all");
            browser.close_all().await;
        }

        Ok(serde_json::json!({ "deleted": deleted }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn filter_ui_history_removes_empty_assistant_messages() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "hi there"}),
            serde_json::json!({"role": "user", "content": "run ls"}),
            // Empty assistant after tool use — should be filtered
            serde_json::json!({"role": "assistant", "content": ""}),
            serde_json::json!({"role": "user", "content": "run pwd"}),
            serde_json::json!({"role": "assistant", "content": "here is the output"}),
        ];
        let filtered = filter_ui_history(messages);
        assert_eq!(filtered.len(), 5);
        // The empty assistant message at index 3 should be gone.
        assert_eq!(filtered[2]["role"], "user");
        assert_eq!(filtered[2]["content"], "run ls");
        assert_eq!(filtered[3]["role"], "user");
        assert_eq!(filtered[3]["content"], "run pwd");
    }

    #[test]
    fn filter_ui_history_removes_whitespace_only_assistant() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "   \n  "}),
        ];
        let filtered = filter_ui_history(messages);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["role"], "user");
    }

    #[test]
    fn filter_ui_history_keeps_non_empty_assistant() {
        let messages = vec![
            serde_json::json!({"role": "assistant", "content": "real response"}),
            serde_json::json!({"role": "assistant", "content": ".", "model": "gpt-4o"}),
        ];
        let filtered = filter_ui_history(messages);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_ui_history_keeps_non_assistant_roles() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": ""}),
            serde_json::json!({"role": "tool", "tool_call_id": "x", "content": ""}),
            serde_json::json!({"role": "user", "content": ""}),
        ];
        // Non-assistant roles pass through even if content is empty.
        let filtered = filter_ui_history(messages);
        assert_eq!(filtered.len(), 3);
    }

    // --- Preview extraction tests ---

    #[test]
    fn message_text_from_string_content() {
        let msg = serde_json::json!({"role": "user", "content": "hello world"});
        assert_eq!(message_text(&msg), Some("hello world".to_string()));
    }

    #[test]
    fn message_text_from_content_blocks() {
        let msg = serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "image_url", "url": "http://example.com/img.png"},
                {"type": "text", "text": "world"}
            ]
        });
        assert_eq!(message_text(&msg), Some("hello world".to_string()));
    }

    #[test]
    fn message_text_empty_content() {
        let msg = serde_json::json!({"role": "user", "content": "  "});
        assert_eq!(message_text(&msg), None);
    }

    #[test]
    fn message_text_no_content_field() {
        let msg = serde_json::json!({"role": "user"});
        assert_eq!(message_text(&msg), None);
    }

    #[test]
    fn truncate_preview_short_string() {
        assert_eq!(truncate_preview("short", 200), "short");
    }

    #[test]
    fn truncate_preview_long_string() {
        let long = "a".repeat(250);
        let result = truncate_preview(&long, 200);
        assert!(result.ends_with('…'));
        // 200 'a' chars + the '…' char
        assert!(result.len() <= 204); // 200 bytes + up to 3 for '…'
    }

    #[test]
    fn extract_preview_from_value_basic() {
        let msg = serde_json::json!({"role": "user", "content": "tell me a joke"});
        let result = extract_preview_from_value(&msg);
        assert_eq!(result, Some("tell me a joke".to_string()));
    }

    #[test]
    fn extract_preview_single_short_message() {
        let history = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let result = extract_preview(&history);
        // Short message is still returned, just won't reach the 80-char target
        assert_eq!(result, Some("hi".to_string()));
    }

    #[test]
    fn extract_preview_combines_messages_until_target() {
        let history = vec![
            serde_json::json!({"role": "user", "content": "hi"}),
            serde_json::json!({"role": "assistant", "content": "Hello! How can I help you today?"}),
            serde_json::json!({"role": "user", "content": "Tell me about Rust programming language"}),
        ];
        let result = extract_preview(&history).expect("should produce preview");
        assert!(result.contains("hi"));
        assert!(result.contains(" — "));
        assert!(result.contains("Hello!"));
        // Should stop once target (80) is reached
        assert!(result.len() >= 30);
    }

    #[test]
    fn extract_preview_skips_system_and_tool_messages() {
        let history = vec![
            serde_json::json!({"role": "system", "content": "You are a helpful assistant."}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "tool", "content": "tool output"}),
            serde_json::json!({"role": "assistant", "content": "Hi there!"}),
        ];
        let result = extract_preview(&history).expect("should produce preview");
        // Should not contain system or tool content
        assert!(!result.contains("helpful assistant"));
        assert!(!result.contains("tool output"));
        assert!(result.contains("hello"));
        assert!(result.contains("Hi there!"));
    }

    #[test]
    fn extract_preview_empty_history() {
        let result = extract_preview(&[]);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_preview_only_system_messages() {
        let history =
            vec![serde_json::json!({"role": "system", "content": "You are a helpful assistant."})];
        let result = extract_preview(&history);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_preview_truncates_at_max() {
        // Build a very long message that exceeds MAX (200)
        let long_text = "a".repeat(300);
        let history = vec![serde_json::json!({"role": "user", "content": long_text})];
        let result = extract_preview(&history).expect("should produce preview");
        assert!(result.ends_with('…'));
        assert!(result.len() <= 204);
    }

    // --- Browser service integration tests ---

    use std::sync::atomic::{AtomicU32, Ordering};

    /// Mock browser service that tracks lifecycle method calls.
    struct MockBrowserService {
        close_all_calls: AtomicU32,
    }

    impl MockBrowserService {
        fn new() -> Self {
            Self {
                close_all_calls: AtomicU32::new(0),
            }
        }

        fn close_all_count(&self) -> u32 {
            self.close_all_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl crate::services::BrowserService for MockBrowserService {
        async fn request(&self, _p: serde_json::Value) -> crate::services::ServiceResult {
            Err("mock".into())
        }

        async fn close_all(&self) {
            self.close_all_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn sqlite_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        moltis_sessions::metadata::SqliteSessionMetadata::init(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn with_browser_service_builder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(moltis_sessions::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let pool = sqlite_pool().await;
        let metadata = Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool));

        let mock = Arc::new(MockBrowserService::new());
        let svc = LiveSessionService::new(store, metadata)
            .with_browser_service(Arc::clone(&mock) as Arc<dyn crate::services::BrowserService>);

        assert!(svc.browser_service.is_some());
    }

    #[tokio::test]
    async fn clear_all_calls_browser_close_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(moltis_sessions::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let pool = sqlite_pool().await;
        let metadata = Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool));

        let mock = Arc::new(MockBrowserService::new());
        let svc = LiveSessionService::new(store, metadata)
            .with_browser_service(Arc::clone(&mock) as Arc<dyn crate::services::BrowserService>);

        let result = svc.clear_all().await;
        assert!(result.is_ok());
        assert_eq!(mock.close_all_count(), 1, "close_all should be called once");
    }

    #[tokio::test]
    async fn clear_all_without_browser_service() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(moltis_sessions::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let pool = sqlite_pool().await;
        let metadata = Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool));

        // No browser_service wired.
        let svc = LiveSessionService::new(store, metadata);

        let result = svc.clear_all().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn home_creates_and_reuses_default_main_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(moltis_sessions::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let pool = sqlite_pool().await;
        let metadata = Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool));
        let svc = LiveSessionService::new(store, Arc::clone(&metadata));

        let first = svc.home().await.expect("home should succeed");
        let first_session_id = first["sessionId"]
            .as_str()
            .expect("home must return sessionId")
            .to_string();
        assert_eq!(first["sessionKey"], "agent:default:main");
        assert_eq!(first["displayName"], "Main");
        assert_eq!(first["sessionKind"], "agent");
        assert_eq!(first["canRename"], false);
        assert_eq!(first["canDelete"], false);
        assert_eq!(first["canFork"], true);
        assert_eq!(first["canClear"], true);
        assert_eq!(
            metadata.get_active_session_id("agent:default:main").await,
            Some(first_session_id.clone())
        );

        let second = svc.home().await.expect("home should reuse active main session");
        assert_eq!(second["sessionId"], first_session_id);
        assert_eq!(second["sessionKey"], "agent:default:main");
    }

    #[tokio::test]
    async fn create_returns_service_owned_agent_chat_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(moltis_sessions::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let pool = sqlite_pool().await;
        let metadata = Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool));
        let svc = LiveSessionService::new(store, Arc::clone(&metadata));

        let created = svc
            .create(serde_json::json!({ "label": "Scratchpad" }))
            .await
            .expect("create should succeed");
        let session_id = created["sessionId"]
            .as_str()
            .expect("create must return sessionId");
        let session_key = created["sessionKey"]
            .as_str()
            .expect("create must return sessionKey");

        assert!(session_id.starts_with("sess_"));
        assert!(session_key.starts_with("agent:default:chat-"));
        assert_eq!(created["displayName"], "Scratchpad");
        assert_eq!(created["sessionKind"], "agent");
        assert_eq!(created["canRename"], true);
        assert_eq!(created["canDelete"], true);
        assert_eq!(created["canFork"], true);

        let stored = metadata
            .get(session_id)
            .await
            .expect("created session must be persisted");
        assert_eq!(stored.session_key, session_key);
        assert_eq!(stored.label.as_deref(), Some("Scratchpad"));
    }
}

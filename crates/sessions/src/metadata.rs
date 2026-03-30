use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use {
    anyhow::Result,
    serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub session_id: String,
    pub session_key: String,
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: u32,
    #[serde(default)]
    pub last_seen_message_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_point: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default)]
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SessionMetadataFile {
    #[serde(default)]
    sessions: HashMap<String, SessionEntry>,
    #[serde(default)]
    active_sessions: HashMap<String, String>,
}

pub struct SessionMetadata {
    path: PathBuf,
    data: SessionMetadataFile,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn new_session_id() -> String {
    format!("sess_{}", uuid::Uuid::new_v4().simple())
}

impl SessionMetadata {
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            serde_json::from_str(&raw)?
        } else {
            SessionMetadataFile::default()
        };
        Ok(Self { path, data })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&self.data)?;
        fs::write(&self.path, raw)?;
        Ok(())
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionEntry> {
        self.data.sessions.get(session_id)
    }

    pub fn get_active_session_id(&self, session_key: &str) -> Option<String> {
        self.data.active_sessions.get(session_key).cloned()
    }

    pub fn set_active_session_id(&mut self, session_key: &str, session_id: &str) {
        self.data
            .active_sessions
            .insert(session_key.to_string(), session_id.to_string());
    }

    pub fn create(&mut self, session_key: &str, label: Option<String>) -> &SessionEntry {
        let session_id = new_session_id();
        self.upsert(&session_id, session_key, label)
    }

    pub fn upsert(
        &mut self,
        session_id: &str,
        session_key: &str,
        label: Option<String>,
    ) -> &SessionEntry {
        let now = now_ms();
        self.data
            .sessions
            .entry(session_id.to_string())
            .and_modify(|entry| {
                if entry.session_key != session_key {
                    entry.session_key = session_key.to_string();
                    entry.updated_at = now;
                    entry.version += 1;
                }
                if let Some(ref next_label) = label
                    && entry.label.as_deref() != Some(next_label)
                {
                    entry.label = Some(next_label.clone());
                    entry.updated_at = now;
                    entry.version += 1;
                }
            })
            .or_insert_with(|| SessionEntry {
                session_id: session_id.to_string(),
                session_key: session_key.to_string(),
                label,
                model: None,
                created_at: now,
                updated_at: now,
                message_count: 0,
                last_seen_message_count: 0,
                project_id: None,
                archived: false,
                worktree_branch: None,
                sandbox_enabled: None,
                sandbox_image: None,
                channel_binding: None,
                parent_session_id: None,
                fork_point: None,
                mcp_disabled: None,
                preview: None,
                version: 0,
            })
    }

    pub fn set_model(&mut self, session_id: &str, model: Option<String>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.model = model;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn touch(&mut self, session_id: &str, message_count: u32) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.message_count = message_count;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_preview(&mut self, session_id: &str, preview: Option<&str>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.preview = preview.map(str::to_string);
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn mark_seen(&mut self, session_id: &str) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.last_seen_message_count = entry.message_count;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_project_id(&mut self, session_id: &str, project_id: Option<String>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.project_id = project_id;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_worktree_branch(&mut self, session_id: &str, branch: Option<String>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.worktree_branch = branch;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_sandbox_image(&mut self, session_id: &str, image: Option<String>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.sandbox_image = image;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_sandbox_enabled(&mut self, session_id: &str, enabled: Option<bool>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.sandbox_enabled = enabled;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_mcp_disabled(&mut self, session_id: &str, disabled: Option<bool>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.mcp_disabled = disabled;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_channel_binding(&mut self, session_id: &str, binding: Option<String>) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.channel_binding = binding;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn set_parent(
        &mut self,
        session_id: &str,
        parent_session_id: Option<String>,
        fork_point: Option<u32>,
    ) {
        if let Some(entry) = self.data.sessions.get_mut(session_id) {
            entry.parent_session_id = parent_session_id;
            entry.fork_point = fork_point;
            entry.updated_at = now_ms();
            entry.version += 1;
        }
    }

    pub fn remove(&mut self, session_id: &str) -> Option<SessionEntry> {
        self.data.active_sessions.retain(|_, active| active != session_id);
        self.data.sessions.remove(session_id)
    }

    pub fn list(&self) -> Vec<SessionEntry> {
        let mut sessions: Vec<_> = self.data.sessions.values().cloned().collect();
        sessions.sort_by_key(|entry| entry.created_at);
        sessions
    }

    pub fn list_children(&self, parent_session_id: &str) -> Vec<SessionEntry> {
        let mut sessions: Vec<_> = self
            .data
            .sessions
            .values()
            .filter(|entry| entry.parent_session_id.as_deref() == Some(parent_session_id))
            .cloned()
            .collect();
        sessions.sort_by_key(|entry| entry.created_at);
        sessions
    }
}

pub struct SqliteSessionMetadata {
    pool: sqlx::SqlitePool,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    session_id: String,
    session_key: String,
    label: Option<String>,
    model: Option<String>,
    created_at: i64,
    updated_at: i64,
    message_count: i32,
    last_seen_message_count: i32,
    project_id: Option<String>,
    archived: i32,
    worktree_branch: Option<String>,
    sandbox_enabled: Option<i32>,
    sandbox_image: Option<String>,
    channel_binding: Option<String>,
    parent_session_id: Option<String>,
    fork_point: Option<i32>,
    mcp_disabled: Option<i32>,
    preview: Option<String>,
    version: i64,
}

impl From<SessionRow> for SessionEntry {
    fn from(row: SessionRow) -> Self {
        Self {
            session_id: row.session_id,
            session_key: row.session_key,
            label: row.label,
            model: row.model,
            created_at: row.created_at as u64,
            updated_at: row.updated_at as u64,
            message_count: row.message_count as u32,
            last_seen_message_count: row.last_seen_message_count as u32,
            project_id: row.project_id,
            archived: row.archived != 0,
            worktree_branch: row.worktree_branch,
            sandbox_enabled: row.sandbox_enabled.map(|value| value != 0),
            sandbox_image: row.sandbox_image,
            channel_binding: row.channel_binding,
            parent_session_id: row.parent_session_id,
            fork_point: row.fork_point.map(|value| value as u32),
            mcp_disabled: row.mcp_disabled.map(|value| value != 0),
            preview: row.preview,
            version: row.version as u64,
        }
    }
}

impl SqliteSessionMetadata {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    #[doc(hidden)]
    pub async fn init(pool: &sqlx::SqlitePool) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS sessions (
                session_id              TEXT    PRIMARY KEY,
                session_key             TEXT    NOT NULL,
                label                   TEXT,
                model                   TEXT,
                created_at              INTEGER NOT NULL,
                updated_at              INTEGER NOT NULL,
                message_count           INTEGER NOT NULL DEFAULT 0,
                last_seen_message_count INTEGER NOT NULL DEFAULT 0,
                project_id              TEXT,
                archived                INTEGER NOT NULL DEFAULT 0,
                worktree_branch         TEXT,
                sandbox_enabled         INTEGER,
                sandbox_image           TEXT,
                channel_binding         TEXT,
                parent_session_id       TEXT,
                fork_point              INTEGER,
                mcp_disabled            INTEGER,
                preview                 TEXT,
                version                 INTEGER NOT NULL DEFAULT 0
            )"#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at)",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_session_key ON sessions(session_key)",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_parent_session_id ON sessions(parent_session_id)",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS active_sessions (
                session_key TEXT    PRIMARY KEY,
                session_id  TEXT    NOT NULL,
                updated_at  INTEGER NOT NULL
            )"#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionEntry> {
        match sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE session_id = ?")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(row) => row.map(Into::into),
            Err(err) => {
                tracing::error!("sessions.get failed: {err}");
                None
            },
        }
    }

    pub async fn create(
        &self,
        session_key: &str,
        label: Option<String>,
    ) -> Result<SessionEntry, sqlx::Error> {
        let session_id = new_session_id();
        self.upsert(&session_id, session_key, label).await
    }

    pub async fn upsert(
        &self,
        session_id: &str,
        session_key: &str,
        label: Option<String>,
    ) -> Result<SessionEntry, sqlx::Error> {
        let now = now_ms() as i64;
        sqlx::query(
            r#"INSERT INTO sessions (session_id, session_key, label, created_at, updated_at, version)
               VALUES (?, ?, ?, ?, ?, 0)
               ON CONFLICT(session_id) DO UPDATE SET
                 session_key = excluded.session_key,
                 label = COALESCE(excluded.label, sessions.label),
                 updated_at = excluded.updated_at,
                 version = sessions.version + 1"#,
        )
        .bind(session_id)
        .bind(session_key)
        .bind(&label)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.get(session_id).await.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn set_model(&self, session_id: &str, model: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET model = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&model)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn touch(&self, session_id: &str, message_count: u32) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET message_count = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(message_count as i32)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_preview(&self, session_id: &str, preview: Option<&str>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET preview = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(preview)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn mark_seen(&self, session_id: &str) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET last_seen_message_count = message_count, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_project_id(&self, session_id: &str, project_id: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET project_id = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&project_id)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_sandbox_image(&self, session_id: &str, image: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET sandbox_image = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&image)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_sandbox_enabled(&self, session_id: &str, enabled: Option<bool>) {
        let now = now_ms() as i64;
        let value = enabled.map(|flag| flag as i32);
        sqlx::query(
            "UPDATE sessions SET sandbox_enabled = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(value)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_worktree_branch(&self, session_id: &str, branch: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET worktree_branch = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&branch)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_mcp_disabled(&self, session_id: &str, disabled: Option<bool>) {
        let now = now_ms() as i64;
        let value = disabled.map(|flag| flag as i32);
        sqlx::query(
            "UPDATE sessions SET mcp_disabled = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(value)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_channel_binding(&self, session_id: &str, binding: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET channel_binding = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&binding)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn set_parent(
        &self,
        session_id: &str,
        parent_session_id: Option<String>,
        fork_point: Option<u32>,
    ) {
        let now = now_ms() as i64;
        sqlx::query(
            "UPDATE sessions SET parent_session_id = ?, fork_point = ?, updated_at = ?, version = version + 1 WHERE session_id = ?",
        )
        .bind(&parent_session_id)
        .bind(fork_point.map(|value| value as i32))
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .ok();
    }

    pub async fn list_children(&self, parent_session_id: &str) -> Vec<SessionEntry> {
        sqlx::query_as::<_, SessionRow>(
            "SELECT * FROM sessions WHERE parent_session_id = ? ORDER BY created_at ASC",
        )
        .bind(parent_session_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
    }

    pub async fn remove(&self, session_id: &str) -> Option<SessionEntry> {
        let entry = self.get(session_id).await;
        sqlx::query("DELETE FROM active_sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        entry
    }

    pub async fn list(&self) -> Vec<SessionEntry> {
        sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions ORDER BY created_at ASC")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub async fn get_active_session_id(&self, session_key: &str) -> Option<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT session_id FROM active_sessions WHERE session_key = ?",
        )
        .bind(session_key)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn set_active_session_id(&self, session_key: &str, session_id: &str) {
        let now = now_ms() as i64;
        sqlx::query(
            r#"INSERT INTO active_sessions (session_key, session_id, updated_at)
               VALUES (?, ?, ?)
               ON CONFLICT(session_key) DO UPDATE SET
                 session_id = excluded.session_id,
                 updated_at = excluded.updated_at"#,
        )
        .bind(session_key)
        .bind(session_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .ok();
    }

    /// List session IDs for a given stable `session_key`, newest-first.
    ///
    /// This is intended for strict contract validation where `session_key`
    /// must resolve to exactly one session record.
    pub async fn list_session_ids_by_session_key(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT session_id FROM sessions WHERE session_key = ? ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(session_key)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_channel_sessions(
        &self,
        channel_type: &str,
        account_handle: &str,
        chat_id: &str,
    ) -> Vec<SessionEntry> {
        let binding_pattern = format!(
            r#"%"channel_type":"{channel_type}"%"account_handle":"{account_handle}"%"chat_id":"{chat_id}"%"#,
        );
        sqlx::query_as::<_, SessionRow>(
            "SELECT * FROM sessions WHERE channel_binding LIKE ? ORDER BY created_at ASC",
        )
        .bind(binding_pattern)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
    }

    pub async fn list_account_sessions(
        &self,
        channel_type: &str,
        account_handle: &str,
    ) -> Vec<SessionEntry> {
        let binding_pattern =
            format!(r#"%"channel_type":"{channel_type}"%"account_handle":"{account_handle}"%"#);
        sqlx::query_as::<_, SessionRow>(
            "SELECT * FROM sessions WHERE channel_binding LIKE ? ORDER BY created_at ASC",
        )
        .bind(binding_pattern)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
    }

    pub async fn list_active_sessions(
        &self,
        channel_type: &str,
        account_handle: &str,
    ) -> Vec<(String, String)> {
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for entry in self.list_account_sessions(channel_type, account_handle).await {
            if !seen.insert(entry.session_key.clone()) {
                continue;
            }
            if let Some(active_session_id) = self.get_active_session_id(&entry.session_key).await {
                rows.push((entry.session_key.clone(), active_session_id));
            }
        }
        rows
    }

    pub fn save(&self) -> Result<()> {
        Ok(())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_backend_tracks_sessions_and_active_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        let mut meta = SessionMetadata::load(path.clone()).unwrap();

        let created = meta.create("agent:zhuzhu:main", Some("Main".into())).clone();
        meta.set_active_session_id("agent:zhuzhu:main", &created.session_id);
        meta.save().unwrap();

        let reloaded = SessionMetadata::load(path).unwrap();
        let entry = reloaded.get(&created.session_id).unwrap();
        assert_eq!(entry.session_key, "agent:zhuzhu:main");
        assert_eq!(
            reloaded.get_active_session_id("agent:zhuzhu:main"),
            Some(created.session_id)
        );
    }

    async fn sqlite_meta() -> SqliteSessionMetadata {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        SqliteSessionMetadata::init(&pool).await.unwrap();
        SqliteSessionMetadata::new(pool)
    }

    #[tokio::test]
    async fn sqlite_upsert_persists_session_id_and_session_key() {
        let meta = sqlite_meta().await;
        let entry = meta
            .upsert(
                "sess_0195f3c5b3d27d8aa91e4439bb3c2e74",
                "agent:zhuzhu:main",
                Some("Main".into()),
            )
            .await
            .unwrap();

        assert_eq!(entry.session_id, "sess_0195f3c5b3d27d8aa91e4439bb3c2e74");
        assert_eq!(entry.session_key, "agent:zhuzhu:main");
        assert_eq!(entry.label.as_deref(), Some("Main"));
    }

    #[tokio::test]
    async fn sqlite_active_session_mapping_is_keyed_by_session_key() {
        let meta = sqlite_meta().await;
        meta.upsert("sess_main", "agent:zhuzhu:main", None)
            .await
            .unwrap();
        meta.set_active_session_id("agent:zhuzhu:main", "sess_main")
            .await;

        assert_eq!(
            meta.get_active_session_id("agent:zhuzhu:main").await,
            Some("sess_main".into())
        );
    }

    #[tokio::test]
    async fn sqlite_parent_relationship_uses_parent_session_id() {
        let meta = sqlite_meta().await;
        meta.upsert("sess_parent", "agent:zhuzhu:main", None)
            .await
            .unwrap();
        meta.upsert("sess_child", "agent:zhuzhu:chat-1", None)
            .await
            .unwrap();
        meta.set_parent("sess_child", Some("sess_parent".into()), Some(3))
            .await;

        let children = meta.list_children("sess_parent").await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].session_id, "sess_child");
        assert_eq!(children[0].parent_session_id.as_deref(), Some("sess_parent"));
        assert_eq!(children[0].fork_point, Some(3));
    }

    #[tokio::test]
    async fn sqlite_list_account_sessions_uses_binding_without_legacy_fallback() {
        let meta = sqlite_meta().await;
        meta.upsert("sess_a", "agent:zhuzhu:group-peer-tgchat.n100", None)
            .await
            .unwrap();
        meta.set_channel_binding(
            "sess_a",
            Some(
                r#"{"channel_type":"telegram","account_handle":"telegram:bot1","chat_id":"-100"}"#
                    .to_string(),
            ),
        )
        .await;

        let sessions = meta.list_account_sessions("telegram", "telegram:bot1").await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "sess_a");
    }

    #[tokio::test]
    async fn sqlite_remove_clears_active_session_mapping() {
        let meta = sqlite_meta().await;
        meta.upsert("sess_main", "agent:zhuzhu:main", None)
            .await
            .unwrap();
        meta.set_active_session_id("agent:zhuzhu:main", "sess_main")
            .await;

        meta.remove("sess_main").await;

        assert!(meta.get("sess_main").await.is_none());
        assert!(meta.get_active_session_id("agent:zhuzhu:main").await.is_none());
    }
}

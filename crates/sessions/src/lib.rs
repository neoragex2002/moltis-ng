//! Session storage and management.
//!
//! Sessions are stored as JSONL files (one message per line) at
//! ~/.clawdbot/agents/<agentId>/sessions/<sessionId>.jsonl
//! with file locking for concurrent access.

pub mod compaction;
pub mod key;
pub mod message;
pub mod metadata;
pub mod state_store;
pub mod store;

pub use {
    key::SessionKey,
    message::{ContentBlock, MessageContent, PersistedMessage},
    store::SearchResult,
};

/// Run database migrations for the sessions crate.
///
/// This creates the `sessions` and `channel_sessions` tables. Should be called
/// at application startup after [`moltis_projects::run_migrations`] (sessions
/// has a foreign key to projects).
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('channel_sessions')")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    if cols.iter().any(|c| c == "session_key") && !cols.iter().any(|c| c == "session_id") {
        sqlx::query("ALTER TABLE channel_sessions RENAME COLUMN session_key TO session_id")
            .execute(pool)
            .await?;
    }

    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('channel_sessions')")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    if cols.iter().any(|c| c == "account_id") && !cols.iter().any(|c| c == "account_handle") {
        sqlx::query("ALTER TABLE channel_sessions RENAME COLUMN account_id TO account_handle")
            .execute(pool)
            .await?;
    }

    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(pool)
        .await?;
    Ok(())
}

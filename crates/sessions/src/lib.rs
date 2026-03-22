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
        tracing::error!(
            event = "sessions.schema.reject_legacy_column",
            table = "channel_sessions",
            column = "session_key",
            reason_code = "legacy_schema_rejected",
            "legacy schema is no longer supported; rename channel_sessions.session_key to session_id before startup"
        );
        anyhow::bail!(
            "legacy schema rejected: channel_sessions.session_key is no longer supported; rename it to session_id before startup"
        );
    }

    if cols.iter().any(|c| c == "account_id") && !cols.iter().any(|c| c == "account_handle") {
        tracing::error!(
            event = "sessions.schema.reject_legacy_column",
            table = "channel_sessions",
            column = "account_id",
            reason_code = "legacy_schema_rejected",
            "legacy schema is no longer supported; rename channel_sessions.account_id to account_handle before startup"
        );
        anyhow::bail!(
            "legacy schema rejected: channel_sessions.account_id is no longer supported; rename it to account_handle before startup"
        );
    }

    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(pool)
        .await?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::run_migrations;

    async fn sqlite_pool() -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool")
    }

    async fn table_columns(pool: &sqlx::SqlitePool, table: &str) -> Vec<String> {
        sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .fetch_all(pool)
            .await
            .expect("table columns")
    }

    #[tokio::test]
    async fn run_migrations_rejects_legacy_channel_sessions_session_key_column() {
        let pool = sqlite_pool().await;
        sqlx::query("CREATE TABLE channel_sessions (session_key TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        let err = run_migrations(&pool)
            .await
            .expect_err("legacy session_key column should be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("channel_sessions.session_key is no longer supported"));

        let cols = table_columns(&pool, "channel_sessions").await;
        assert!(cols.iter().any(|c| c == "session_key"));
        assert!(!cols.iter().any(|c| c == "session_id"));
    }

    #[tokio::test]
    async fn run_migrations_rejects_legacy_channel_sessions_account_id_column() {
        let pool = sqlite_pool().await;
        sqlx::query("CREATE TABLE channel_sessions (account_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        let err = run_migrations(&pool)
            .await
            .expect_err("legacy account_id column should be rejected");
        let message = format!("{err:#}");
        assert!(message.contains("channel_sessions.account_id is no longer supported"));

        let cols = table_columns(&pool, "channel_sessions").await;
        assert!(cols.iter().any(|c| c == "account_id"));
        assert!(!cols.iter().any(|c| c == "account_handle"));
    }
}

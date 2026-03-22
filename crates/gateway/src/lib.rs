//! Gateway: central WebSocket/HTTP server, protocol dispatch, session/node registry.
//!
//! Lifecycle:
//! 1. Load + validate config
//! 2. Resolve auth, bind address
//! 3. Start HTTP server (health, control UI, hooks)
//! 4. Attach WebSocket upgrade handler
//! 5. Start channel accounts, cron, maintenance timers
//!
//! All domain logic (agents, channels, etc.) lives in other crates and is
//! invoked through method handlers registered in `methods.rs`.

pub mod approval;
pub mod auth;
pub mod auth_middleware;
pub mod auth_routes;
pub mod auth_webauthn;
pub mod broadcast;
pub mod channel;
pub mod channel_events;
pub mod channel_store;
pub mod chat;
pub mod chat_error;
pub mod cron;
pub mod env_routes;
pub mod ids;
#[cfg(feature = "local-llm")]
pub mod local_llm_setup;
pub mod logs;
pub mod mcp_health;
pub mod mcp_service;
pub mod message_log_store;
pub mod methods;
#[cfg(feature = "metrics")]
pub mod metrics_middleware;
#[cfg(feature = "metrics")]
pub mod metrics_routes;
pub mod nodes;
pub mod onboarding;
pub mod pairing;
pub mod people;
pub mod person;
pub mod project;
pub mod provider_setup;
#[cfg(feature = "push-notifications")]
pub mod push;
#[cfg(feature = "push-notifications")]
pub mod push_routes;
pub mod request_throttle;
pub mod run_failure;
pub mod server;
pub mod services;
pub mod session;
pub mod session_labels;
pub mod state;
#[cfg(feature = "tailscale")]
pub mod tailscale;
#[cfg(feature = "tailscale")]
pub mod tailscale_routes;
#[cfg(feature = "tls")]
pub mod tls;
pub mod tools_routes;
pub mod tts_phrases;
pub mod update_check;
pub mod upload_routes;
pub mod user;
pub mod voice;
pub mod voice_agent_tools;
pub mod ws;

#[cfg(test)]
pub(crate) mod test_support;

/// Run database migrations for the gateway crate.
///
/// This creates the auth tables (auth_password, passkeys, api_keys, auth_sessions),
/// env_variables, message_log, and channels tables. Should be called at application
/// startup after the other crate migrations (projects, sessions, cron).
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let channels_cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('channels')")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    if channels_cols.iter().any(|c| c == "account_id")
        && !channels_cols.iter().any(|c| c == "account_handle")
    {
        tracing::error!(
            event = "gateway.schema.reject_legacy_column",
            table = "channels",
            column = "account_id",
            reason_code = "legacy_schema_rejected",
            "legacy schema is no longer supported; rename channels.account_id to account_handle before startup"
        );
        anyhow::bail!(
            "legacy schema rejected: channels.account_id is no longer supported; rename it to account_handle before startup"
        );
    }

    let message_log_cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('message_log')")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    if message_log_cols.iter().any(|c| c == "account_id")
        && !message_log_cols.iter().any(|c| c == "account_handle")
    {
        tracing::error!(
            event = "gateway.schema.reject_legacy_column",
            table = "message_log",
            column = "account_id",
            reason_code = "legacy_schema_rejected",
            "legacy schema is no longer supported; rename message_log.account_id to account_handle before startup"
        );
        anyhow::bail!(
            "legacy schema rejected: message_log.account_id is no longer supported; rename it to account_handle before startup"
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
    #[tokio::test]
    async fn run_migrations_rejects_legacy_channels_account_id_column() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");
        sqlx::query("CREATE TABLE channels (account_id TEXT NOT NULL, chat_id TEXT NOT NULL)")
            .execute(&pool)
            .await
            .expect("create channels table");
        sqlx::query("CREATE TABLE message_log (account_handle TEXT, chat_id TEXT)")
            .execute(&pool)
            .await
            .expect("create message_log table");

        let err = super::run_migrations(&pool)
            .await
            .expect_err("legacy schema must be rejected");
        assert!(
            err.to_string().contains("channels.account_id"),
            "unexpected error: {err:#}"
        );
    }
}

//! SQLite-backed cron store using sqlx.

use {
    anyhow::{Context, Result},
    async_trait::async_trait,
    sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions},
};

use crate::{
    parse,
    store::CronStore,
    store_heartbeat::HeartbeatStore,
    types::{CronJob, CronRunRecord},
};

/// SQLite-backed persistence for cron jobs and run history.
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Create a new store with its own connection pool and run migrations.
    ///
    /// Use this for standalone cron databases. For shared pools (e.g., moltis.db),
    /// use [`SqliteStore::with_pool`] after calling [`crate::run_migrations`].
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("failed to connect to SQLite")?;

        crate::run_migrations(&pool).await?;

        Ok(Self { pool })
    }

    /// Create a store using an existing pool (migrations must already be run).
    ///
    /// Call [`crate::run_migrations`] before using this constructor.
    pub fn with_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CronStore for SqliteStore {
    async fn load_jobs(&self) -> Result<Vec<CronJob>> {
        let rows = sqlx::query("SELECT data FROM cron_jobs")
            .fetch_all(&self.pool)
            .await?;

        let mut jobs = Vec::with_capacity(rows.len());
        for row in rows {
            let data: String = row.get("data");
            let job: CronJob = serde_json::from_str(&data)?;
            jobs.push(job);
        }
        Ok(jobs)
    }

    async fn save_job(&self, job: &CronJob) -> Result<()> {
        let data = serde_json::to_string(job)?;
        sqlx::query(
            "INSERT INTO cron_jobs (id, data) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        )
        .bind(&job.job_id)
        .bind(&data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_job(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM cron_jobs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            anyhow::bail!("job not found: {id}");
        }
        Ok(())
    }

    async fn update_job(&self, job: &CronJob) -> Result<()> {
        let data = serde_json::to_string(job)?;
        let result = sqlx::query("UPDATE cron_jobs SET data = ? WHERE id = ?")
            .bind(&data)
            .bind(&job.job_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            anyhow::bail!("job not found: {}", job.job_id);
        }
        Ok(())
    }

    async fn append_run(&self, job_id: &str, run: &CronRunRecord) -> Result<()> {
        let status = serde_json::to_string(&run.status)?;
        let started_at_ms = parse::parse_absolute_time_ms(&run.started_at)?;
        let finished_at_ms = parse::parse_absolute_time_ms(&run.finished_at)?;
        let duration_ms = finished_at_ms.saturating_sub(started_at_ms);
        sqlx::query(
            "INSERT INTO cron_runs (run_id, job_id, started_at_ms, finished_at_ms, status, error, duration_ms, output, input_tokens, output_tokens)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.run_id)
        .bind(job_id)
        .bind(started_at_ms as i64)
        .bind(finished_at_ms as i64)
        .bind(&status)
        .bind(&run.error)
        .bind(duration_ms as i64)
        .bind(&run.output_preview)
        .bind(run.input_tokens.map(|v| v as i64))
        .bind(run.output_tokens.map(|v| v as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        let rows = sqlx::query(
            "SELECT run_id, job_id, started_at_ms, finished_at_ms, status, error, output, input_tokens, output_tokens
             FROM cron_runs
             WHERE job_id = ?
             ORDER BY started_at_ms DESC
             LIMIT ?",
        )
        .bind(job_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut runs = Vec::with_capacity(rows.len());
        for row in rows {
            let status_str: String = row.get("status");
            let status = serde_json::from_str(&status_str)?;
            let started_at_ms: u64 = row.get::<i64, _>("started_at_ms").max(0) as u64;
            let finished_at_ms: u64 = row.get::<i64, _>("finished_at_ms").max(0) as u64;
            runs.push(CronRunRecord {
                run_id: row.get("run_id"),
                job_id: row.get("job_id"),
                started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    started_at_ms as i64,
                )
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                finished_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    finished_at_ms as i64,
                )
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                status,
                error: row.get("error"),
                output_preview: row.get("output"),
                input_tokens: row
                    .try_get::<Option<i64>, _>("input_tokens")
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
                output_tokens: row
                    .try_get::<Option<i64>, _>("output_tokens")
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
            });
        }
        // Reverse so oldest first (consistent with other stores).
        runs.reverse();
        Ok(runs)
    }
}

#[async_trait]
impl HeartbeatStore for SqliteStore {
    async fn load_all(&self) -> Result<Vec<crate::types::HeartbeatStatus>> {
        let rows = sqlx::query("SELECT agent_id, config, state FROM heartbeat")
            .fetch_all(&self.pool)
            .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let config_json: String = row.get("config");
            let state_json: String = row.get("state");
            let config: crate::types::HeartbeatConfig = serde_json::from_str(&config_json)?;
            let state: crate::types::HeartbeatState = serde_json::from_str(&state_json)?;
            out.push(crate::types::HeartbeatStatus { config, state });
        }
        Ok(out)
    }

    async fn get(&self, agent_id: &str) -> Result<Option<crate::types::HeartbeatStatus>> {
        let row = sqlx::query("SELECT config, state FROM heartbeat WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let config_json: String = row.get("config");
        let state_json: String = row.get("state");
        let config: crate::types::HeartbeatConfig = serde_json::from_str(&config_json)?;
        let state: crate::types::HeartbeatState = serde_json::from_str(&state_json)?;
        Ok(Some(crate::types::HeartbeatStatus { config, state }))
    }

    async fn upsert(&self, status: &crate::types::HeartbeatStatus) -> Result<()> {
        let config = serde_json::to_string(&status.config)?;
        let state = serde_json::to_string(&status.state)?;
        sqlx::query(
            "INSERT INTO heartbeat (agent_id, config, state) VALUES (?, ?, ?)
             ON CONFLICT(agent_id) DO UPDATE SET config = excluded.config, state = excluded.state",
        )
        .bind(&status.config.agent_id)
        .bind(&config)
        .bind(&state)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, agent_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM heartbeat_runs WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM heartbeat WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            anyhow::bail!("heartbeat not found: {agent_id}");
        }
        Ok(())
    }

    async fn append_run(&self, agent_id: &str, run: &crate::types::HeartbeatRunRecord) -> Result<()> {
        let status = serde_json::to_string(&run.status)?;
        let started_at_ms = parse::parse_absolute_time_ms(&run.started_at)?;
        let finished_at_ms = parse::parse_absolute_time_ms(&run.finished_at)?;
        let duration_ms = finished_at_ms.saturating_sub(started_at_ms);
        sqlx::query(
            "INSERT INTO heartbeat_runs (run_id, agent_id, started_at_ms, finished_at_ms, status, error, duration_ms, output, input_tokens, output_tokens)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.run_id)
        .bind(agent_id)
        .bind(started_at_ms as i64)
        .bind(finished_at_ms as i64)
        .bind(&status)
        .bind(&run.error)
        .bind(duration_ms as i64)
        .bind(&run.output_preview)
        .bind(run.input_tokens.map(|v| v as i64))
        .bind(run.output_tokens.map(|v| v as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_runs(&self, agent_id: &str, limit: usize) -> Result<Vec<crate::types::HeartbeatRunRecord>> {
        let rows = sqlx::query(
            "SELECT run_id, agent_id, started_at_ms, finished_at_ms, status, error, output, input_tokens, output_tokens
             FROM heartbeat_runs
             WHERE agent_id = ?
             ORDER BY started_at_ms DESC
             LIMIT ?",
        )
        .bind(agent_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut runs = Vec::with_capacity(rows.len());
        for row in rows {
            let status_str: String = row.get("status");
            let status = serde_json::from_str(&status_str)?;
            let started_at_ms: u64 = row.get::<i64, _>("started_at_ms").max(0) as u64;
            let finished_at_ms: u64 = row.get::<i64, _>("finished_at_ms").max(0) as u64;
            runs.push(crate::types::HeartbeatRunRecord {
                run_id: row.get("run_id"),
                agent_id: row.get("agent_id"),
                started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    started_at_ms as i64,
                )
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                finished_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    finished_at_ms as i64,
                )
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                status,
                error: row.get("error"),
                output_preview: row.get("output"),
                input_tokens: row
                    .try_get::<Option<i64>, _>("input_tokens")
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
                output_tokens: row
                    .try_get::<Option<i64>, _>("output_tokens")
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
            });
        }
        runs.reverse();
        Ok(runs)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::types::*};

    async fn make_store() -> SqliteStore {
        SqliteStore::new("sqlite::memory:").await.unwrap()
    }

    fn make_job(id: &str) -> CronJob {
        CronJob {
            job_id: id.into(),
            agent_id: "default".into(),
            name: format!("job-{id}"),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::Every { every: "1m".into() },
            prompt: "hi".into(),
            model_selector: ModelSelector::Inherit,
            timeout_secs: None,
            delivery: CronDelivery::Silent,
            state: CronJobState::default(),
        }
    }

    #[tokio::test]
    async fn test_sqlite_roundtrip() {
        let store = make_store().await;
        store.save_job(&make_job("1")).await.unwrap();
        store.save_job(&make_job("2")).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test]
    async fn test_sqlite_upsert() {
        let store = make_store().await;
        store.save_job(&make_job("1")).await.unwrap();

        let mut job = make_job("1");
        job.name = "updated".into();
        store.save_job(&job).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "updated");
    }

    #[tokio::test]
    async fn test_sqlite_delete() {
        let store = make_store().await;
        store.save_job(&make_job("1")).await.unwrap();
        store.delete_job("1").await.unwrap();
        assert!(store.load_jobs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_delete_not_found() {
        let store = make_store().await;
        assert!(store.delete_job("nope").await.is_err());
    }

    #[tokio::test]
    async fn test_sqlite_update() {
        let store = make_store().await;
        store.save_job(&make_job("1")).await.unwrap();

        let mut job = make_job("1");
        job.name = "patched".into();
        store.update_job(&job).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs[0].name, "patched");
    }

    #[tokio::test]
    async fn test_sqlite_runs() {
        let store = make_store().await;
        store.save_job(&make_job("j1")).await.unwrap();

        for i in 0..5 {
            let started_at_ms = i * 1000;
            let finished_at_ms = i * 1000 + 500;
            let run = CronRunRecord {
                run_id: format!("r{i}"),
                job_id: "j1".into(),
                started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(started_at_ms as i64)
                    .unwrap()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                finished_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(finished_at_ms as i64)
                    .unwrap()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                status: RunStatus::Ok,
                error: None,
                output_preview: None,
                input_tokens: None,
                output_tokens: None,
            };
            crate::store::CronStore::append_run(&store, "j1", &run)
                .await
                .unwrap();
        }

        let runs = crate::store::CronStore::get_runs(&store, "j1", 3)
            .await
            .unwrap();
        assert_eq!(runs.len(), 3);
        // Should be the last 3, in chronological order
        assert_eq!(runs[0].run_id, "r2");
        assert!(runs[0].started_at.contains("1970-01-01T00:00:02"));
        assert_eq!(runs[2].run_id, "r4");
        assert!(runs[2].started_at.contains("1970-01-01T00:00:04"));
    }

    #[tokio::test]
    async fn test_sqlite_runs_empty() {
        let store = make_store().await;
        let runs = crate::store::CronStore::get_runs(&store, "none", 10)
            .await
            .unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_heartbeat_roundtrip() {
        let store = make_store().await;
        let status = HeartbeatStatus {
            config: HeartbeatConfig {
                agent_id: "default".into(),
                enabled: true,
                every: "30m".into(),
                session_target: SessionTarget::Main,
                model_selector: ModelSelector::Inherit,
                active_hours: None,
            },
            state: HeartbeatState::default(),
        };
        store.upsert(&status).await.unwrap();

        let got = store.get("default").await.unwrap().expect("exists");
        assert_eq!(got.config.agent_id, "default");
    }

    #[tokio::test]
    async fn test_sqlite_heartbeat_runs_preserve_run_id() {
        let store = make_store().await;
        let status = HeartbeatStatus {
            config: HeartbeatConfig {
                agent_id: "default".into(),
                enabled: true,
                every: "30m".into(),
                session_target: SessionTarget::Main,
                model_selector: ModelSelector::Inherit,
                active_hours: None,
            },
            state: HeartbeatState::default(),
        };
        store.upsert(&status).await.unwrap();

        for i in 0..2 {
            let started_at_ms = i * 1000;
            let finished_at_ms = i * 1000 + 500;
            let run = HeartbeatRunRecord {
                run_id: format!("hb-{i}"),
                agent_id: "default".into(),
                started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    started_at_ms as i64,
                )
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                finished_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    finished_at_ms as i64,
                )
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                status: RunStatus::Ok,
                error: None,
                output_preview: None,
                input_tokens: None,
                output_tokens: None,
            };
            crate::store_heartbeat::HeartbeatStore::append_run(&store, "default", &run)
                .await
                .unwrap();
        }

        let runs = crate::store_heartbeat::HeartbeatStore::get_runs(&store, "default", 10)
            .await
            .unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, "hb-0");
        assert_eq!(runs[1].run_id, "hb-1");
    }
}

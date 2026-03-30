//! Live cron service implementation wiring the cron crate into gateway services.

use std::sync::Arc;

use {async_trait::async_trait, serde_json::Value, tracing::error};

use moltis_cron::{
    service::CronService,
    types::{CronJobCreate, CronJobPatch},
};

use crate::services::{CronService as CronServiceTrait, ServiceResult};

/// Gateway-facing cron service backed by the real [`moltis_cron::service::CronService`].
pub struct LiveCronService {
    inner: Arc<CronService>,
}

impl LiveCronService {
    pub fn new(inner: Arc<CronService>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &Arc<CronService> {
        &self.inner
    }
}

fn reason_code_or(default_code: &str, err: &str) -> String {
    let normalized = err
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let normalized = normalized.trim_matches('_').to_string();
    if normalized.is_empty() {
        default_code.to_string()
    } else {
        normalized
    }
}

#[async_trait]
impl CronServiceTrait for LiveCronService {
    async fn list(&self) -> ServiceResult {
        let jobs = self.inner.list().await;
        serde_json::to_value(jobs).map_err(|e| e.to_string())
    }

    async fn status(&self) -> ServiceResult {
        let status = self.inner.status().await;
        serde_json::to_value(status).map_err(|e| e.to_string())
    }

    async fn add(&self, params: Value) -> ServiceResult {
        let create: CronJobCreate =
            serde_json::from_value(params).map_err(|e| format!("invalid job spec: {e}"))?;
        let job = self.inner.add(create).await.map_err(|e| {
            let reason_code = reason_code_or("cron_add_failed", &e.to_string());
            error!(
                event = "cron.request.reject",
                policy = "cron_heartbeat_governance_v1",
                decision = "reject",
                reason_code = %reason_code,
                operation = "add",
                error = %e,
                "cron add failed"
            );
            e.to_string()
        })?;
        serde_json::to_value(job).map_err(|e| e.to_string())
    }

    async fn update(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id'".to_string())?;
        let patch: CronJobPatch = serde_json::from_value(
            params
                .get("patch")
                .cloned()
                .unwrap_or(Value::Object(Default::default())),
        )
        .map_err(|e| format!("invalid patch: {e}"))?;
        let job = self
            .inner
            .update(id, patch)
            .await
            .map_err(|e| {
                let reason_code = reason_code_or("cron_update_failed", &e.to_string());
                error!(
                    event = "cron.request.reject",
                    policy = "cron_heartbeat_governance_v1",
                    decision = "reject",
                    reason_code = %reason_code,
                    operation = "update",
                    job_id = %id,
                    error = %e,
                    "cron update failed"
                );
                e.to_string()
            })?;
        serde_json::to_value(job).map_err(|e| e.to_string())
    }

    async fn remove(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id'".to_string())?;
        self.inner.remove(id).await.map_err(|e| {
            let reason_code = reason_code_or("cron_remove_failed", &e.to_string());
            error!(
                event = "cron.request.reject",
                policy = "cron_heartbeat_governance_v1",
                decision = "reject",
                reason_code = %reason_code,
                operation = "remove",
                job_id = %id,
                error = %e,
                "cron remove failed"
            );
            e.to_string()
        })?;
        Ok(serde_json::json!({ "removed": id }))
    }

    async fn run(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id'".to_string())?;
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.inner.run(id, force).await.map_err(|e| {
            let reason_code = reason_code_or("cron_run_failed", &e.to_string());
            error!(
                event = "cron.request.reject",
                policy = "cron_heartbeat_governance_v1",
                decision = "reject",
                reason_code = %reason_code,
                operation = "run",
                job_id = %id,
                force,
                error = %e,
                "cron run failed"
            );
            e.to_string()
        })?;
        Ok(serde_json::json!({ "ran": id }))
    }

    async fn runs(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id'".to_string())?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let runs = self
            .inner
            .runs(id, limit)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(runs).map_err(|e| e.to_string())
    }
}

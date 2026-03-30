//! Live heartbeat service implementation wiring the cron crate into gateway services.

use std::sync::Arc;

use {async_trait::async_trait, serde_json::Value, tracing::error};

use moltis_cron::{heartbeat_service::HeartbeatService, types::HeartbeatConfig};

use crate::services::{HeartbeatService as HeartbeatServiceTrait, ServiceResult};

/// Gateway-facing heartbeat service backed by the real [`moltis_cron::heartbeat_service::HeartbeatService`].
pub struct LiveHeartbeatService {
    inner: Arc<HeartbeatService>,
}

impl LiveHeartbeatService {
    pub fn new(inner: Arc<HeartbeatService>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &Arc<HeartbeatService> {
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

fn require_agent_id(params: &Value) -> Result<&str, String> {
    params
        .get("agentId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing 'agentId'".to_string())
}

#[async_trait]
impl HeartbeatServiceTrait for LiveHeartbeatService {
    async fn status(&self, params: Value) -> ServiceResult {
        let agent_id = require_agent_id(&params)?;
        let status = self.inner.get(agent_id).await.map_err(|e| e.to_string())?;
        serde_json::to_value(status).map_err(|e| e.to_string())
    }

    async fn update(&self, params: Value) -> ServiceResult {
        let cfg: HeartbeatConfig =
            serde_json::from_value(params).map_err(|e| format!("invalid heartbeat config: {e}"))?;
        let agent_id = cfg.agent_id.clone();
        let status = self.inner.upsert(cfg).await.map_err(|e| {
            let reason_code = reason_code_or("heartbeat_update_failed", &e.to_string());
            error!(
                event = "heartbeat.request.reject",
                policy = "cron_heartbeat_governance_v1",
                decision = "reject",
                reason_code = %reason_code,
                operation = "update",
                agent_id = %agent_id,
                error = %e,
                "heartbeat update failed"
            );
            e.to_string()
        })?;
        serde_json::to_value(status).map_err(|e| e.to_string())
    }

    async fn run(&self, params: Value) -> ServiceResult {
        let agent_id = require_agent_id(&params)?;
        let force = params.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        self.inner
            .run(agent_id, force)
            .await
            .map_err(|e| {
                let reason_code = reason_code_or("heartbeat_run_failed", &e.to_string());
                error!(
                    event = "heartbeat.request.reject",
                    policy = "cron_heartbeat_governance_v1",
                    decision = "reject",
                    reason_code = %reason_code,
                    operation = "run",
                    agent_id = %agent_id,
                    force,
                    error = %e,
                    "heartbeat run failed"
                );
                e.to_string()
            })?;
        Ok(serde_json::json!({ "ran": agent_id }))
    }

    async fn runs(&self, params: Value) -> ServiceResult {
        let agent_id = require_agent_id(&params)?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let runs = self
            .inner
            .runs(agent_id, limit)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(runs).map_err(|e| e.to_string())
    }
}

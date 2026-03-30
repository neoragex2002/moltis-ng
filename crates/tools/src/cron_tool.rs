//! Agent-callable cron tool for managing scheduled jobs.

use std::sync::Arc;

use {
    anyhow::{Result, bail},
    async_trait::async_trait,
    serde_json::{Value, json},
};

use {
    moltis_agents::tool_registry::AgentTool,
    moltis_cron::{
        service::CronService,
        types::{CronJobCreate, CronJobPatch},
    },
};

/// The cron tool exposed to LLM agents.
pub struct CronTool {
    service: Arc<CronService>,
}

impl CronTool {
    pub fn new(service: Arc<CronService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AgentTool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Manage governed cron jobs. Cron execution is always isolated: it does not \
         read chat session context while running. Each job must define its own \
         prompt and post-run delivery policy: silent, session target, or telegram target."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "list", "add", "update", "remove", "run", "runs"],
                    "description": "The action to perform"
                },
                "job": {
                    "type": "object",
                    "description": "Job specification (for 'add' action)",
                    "properties": {
                        "jobId": { "type": "string", "description": "Optional UUID job id" },
                        "agentId": { "type": "string", "description": "Owning agent id" },
                        "name": { "type": "string", "description": "Human-readable job name" },
                        "schedule": {
                            "type": "object",
                            "description": "Schedule: {kind:'once', at}, {kind:'every', every}, or {kind:'cron', expr, timezone}",
                            "properties": {
                                "kind": { "type": "string", "enum": ["once", "every", "cron"] },
                                "at": { "type": "string", "description": "RFC3339 timestamp for kind=once" },
                                "every": { "type": "string", "description": "Interval string such as 30m for kind=every" },
                                "expr": { "type": "string" },
                                "timezone": { "type": "string", "description": "IANA timezone for kind=cron" }
                            },
                            "required": ["kind"]
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Task prompt executed in isolated cron runtime"
                        },
                        "modelSelector": {
                            "type": "object",
                            "description": "Model policy: {kind:'inherit'} or {kind:'explicit', modelId}",
                            "properties": {
                                "kind": { "type": "string", "enum": ["inherit", "explicit"] },
                                "modelId": { "type": "string" }
                            },
                            "required": ["kind"]
                        },
                        "timeoutSecs": { "type": "integer", "minimum": 1 },
                        "delivery": {
                            "description": "Post-run delivery: silent, session target, or telegram target",
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["silent"] }
                                    },
                                    "required": ["kind"],
                                    "additionalProperties": false
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["session"] },
                                        "target": {
                                            "description": "Session target: {kind:'main'} or {kind:'session', sessionKey}",
                                            "oneOf": [
                                                {
                                                    "type": "object",
                                                    "properties": {
                                                        "kind": { "type": "string", "enum": ["main"] }
                                                    },
                                                    "required": ["kind"],
                                                    "additionalProperties": false
                                                },
                                                {
                                                    "type": "object",
                                                    "properties": {
                                                        "kind": { "type": "string", "enum": ["session"] },
                                                        "sessionKey": { "type": "string" }
                                                    },
                                                    "required": ["kind", "sessionKey"],
                                                    "additionalProperties": false
                                                }
                                            ]
                                        }
                                    },
                                    "required": ["kind", "target"],
                                    "additionalProperties": false
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["telegram"] },
                                        "target": {
                                            "type": "object",
                                            "properties": {
                                                "accountKey": { "type": "string" },
                                                "chatId": { "type": "string" },
                                                "threadId": { "type": "string" }
                                            },
                                            "required": ["accountKey", "chatId"],
                                            "additionalProperties": false
                                        }
                                    },
                                    "required": ["kind", "target"],
                                    "additionalProperties": false
                                }
                            ]
                        },
                        "deleteAfterRun": { "type": "boolean", "default": false },
                        "enabled": { "type": "boolean", "default": true }
                    },
                    "required": ["agentId", "name", "schedule", "prompt", "delivery"]
                },
                "patch": {
                    "type": "object",
                    "description": "Fields to update (for 'update' action)"
                },
                "id": {
                    "type": "string",
                    "description": "Job ID (for update/remove/run/runs)"
                },
                "force": {
                    "type": "boolean",
                    "description": "Force-run even if disabled (for 'run' action)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max run records to return (for 'runs' action, default 20)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'action' parameter"))?;

        match action {
            "status" => {
                let status = self.service.status().await;
                Ok(serde_json::to_value(status)?)
            },
            "list" => {
                let jobs = self.service.list().await;
                Ok(serde_json::to_value(jobs)?)
            },
            "add" => {
                let job_val = params
                    .get("job")
                    .ok_or_else(|| anyhow::anyhow!("missing 'job' parameter for add"))?;
                let create: CronJobCreate = serde_json::from_value(job_val.clone())
                    .map_err(|e| anyhow::anyhow!("invalid job spec: {e}"))?;
                let job = self.service.add(create).await?;
                Ok(serde_json::to_value(job)?)
            },
            "update" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing 'id' for update"))?;
                let patch_val = params
                    .get("patch")
                    .ok_or_else(|| anyhow::anyhow!("missing 'patch' for update"))?;
                let patch: CronJobPatch = serde_json::from_value(patch_val.clone())
                    .map_err(|e| anyhow::anyhow!("invalid patch: {e}"))?;
                let job = self.service.update(id, patch).await?;
                Ok(serde_json::to_value(job)?)
            },
            "remove" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing 'id' for remove"))?;
                self.service.remove(id).await?;
                Ok(json!({ "removed": id }))
            },
            "run" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing 'id' for run"))?;
                let force = params
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                self.service.run(id, force).await?;
                Ok(json!({ "ran": id }))
            },
            "runs" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing 'id' for runs"))?;
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let runs = self.service.runs(id, limit).await?;
                Ok(serde_json::to_value(runs)?)
            },
            _ => bail!("unknown cron action: {action}"),
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moltis_cron::{
        service::{AgentTurnResult, CronService, DeliverCronFn, RunCronFn},
        store_sqlite::SqliteStore,
    };

    use super::*;

    fn noop_run() -> RunCronFn {
        Arc::new(|_| {
            Box::pin(async {
                Ok(AgentTurnResult {
                    output: "ok".into(),
                    input_tokens: None,
                    output_tokens: None,
                })
            })
        })
    }

    fn noop_deliver() -> DeliverCronFn {
        Arc::new(|_| Box::pin(async { Ok(()) }))
    }

    async fn make_tool() -> CronTool {
        let store = Arc::new(SqliteStore::new("sqlite::memory:").await.unwrap());
        let svc = CronService::new(store, noop_run(), noop_deliver());
        CronTool::new(svc)
    }

    #[tokio::test]
    async fn test_status() {
        let tool = make_tool().await;
        let result = tool.execute(json!({ "action": "status" })).await.unwrap();
        assert_eq!(result["running"], false);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let tool = make_tool().await;
        let result = tool.execute(json!({ "action": "list" })).await.unwrap();
        assert!(result.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_add_and_list() {
        let tool = make_tool().await;
        let add_result = tool
            .execute(json!({
                "action": "add",
                "job": {
                    "agentId": "default",
                    "name": "test job",
                    "schedule": { "kind": "every", "every": "1m" },
                    "prompt": "do stuff",
                    "delivery": { "kind": "silent" }
                }
            }))
            .await
            .unwrap();

        assert!(add_result.get("jobId").is_some());

        let list = tool.execute(json!({ "action": "list" })).await.unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_remove() {
        let tool = make_tool().await;
        let add = tool
            .execute(json!({
                "action": "add",
                "job": {
                    "agentId": "default",
                    "name": "to remove",
                    "schedule": { "kind": "every", "every": "1m" },
                    "prompt": "x",
                    "delivery": { "kind": "silent" }
                }
            }))
            .await
            .unwrap();

        let id = add["jobId"].as_str().unwrap();
        let result = tool
            .execute(json!({ "action": "remove", "id": id }))
            .await
            .unwrap();
        assert_eq!(result["removed"].as_str().unwrap(), id);
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = make_tool().await;
        let result = tool.execute(json!({ "action": "nope" })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_runs_empty() {
        let tool = make_tool().await;
        let result = tool
            .execute(json!({ "action": "runs", "id": "nonexistent" }))
            .await
            .unwrap();
        assert!(result.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn parameters_schema_uses_governance_v1_fields() {
        let schema = make_tool().await.parameters_schema();

        let job = &schema["properties"]["job"]["properties"];
        assert!(job.get("prompt").is_some());
        assert!(job.get("delivery").is_some());
        assert!(job.get("payload").is_none());
        assert!(job.get("sessionTarget").is_none());
    }

    #[tokio::test]
    async fn parameters_schema_does_not_mix_session_and_telegram_targets() {
        let schema = make_tool().await.parameters_schema();

        let delivery = &schema["properties"]["job"]["properties"]["delivery"];
        let one_of = delivery["oneOf"].as_array().expect("delivery.oneOf");
        let telegram = one_of
            .iter()
            .find(|variant| {
                variant["properties"]["kind"]["enum"]
                    .as_array()
                    .and_then(|vals| vals.first())
                    .and_then(|v| v.as_str())
                    == Some("telegram")
            })
            .expect("telegram delivery variant");

        let props = telegram["properties"]["target"]["properties"]
            .as_object()
            .expect("telegram target properties");
        assert!(!props.contains_key("kind"));
        assert!(!props.contains_key("sessionKey"));
        assert!(props.contains_key("accountKey"));
        assert!(props.contains_key("chatId"));
    }
}

//! Core data types for the cron + heartbeat governance model.
//!
//! Contract note:
//! - Internal identifiers use snake_case.
//! - External JSON shapes use camelCase via explicit serde rename.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

// ── Shared ──────────────────────────────────────────────────────────────────

/// Model selection policy for a single run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ModelSelector {
    /// Inherit the model configured on the target session (or global default).
    Inherit,
    /// Use an explicit model id.
    Explicit { model_id: String },
}

impl Default for ModelSelector {
    fn default() -> Self {
        Self::Inherit
    }
}

/// Outcome of a single run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Ok,
    Error,
    Skipped,
}

/// A stable session target selector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionTarget {
    Main,
    Session { session_key: String },
}

// ── Cron ────────────────────────────────────────────────────────────────────

/// How a cron job is scheduled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CronSchedule {
    /// One-shot: fire once at `at` (RFC3339).
    Once { at: String },
    /// Fixed interval: fire every `every` interval string (e.g. "30m").
    Every { every: String },
    /// Cron expression (5-field standard or 6-field with seconds).
    Cron { expr: String, timezone: String },
}

/// Cron delivery policy (execution is always isolated; delivery is post-run).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CronDelivery {
    Silent,
    Session { target: SessionTarget },
    Telegram { target: TelegramTarget },
}

/// Telegram target address (minimal, strict).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramTarget {
    pub account_key: String,
    /// Decimal string (avoid JS number precision loss).
    pub chat_id: String,
    /// Decimal string; optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Mutable runtime state of a cron job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronJobState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<RunStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
}

/// A scheduled cron job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronJob {
    pub job_id: String,
    pub agent_id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_false")]
    pub delete_after_run: bool,
    pub schedule: CronSchedule,
    pub prompt: String,
    #[serde(default)]
    pub model_selector: ModelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    pub delivery: CronDelivery,
    #[serde(default)]
    pub state: CronJobState,
}

/// Input for creating a new cron job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronJobCreate {
    /// Optional job id. If not provided, a UUID will be generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub agent_id: String,
    pub name: String,
    pub schedule: CronSchedule,
    pub prompt: String,
    #[serde(default)]
    pub model_selector: ModelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    pub delivery: CronDelivery,
    #[serde(default = "default_false")]
    pub delete_after_run: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Patch for updating an existing cron job.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronJobPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<CronSchedule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_selector: Option<ModelSelector>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub timeout_secs: Option<Option<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<CronDelivery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_after_run: Option<bool>,
}

/// Summary status of the cron system.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronStatus {
    pub running: bool,
    pub job_count: usize,
    pub enabled_count: usize,
    pub next_run_at: Option<String>,
}

/// Record of a completed cron job run, stored in run history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CronRunRecord {
    pub run_id: String,
    pub job_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

/// Notification emitted when cron jobs change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CronNotification {
    Created { job: CronJob },
    Updated { job: CronJob },
    Removed { job_id: String },
}

// ── Heartbeat ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveHours {
    pub start: String,
    pub end: String,
    pub timezone: String,
}

/// Heartbeat configuration stored in DB (prompt is owned by agents/<agent_id>/HEARTBEAT.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeartbeatConfig {
    pub agent_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub every: String,
    pub session_target: SessionTarget,
    #[serde(default)]
    pub model_selector: ModelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_hours: Option<ActiveHours>,
}

/// Mutable runtime state of a heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeartbeatState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<RunStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
}

/// Summary status of a heartbeat (typed projection for UI/RPC).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeartbeatStatus {
    pub config: HeartbeatConfig,
    pub state: HeartbeatState,
}

/// Record of a completed heartbeat run, stored in run history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeartbeatRunRecord {
    pub run_id: String,
    pub agent_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_schedule_roundtrip_once() {
        let s = CronSchedule::Once {
            at: "2026-03-29T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"once\""));
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn cron_schedule_roundtrip_every() {
        let s = CronSchedule::Every { every: "30m".into() };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"every\""));
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn cron_schedule_roundtrip_cron() {
        let s = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            timezone: "Asia/Shanghai".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"cron\""));
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn model_selector_roundtrip_inherit() {
        let m = ModelSelector::Inherit;
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, "{\"kind\":\"inherit\"}");
        let back: ModelSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn telegram_target_rejects_unknown_fields() {
        let bad = serde_json::json!({
            "accountKey": "acc",
            "chatId": "123",
            "username": "nope"
        });
        let err = serde_json::from_value::<TelegramTarget>(bad).expect_err("unknown fields");
        let msg = err.to_string();
        assert!(msg.contains("unknown field"));
    }

    #[test]
    fn cron_job_patch_allows_clearing_timeout_secs_with_null() {
        let clear = serde_json::json!({ "timeoutSecs": null });
        let patch: CronJobPatch = serde_json::from_value(clear).unwrap();
        assert_eq!(patch.timeout_secs, Some(None));

        let omit = serde_json::json!({});
        let patch: CronJobPatch = serde_json::from_value(omit).unwrap();
        assert_eq!(patch.timeout_secs, None);

        let set = serde_json::json!({ "timeoutSecs": 42 });
        let patch: CronJobPatch = serde_json::from_value(set).unwrap();
        assert_eq!(patch.timeout_secs, Some(Some(42)));

        let json = serde_json::to_value(&CronJobPatch {
            timeout_secs: Some(None),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(json.get("timeoutSecs"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn cron_schedule_rejects_legacy_at_ms() {
        let bad = serde_json::json!({
            "kind": "once",
            "at_ms": 123
        });
        assert!(serde_json::from_value::<CronSchedule>(bad).is_err());
    }

    #[test]
    fn cron_schedule_rejects_legacy_tz_field() {
        let bad = serde_json::json!({
            "kind": "cron",
            "expr": "0 9 * * *",
            "timezone": "UTC",
            "tz": "UTC"
        });
        assert!(serde_json::from_value::<CronSchedule>(bad).is_err());
    }

    #[test]
    fn cron_job_create_rejects_legacy_payload_kind() {
        let bad = serde_json::json!({
            "jobId": "j1",
            "agentId": "default",
            "name": "x",
            "enabled": true,
            "schedule": { "kind": "every", "every": "1m" },
            "payloadKind": "agentTurn",
            "prompt": "hi",
            "modelSelector": { "kind": "inherit" },
            "delivery": { "kind": "silent" },
            "deleteAfterRun": false
        });
        assert!(serde_json::from_value::<CronJobCreate>(bad).is_err());
    }
}

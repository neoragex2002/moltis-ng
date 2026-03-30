//! Persistence trait for heartbeat configuration + state + run history.

use {anyhow::Result, async_trait::async_trait};

use crate::types::{HeartbeatRunRecord, HeartbeatStatus};

/// Persistence backend for heartbeat config/state and run history.
#[async_trait]
pub trait HeartbeatStore: Send + Sync {
    /// Load all heartbeats (config + state).
    async fn load_all(&self) -> Result<Vec<HeartbeatStatus>>;
    /// Get a single heartbeat by agent_id.
    async fn get(&self, agent_id: &str) -> Result<Option<HeartbeatStatus>>;
    /// Insert or update a heartbeat (config + state).
    async fn upsert(&self, status: &HeartbeatStatus) -> Result<()>;
    /// Delete a heartbeat and its run history.
    async fn delete(&self, agent_id: &str) -> Result<()>;
    /// Append a run record.
    async fn append_run(&self, agent_id: &str, run: &HeartbeatRunRecord) -> Result<()>;
    /// Get recent runs, oldest-first.
    async fn get_runs(&self, agent_id: &str, limit: usize) -> Result<Vec<HeartbeatRunRecord>>;
}


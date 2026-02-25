use {anyhow::Result, async_trait::async_trait, serde::Serialize};

/// A persisted channel configuration.
#[derive(Debug, Clone, Serialize)]
pub struct StoredChannel {
    pub account_handle: String,
    pub channel_type: String,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Persistent storage for channel configurations.
#[async_trait]
pub trait ChannelStore: Send + Sync {
    async fn list(&self) -> Result<Vec<StoredChannel>>;
    async fn get(&self, account_handle: &str) -> Result<Option<StoredChannel>>;
    async fn upsert(&self, channel: StoredChannel) -> Result<()>;
    async fn delete(&self, account_handle: &str) -> Result<()>;
}

use std::sync::Arc;

use {
    async_trait::async_trait,
    secrecy::ExposeSecret,
    serde_json::Value,
    tokio::sync::RwLock,
    tracing::{error, info, warn},
};

use {moltis_channels::ChannelPlugin, moltis_telegram::TelegramPlugin};

use {
    moltis_channels::{
        message_log::MessageLog,
        store::{ChannelStore, StoredChannel},
    },
    moltis_sessions::metadata::SqliteSessionMetadata,
};

use crate::services::{ChannelService, ServiceResult};

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn merge_json_in_place(base: &mut Value, patch: &Value) {
    let Value::Object(patch_obj) = patch else {
        *base = patch.clone();
        return;
    };

    let Value::Object(base_obj) = base else {
        *base = patch.clone();
        return;
    };

    for (key, patch_val) in patch_obj {
        match (base_obj.get_mut(key), patch_val) {
            (Some(base_val), Value::Object(_)) if base_val.is_object() => {
                merge_json_in_place(base_val, patch_val);
            },
            // Explicit `null` overwrites to null (does not delete).
            (Some(base_val), _) => {
                *base_val = patch_val.clone();
            },
            (None, _) => {
                base_obj.insert(key.clone(), patch_val.clone());
            },
        }
    }
}

/// Live channel service backed by `TelegramPlugin`.
pub struct LiveChannelService {
    telegram: Arc<RwLock<TelegramPlugin>>,
    store: Arc<dyn ChannelStore>,
    message_log: Arc<dyn MessageLog>,
    session_metadata: Arc<SqliteSessionMetadata>,
}

impl LiveChannelService {
    pub fn new(
        telegram: TelegramPlugin,
        store: Arc<dyn ChannelStore>,
        message_log: Arc<dyn MessageLog>,
        session_metadata: Arc<SqliteSessionMetadata>,
    ) -> Self {
        Self {
            telegram: Arc::new(RwLock::new(telegram)),
            store,
            message_log,
            session_metadata,
        }
    }
}

#[async_trait]
impl ChannelService for LiveChannelService {
    async fn status(&self) -> ServiceResult {
        let tg = self.telegram.read().await;
        let account_ids = tg.account_ids();
        let mut channels = Vec::new();

        if let Some(status) = tg.status() {
            for aid in &account_ids {
                match status.probe(aid).await {
                    Ok(snap) => {
                        let mut entry = serde_json::json!({
                            "type": "telegram",
                            "name": format!("Telegram ({})", aid),
                            "account_id": aid,
                            "status": if snap.connected { "connected" } else { "disconnected" },
                            "details": snap.details,
                        });
                        if let Some(cfg) = tg.account_config(aid) {
                            entry["config"] = cfg;
                        }

                        // Include bound sessions and active session mappings.
                        let bound = self
                            .session_metadata
                            .list_account_sessions("telegram", aid)
                            .await;
                        let active_map = self
                            .session_metadata
                            .list_active_sessions("telegram", aid)
                            .await;
                        let sessions: Vec<_> = bound
                            .iter()
                            .map(|s| {
                                let is_active = active_map.iter().any(|(_, sk)| sk == &s.key);
                                serde_json::json!({
                                    "key": s.key,
                                    "label": s.label,
                                    "messageCount": s.message_count,
                                    "active": is_active,
                                })
                            })
                            .collect();
                        if !sessions.is_empty() {
                            entry["sessions"] = serde_json::json!(sessions);
                        }

                        channels.push(entry);
                    },
                    Err(e) => {
                        channels.push(serde_json::json!({
                            "type": "telegram",
                            "name": format!("Telegram ({})", aid),
                            "account_id": aid,
                            "status": "error",
                            "details": e.to_string(),
                        }));
                    },
                }
            }
        }

        Ok(serde_json::json!({ "channels": channels }))
    }

    async fn add(&self, params: Value) -> ServiceResult {
        let channel_type = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("telegram");

        if channel_type != "telegram" {
            return Err(format!("unsupported channel type: {channel_type}"));
        }

        let config = params
            .get("config")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        let mut tg_cfg: moltis_telegram::TelegramAccountConfig =
            serde_json::from_value(config.clone()).map_err(|e| e.to_string())?;
        if tg_cfg.token.expose_secret().is_empty() {
            return Err("telegram bot token is required".into());
        }

        let identity = moltis_telegram::bot::probe_bot_identity(tg_cfg.token.expose_secret())
            .await
            .map_err(|e| format!("telegram getMe failed: {e}"))?;
        let account_id = format!("telegram:{}", identity.chan_user_id);

        if let Some(supplied) = params.get("account_id").and_then(|v| v.as_str())
            && supplied != account_id
        {
            warn!(
                supplied,
                derived = %account_id,
                "telegram channel add: ignoring supplied account_id and using derived identity handle"
            );
        }

        tg_cfg.chan_user_id = Some(identity.chan_user_id);
        tg_cfg.chan_user_name = identity.chan_user_name;
        tg_cfg.chan_nickname = identity.chan_nickname;

        let config = serde_json::to_value(tg_cfg).map_err(|e| e.to_string())?;

        info!(account_id, "adding telegram channel account");

        let mut tg = self.telegram.write().await;
        tg.start_account(&account_id, config.clone())
            .await
            .map_err(|e| {
                error!(error = %e, account_id, "failed to start telegram account");
                e.to_string()
            })?;

        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_id: account_id.to_string(),
                channel_type: "telegram".into(),
                config,
                created_at: now,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, account_id, "failed to persist channel");
        }

        Ok(serde_json::json!({ "added": account_id }))
    }

    async fn remove(&self, params: Value) -> ServiceResult {
        let account_id = params
            .get("account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'account_id'".to_string())?;

        info!(account_id, "removing telegram channel account");

        let mut tg = self.telegram.write().await;
        tg.stop_account(account_id).await.map_err(|e| {
            error!(error = %e, account_id, "failed to stop telegram account");
            e.to_string()
        })?;

        if let Err(e) = self.store.delete(account_id).await {
            warn!(error = %e, account_id, "failed to delete channel from store");
        }

        Ok(serde_json::json!({ "removed": account_id }))
    }

    async fn logout(&self, params: Value) -> ServiceResult {
        self.remove(params).await
    }

    async fn update(&self, params: Value) -> ServiceResult {
        let account_id = params
            .get("account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'account_id'".to_string())?;

        let patch = params
            .get("config")
            .cloned()
            .ok_or_else(|| "missing 'config'".to_string())?;

        info!(account_id, "updating telegram channel account");

        // Merge patch into stored config so UI updates don't reset unseen fields (token, etc).
        let stored = self
            .store
            .get(account_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{account_id}' not found in store"))?;

        if !patch.is_object() {
            return Err("config patch must be an object".into());
        }
        let mut merged = stored.config.clone();
        merge_json_in_place(&mut merged, &patch);

        // Validate and normalize before stopping the bot (avoid downtime on invalid patches).
        let tg_cfg: moltis_telegram::TelegramAccountConfig =
            serde_json::from_value(merged.clone()).map_err(|e| e.to_string())?;
        if tg_cfg.token.expose_secret().is_empty() {
            return Err("telegram bot token is required".into());
        }
        let merged = serde_json::to_value(tg_cfg).map_err(|e| e.to_string())?;

        let mut tg = self.telegram.write().await;

        // Stop then restart with new config
        tg.stop_account(account_id).await.map_err(|e| {
            error!(error = %e, account_id, "failed to stop telegram account for update");
            e.to_string()
        })?;

        tg.start_account(account_id, merged.clone())
            .await
            .map_err(|e| {
                error!(error = %e, account_id, "failed to restart telegram account after update");
                e.to_string()
            })?;

        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_id: account_id.to_string(),
                channel_type: "telegram".into(),
                config: merged,
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, account_id, "failed to persist channel update");
        }

        Ok(serde_json::json!({ "updated": account_id }))
    }

    async fn send(&self, _params: Value) -> ServiceResult {
        Err("direct channel send not yet implemented".into())
    }

    async fn senders_list(&self, params: Value) -> ServiceResult {
        let account_id = params
            .get("account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'account_id'".to_string())?;

        let senders = self
            .message_log
            .unique_senders(account_id)
            .await
            .map_err(|e| e.to_string())?;

        // Read allowlist from current config to tag each sender.
        let tg = self.telegram.read().await;
        let allowlist: Vec<String> = tg
            .account_config(account_id)
            .and_then(|cfg| cfg.get("allowlist").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        // Query pending OTP challenges for this account.
        let otp_challenges = {
            let tg_inner = self.telegram.read().await;
            tg_inner.pending_otp_challenges(account_id)
        };

        let list: Vec<Value> = senders
            .into_iter()
            .map(|s| {
                let is_allowed = allowlist.iter().any(|a| {
                    let a_lower = a.to_lowercase();
                    a_lower == s.peer_id.to_lowercase()
                        || s.username
                            .as_ref()
                            .is_some_and(|u| a_lower == u.to_lowercase())
                });
                let mut entry = serde_json::json!({
                    "peer_id": s.peer_id,
                    "username": s.username,
                    "sender_name": s.sender_name,
                    "message_count": s.message_count,
                    "last_seen": s.last_seen,
                    "allowed": is_allowed,
                });
                // Attach OTP info if a challenge is pending for this peer.
                if let Some(otp) = otp_challenges.iter().find(|c| c.peer_id == s.peer_id) {
                    entry["otp_pending"] = serde_json::json!({
                        "code": otp.code,
                        "expires_at": otp.expires_at,
                    });
                }
                entry
            })
            .collect();

        Ok(serde_json::json!({ "senders": list }))
    }

    async fn sender_approve(&self, params: Value) -> ServiceResult {
        let account_id = params
            .get("account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'account_id'".to_string())?;

        let identifier = params
            .get("identifier")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'identifier'".to_string())?;

        // Read current stored config, add identifier to allowlist, persist & restart.
        let stored = self
            .store
            .get(account_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{account_id}' not found in store"))?;

        let mut config = stored.config.clone();
        let allowlist = config
            .as_object_mut()
            .ok_or_else(|| "config is not an object".to_string())?
            .entry("allowlist")
            .or_insert_with(|| serde_json::json!([]));

        let arr = allowlist
            .as_array_mut()
            .ok_or_else(|| "allowlist is not an array".to_string())?;

        let id_lower = identifier.to_lowercase();
        if !arr
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.to_lowercase() == id_lower))
        {
            arr.push(serde_json::json!(identifier));
        }

        // Also ensure dm_policy is set to "allowlist" so the list is enforced.
        if let Some(obj) = config.as_object_mut() {
            obj.insert("dm_policy".into(), serde_json::json!("allowlist"));
        }

        // Persist.
        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_id: account_id.to_string(),
                channel_type: "telegram".into(),
                config: config.clone(),
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, account_id, "failed to persist sender approval");
        }

        // Hot-update the in-memory config (no bot restart, preserves polling
        // offset so Telegram doesn't re-deliver the OTP code message).
        let tg = self.telegram.read().await;
        if let Err(e) = tg.update_account_config(account_id, config) {
            warn!(error = %e, account_id, "failed to hot-update config for sender approval");
        }

        info!(account_id, identifier, "sender approved");
        Ok(serde_json::json!({ "approved": identifier }))
    }

    async fn sender_deny(&self, params: Value) -> ServiceResult {
        let account_id = params
            .get("account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'account_id'".to_string())?;

        let identifier = params
            .get("identifier")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'identifier'".to_string())?;

        let stored = self
            .store
            .get(account_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{account_id}' not found in store"))?;

        let mut config = stored.config.clone();
        if let Some(arr) = config
            .as_object_mut()
            .and_then(|o| o.get_mut("allowlist"))
            .and_then(|v| v.as_array_mut())
        {
            let id_lower = identifier.to_lowercase();
            arr.retain(|v| v.as_str().is_none_or(|s| s.to_lowercase() != id_lower));
        }

        // Persist.
        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_id: account_id.to_string(),
                channel_type: "telegram".into(),
                config: config.clone(),
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, account_id, "failed to persist sender denial");
        }

        // Hot-update the in-memory config (no bot restart needed for allowlist removal).
        let tg = self.telegram.read().await;
        if let Err(e) = tg.update_account_config(account_id, config) {
            warn!(error = %e, account_id, "failed to hot-update config for sender denial");
        }

        info!(account_id, identifier, "sender denied");
        Ok(serde_json::json!({ "denied": identifier }))
    }

    async fn list_telegram_accounts(&self) -> Vec<String> {
        let tg = self.telegram.read().await;
        tg.account_ids()
    }

    async fn telegram_bus_accounts_snapshot(
        &self,
    ) -> Vec<moltis_telegram::config::TelegramBusAccountSnapshot> {
        let tg = self.telegram.read().await;
        tg.bus_accounts_snapshot()
    }

    async fn telegram_account_persona_id(&self, account_id: &str) -> Option<String> {
        let stored = self.store.get(account_id).await.ok().flatten()?;
        let cfg: moltis_telegram::TelegramAccountConfig = serde_json::from_value(stored.config).ok()?;
        cfg.persona_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_unseen_fields_and_overwrites_known() {
        let mut base = serde_json::json!({
            "token": "123:ABC",
            "dm_policy": "open",
            "allowlist": ["user1"]
        });
        let patch = serde_json::json!({
            "dm_policy": "allowlist"
        });
        merge_json_in_place(&mut base, &patch);

        assert_eq!(base["token"], "123:ABC");
        assert_eq!(base["dm_policy"], "allowlist");
        assert_eq!(base["allowlist"], serde_json::json!(["user1"]));
    }

    #[test]
    fn merge_null_overwrites_to_null() {
        let mut base = serde_json::json!({
            "model": "gpt-5.2",
            "model_provider": "openai-responses"
        });
        let patch = serde_json::json!({
            "model_provider": null
        });
        merge_json_in_place(&mut base, &patch);

        assert_eq!(base["model"], "gpt-5.2");
        assert!(base.get("model_provider").is_some());
        assert!(base["model_provider"].is_null());
    }

    #[test]
    fn merge_recurses_into_child_objects() {
        let mut base = serde_json::json!({
            "a": { "b": 1, "c": 2 },
            "x": 1
        });
        let patch = serde_json::json!({
            "a": { "b": 3 }
        });
        merge_json_in_place(&mut base, &patch);

        assert_eq!(base, serde_json::json!({ "a": { "b": 3, "c": 2 }, "x": 1 }));
    }

    #[test]
    fn merge_replaces_non_object_values() {
        let mut base = serde_json::json!({
            "a": 1,
            "b": { "x": 1 }
        });
        let patch = serde_json::json!({
            "a": { "y": 2 },
            "b": "nope",
            "c": [1,2,3]
        });
        merge_json_in_place(&mut base, &patch);

        assert_eq!(base["a"], serde_json::json!({ "y": 2 }));
        assert_eq!(base["b"], "nope");
        assert_eq!(base["c"], serde_json::json!([1, 2, 3]));
    }
}

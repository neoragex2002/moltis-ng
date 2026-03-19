use std::{collections::BTreeSet, sync::Arc};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramConfigPatchKind {
    HotUpdate,
    IdentityChange,
}

fn classify_telegram_config_patch(patch: &Value) -> Result<TelegramConfigPatchKind, String> {
    let patch_obj = patch
        .as_object()
        .ok_or_else(|| "config patch must be an object".to_string())?;

    let mut touches_identity = false;
    for key in patch_obj.keys() {
        match key.as_str() {
            "dm_policy"
            | "mention_mode"
            | "allowlist"
            | "dm_scope"
            | "group_scope"
            | "relay_chain_enabled"
            | "relay_hop_limit"
            | "epoch_relay_budget"
            | "relay_strictness"
            | "group_session_transcript_format"
            | "stream_mode"
            | "edit_throttle_ms"
            | "outbound_max_attempts"
            | "outbound_retry_base_delay_ms"
            | "outbound_retry_max_delay_ms"
            | "model"
            | "model_provider"
            | "otp_self_approval"
            | "otp_cooldown_secs"
            | "persona_id" => {},
            "token" | "chan_user_id" | "chan_user_name" | "chan_nickname" => {
                touches_identity = true;
            },
            other => {
                return Err(format!(
                    "unsupported telegram config field in update: {other}"
                ));
            },
        }
    }

    Ok(if touches_identity {
        TelegramConfigPatchKind::IdentityChange
    } else {
        TelegramConfigPatchKind::HotUpdate
    })
}

fn merge_telegram_account_keys(
    runtime_handles: Vec<String>,
    stored_channels: &[StoredChannel],
) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for handle in runtime_handles {
        keys.insert(handle);
    }
    for channel in stored_channels {
        if channel.channel_type == "telegram" {
            keys.insert(channel.account_handle.clone());
        }
    }
    keys.into_iter().collect()
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
        let stored_channels = match self.store.list().await {
            Ok(channels) => channels,
            Err(err) => {
                warn!(error = %err, "failed to list stored channels for status");
                Vec::new()
            },
        };
        let tg = self.telegram.read().await;
        let chan_account_keys = merge_telegram_account_keys(tg.account_handles(), &stored_channels);
        let mut channels = Vec::new();

        if let Some(status) = tg.status() {
            for chan_account_key in &chan_account_keys {
                match status.probe(chan_account_key).await {
                    Ok(snap) => {
                        let mut entry = serde_json::json!({
                            "chanType": "telegram",
                            "name": format!("Telegram ({})", chan_account_key),
                            "chanAccountKey": chan_account_key,
                            "status": if snap.connected { "connected" } else { "disconnected" },
                            "details": snap.details,
                        });
                        if let Some(cfg) = tg.account_config(chan_account_key).or_else(|| {
                            stored_channels
                                .iter()
                                .find(|channel| {
                                    channel.channel_type == "telegram"
                                        && channel.account_handle == *chan_account_key
                                })
                                .map(|channel| channel.config.clone())
                        }) {
                            entry["config"] = cfg;
                        }

                        // Include bound sessions and active session mappings.
                        let bound = self
                            .session_metadata
                            .list_account_sessions("telegram", chan_account_key)
                            .await;
                        let active_map = self
                            .session_metadata
                            .list_active_sessions("telegram", chan_account_key)
                            .await;
                        let sessions: Vec<_> = bound
                            .iter()
                            .map(|s| {
                                let is_active = active_map.iter().any(|(_, sk)| sk == &s.key);
                                serde_json::json!({
                                    "sessionId": s.key,
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
                            "chanType": "telegram",
                            "name": format!("Telegram ({})", chan_account_key),
                            "chanAccountKey": chan_account_key,
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
        let chan_account_key = format!("telegram:{}", identity.chan_user_id);

        if let Some(supplied) = params.get("chanAccountKey").and_then(|v| v.as_str())
            && supplied != chan_account_key
        {
            warn!(
                supplied,
                derived = %chan_account_key,
                "telegram channel add: ignoring supplied chanAccountKey and using derived identity handle"
            );
        }

        tg_cfg.chan_user_id = Some(identity.chan_user_id);
        tg_cfg.chan_user_name = identity.chan_user_name;
        tg_cfg.chan_nickname = identity.chan_nickname;

        let config = serde_json::to_value(tg_cfg).map_err(|e| e.to_string())?;

        info!(chan_account_key, "adding telegram channel account");

        let mut tg = self.telegram.write().await;
        tg.start_account(&chan_account_key, config.clone())
            .await
            .map_err(|e| {
                error!(error = %e, chan_account_key, "failed to start telegram account");
                e.to_string()
            })?;

        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_handle: chan_account_key.to_string(),
                channel_type: "telegram".into(),
                config,
                created_at: now,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, chan_account_key, "failed to persist channel");
        }

        Ok(serde_json::json!({ "chanAccountKey": chan_account_key }))
    }

    async fn remove(&self, params: Value) -> ServiceResult {
        let chan_account_key = params
            .get("chanAccountKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'chanAccountKey'".to_string())?;

        info!(chan_account_key, "removing telegram channel account");

        let mut tg = self.telegram.write().await;
        tg.stop_account(chan_account_key).await.map_err(|e| {
            error!(error = %e, chan_account_key, "failed to stop telegram account");
            e.to_string()
        })?;

        if let Err(e) = self.store.delete(chan_account_key).await {
            warn!(error = %e, chan_account_key, "failed to delete channel from store");
        }

        Ok(serde_json::json!({ "chanAccountKey": chan_account_key }))
    }

    async fn logout(&self, params: Value) -> ServiceResult {
        self.remove(params).await
    }

    async fn update(&self, params: Value) -> ServiceResult {
        let chan_account_key = params
            .get("chanAccountKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'chanAccountKey'".to_string())?;

        let patch = params
            .get("config")
            .cloned()
            .ok_or_else(|| "missing 'config'".to_string())?;

        info!(chan_account_key, "updating telegram channel account");

        // Merge patch into stored config so UI updates don't reset unseen fields (token, etc).
        let stored = self
            .store
            .get(chan_account_key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{chan_account_key}' not found in store"))?;

        let patch_kind = classify_telegram_config_patch(&patch)?;
        let mut merged = stored.config.clone();
        merge_json_in_place(&mut merged, &patch);

        // Validate and normalize before touching the running bot.
        let tg_cfg: moltis_telegram::TelegramAccountConfig =
            serde_json::from_value(merged.clone()).map_err(|e| e.to_string())?;
        if tg_cfg.token.expose_secret().is_empty() {
            return Err("telegram bot token is required".into());
        }
        let merged = serde_json::to_value(tg_cfg).map_err(|e| e.to_string())?;

        match patch_kind {
            TelegramConfigPatchKind::HotUpdate => {
                let tg = self.telegram.read().await;
                tg.update_account_config(chan_account_key, merged.clone())
                    .map_err(|e| {
                        error!(error = %e, chan_account_key, "failed to hot-update telegram account");
                        e.to_string()
                    })?;
            },
            TelegramConfigPatchKind::IdentityChange => {
                return Err(
                    "telegram identity fields cannot be updated in place; remove and re-add the bot"
                        .into(),
                );
            },
        }

        let now = unix_now();
        if let Err(e) = self
            .store
            .upsert(StoredChannel {
                account_handle: chan_account_key.to_string(),
                channel_type: "telegram".into(),
                config: merged,
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, chan_account_key, "failed to persist channel update");
        }

        Ok(serde_json::json!({ "chanAccountKey": chan_account_key }))
    }

    async fn send(&self, _params: Value) -> ServiceResult {
        Err("direct channel send not yet implemented".into())
    }

    async fn senders_list(&self, params: Value) -> ServiceResult {
        let chan_account_key = params
            .get("chanAccountKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'chanAccountKey'".to_string())?;

        let senders = self
            .message_log
            .unique_senders(chan_account_key)
            .await
            .map_err(|e| e.to_string())?;

        // Read allowlist from current config to tag each sender.
        let tg = self.telegram.read().await;
        let allowlist: Vec<String> = tg
            .account_config(chan_account_key)
            .and_then(|cfg| cfg.get("allowlist").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        // Query pending OTP challenges for this account.
        let otp_challenges = {
            let tg_inner = self.telegram.read().await;
            tg_inner.pending_otp_challenges(chan_account_key)
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
                    "peerId": s.peer_id,
                    "username": s.username,
                    "senderName": s.sender_name,
                    "messageCount": s.message_count,
                    "lastSeen": s.last_seen,
                    "allowed": is_allowed,
                });
                // Attach OTP info if a challenge is pending for this peer.
                if let Some(otp) = otp_challenges.iter().find(|c| c.peer_id == s.peer_id) {
                    entry["otpPending"] = serde_json::json!({
                        "code": otp.code,
                        "expiresAt": otp.expires_at,
                    });
                }
                entry
            })
            .collect();

        Ok(serde_json::json!({ "senders": list }))
    }

    async fn sender_approve(&self, params: Value) -> ServiceResult {
        let chan_account_key = params
            .get("chanAccountKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'chanAccountKey'".to_string())?;

        let identifier = params
            .get("identifier")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'identifier'".to_string())?;

        // Read current stored config, add identifier to allowlist, persist & restart.
        let stored = self
            .store
            .get(chan_account_key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{chan_account_key}' not found in store"))?;

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
                account_handle: chan_account_key.to_string(),
                channel_type: "telegram".into(),
                config: config.clone(),
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, chan_account_key, "failed to persist sender approval");
        }

        // Hot-update the in-memory config (no bot restart, preserves polling
        // offset so Telegram doesn't re-deliver the OTP code message).
        let tg = self.telegram.read().await;
        if let Err(e) = tg.update_account_config(chan_account_key, config) {
            warn!(error = %e, chan_account_key, "failed to hot-update config for sender approval");
        }

        info!(chan_account_key, identifier, "sender approved");
        Ok(serde_json::json!({ "approved": identifier }))
    }

    async fn sender_deny(&self, params: Value) -> ServiceResult {
        let chan_account_key = params
            .get("chanAccountKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'chanAccountKey'".to_string())?;

        let identifier = params
            .get("identifier")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'identifier'".to_string())?;

        let stored = self
            .store
            .get(chan_account_key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("channel '{chan_account_key}' not found in store"))?;

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
                account_handle: chan_account_key.to_string(),
                channel_type: "telegram".into(),
                config: config.clone(),
                created_at: stored.created_at,
                updated_at: now,
            })
            .await
        {
            warn!(error = %e, chan_account_key, "failed to persist sender denial");
        }

        // Hot-update the in-memory config (no bot restart needed for allowlist removal).
        let tg = self.telegram.read().await;
        if let Err(e) = tg.update_account_config(chan_account_key, config) {
            warn!(error = %e, chan_account_key, "failed to hot-update config for sender denial");
        }

        info!(chan_account_key, identifier, "sender denied");
        Ok(serde_json::json!({ "denied": identifier }))
    }

    async fn list_telegram_accounts(&self) -> Vec<String> {
        let stored_channels = self.store.list().await.unwrap_or_default();
        let tg = self.telegram.read().await;
        merge_telegram_account_keys(tg.account_handles(), &stored_channels)
    }

    async fn telegram_bus_accounts_snapshot(
        &self,
    ) -> Vec<moltis_telegram::config::TelegramBusAccountSnapshot> {
        let tg = self.telegram.read().await;
        tg.bus_accounts_snapshot()
    }

    async fn telegram_account_persona_id(&self, chan_account_key: &str) -> Option<String> {
        let stored = self.store.get(chan_account_key).await.ok().flatten()?;
        let cfg: moltis_telegram::TelegramAccountConfig =
            serde_json::from_value(stored.config).ok()?;
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

    #[test]
    fn classify_telegram_config_patch_detects_identity_changes() {
        assert_eq!(
            classify_telegram_config_patch(&serde_json::json!({"allowlist": ["alice"]})).unwrap(),
            TelegramConfigPatchKind::HotUpdate
        );
        assert_eq!(
            classify_telegram_config_patch(&serde_json::json!({"token": "123:ABC"})).unwrap(),
            TelegramConfigPatchKind::IdentityChange
        );
        assert_eq!(
            classify_telegram_config_patch(&serde_json::json!({"chan_user_name": "new_name"}))
                .unwrap(),
            TelegramConfigPatchKind::IdentityChange
        );
        assert!(classify_telegram_config_patch(&serde_json::json!({"unexpected": true})).is_err());
    }

    #[test]
    fn merge_telegram_account_keys_unions_runtime_and_store() {
        let merged = merge_telegram_account_keys(
            vec!["telegram:1".into(), "telegram:3".into()],
            &[
                StoredChannel {
                    account_handle: "telegram:2".into(),
                    channel_type: "telegram".into(),
                    config: serde_json::json!({}),
                    created_at: 0,
                    updated_at: 0,
                },
                StoredChannel {
                    account_handle: "other:1".into(),
                    channel_type: "discord".into(),
                    config: serde_json::json!({}),
                    created_at: 0,
                    updated_at: 0,
                },
            ],
        );

        assert_eq!(
            merged,
            vec![
                "telegram:1".to_string(),
                "telegram:2".to_string(),
                "telegram:3".to_string()
            ]
        );
    }
}

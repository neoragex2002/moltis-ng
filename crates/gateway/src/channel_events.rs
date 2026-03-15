use std::{future::Future, sync::Arc};

use {
    anyhow::{Result, anyhow},
    async_trait::async_trait,
    moltis_tools::image_cache::ImageBuilder,
    tracing::{debug, error, info, warn},
};

use {
    moltis_channels::{
        ChannelAttachment, ChannelEvent, ChannelEventSink, ChannelMessageMeta, ChannelReplyTarget,
        ChannelType,
    },
    moltis_common::identity,
    moltis_sessions::metadata::SqliteSessionMetadata,
};

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    session::extract_preview_from_value,
    state::GatewayState,
};

#[cfg(not(test))]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);
#[cfg(test)]
const TELEGRAM_TYPING_KEEPALIVE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(10);

fn format_context_v1_payload(payload: serde_json::Value) -> String {
    serde_json::json!({
        "format": "context.v1",
        "payload": payload,
    })
    .to_string()
}

fn default_chan_chat_key(target: &ChannelReplyTarget) -> String {
    let chan_user_id = match target.chan_type {
        ChannelType::Telegram => target
            .chan_account_key
            .strip_prefix("telegram:")
            .unwrap_or(&target.chan_account_key),
    };
    identity::format_chan_chat_key(
        target.chan_type.as_str(),
        chan_user_id,
        &target.chat_id,
        None,
    )
}

async fn resolve_channel_session_id(
    target: &ChannelReplyTarget,
    metadata: &SqliteSessionMetadata,
) -> Option<String> {
    metadata
        .get_active_session_id(
            target.chan_type.as_str(),
            &target.chan_account_key,
            &target.chat_id,
        )
        .await
}

async fn ensure_channel_session_id(
    target: &ChannelReplyTarget,
    metadata: &SqliteSessionMetadata,
) -> String {
    if let Some(id) = resolve_channel_session_id(target, metadata).await {
        return id;
    }
    let new_id = format!("session:{}", uuid::Uuid::new_v4());
    metadata
        .set_active_session_id(
            target.chan_type.as_str(),
            &target.chan_account_key,
            &target.chat_id,
            &new_id,
        )
        .await;
    new_id
}

async fn run_with_telegram_typing_loop<T>(
    outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound>,
    chan_account_key: String,
    chat_id: String,
    op: &'static str,
    operation: impl Future<Output = T>,
) -> T {
    async fn send_typing_once(
        outbound: &Arc<dyn moltis_channels::plugin::ChannelOutbound>,
        chan_account_key: &str,
        chat_id: &str,
        op: &'static str,
        typing_failed: &mut bool,
    ) {
        match outbound.send_typing(chan_account_key, chat_id).await {
            Ok(()) => {
                if *typing_failed {
                    info!(
                        event = "telegram.typing.recovered",
                        op, chan_account_key, chat_id, "typing indicator recovered"
                    );
                    *typing_failed = false;
                }
            },
            Err(error) => {
                if !*typing_failed {
                    warn!(
                        event = "telegram.typing.failed",
                        op,
                        chan_account_key,
                        chat_id,
                        reason_code = "send_typing_failed",
                        error = %error,
                        "typing indicator failed"
                    );
                    *typing_failed = true;
                } else {
                    debug!(
                        event = "telegram.typing.failed",
                        op,
                        chan_account_key,
                        chat_id,
                        reason_code = "send_typing_failed",
                        "typing indicator still failing"
                    );
                }
            },
        }
    }

    let operation = operation;
    let typing_loop = async {
        let mut typing_failed = false;
        send_typing_once(
            &outbound,
            &chan_account_key,
            &chat_id,
            op,
            &mut typing_failed,
        )
        .await;

        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            keepalive.tick().await;
            send_typing_once(
                &outbound,
                &chan_account_key,
                &chat_id,
                op,
                &mut typing_failed,
            )
            .await;
        }
    };

    tokio::pin!(operation);
    tokio::pin!(typing_loop);

    tokio::select! {
        result = &mut operation => result,
        _ = &mut typing_loop => unreachable!("telegram typing loop must stay pending until the operation completes"),
    }
}

fn spawn_telegram_typing_loop_until(
    outbound: Arc<dyn moltis_channels::plugin::ChannelOutbound>,
    chan_account_key: String,
    chat_id: String,
    op: &'static str,
    completion: impl Future<Output = ()> + Send + 'static,
) {
    tokio::spawn(async move {
        async fn send_typing_once(
            outbound: &Arc<dyn moltis_channels::plugin::ChannelOutbound>,
            chan_account_key: &str,
            chat_id: &str,
            op: &'static str,
            typing_failed: &mut bool,
        ) {
            match outbound.send_typing(chan_account_key, chat_id).await {
                Ok(()) => {
                    if *typing_failed {
                        info!(
                            event = "telegram.typing.recovered",
                            op, chan_account_key, chat_id, "typing indicator recovered"
                        );
                        *typing_failed = false;
                    }
                },
                Err(error) => {
                    if !*typing_failed {
                        warn!(
                            event = "telegram.typing.failed",
                            op,
                            chan_account_key,
                            chat_id,
                            reason_code = "send_typing_failed",
                            error = %error,
                            "typing indicator failed"
                        );
                        *typing_failed = true;
                    } else {
                        debug!(
                            event = "telegram.typing.failed",
                            op,
                            chan_account_key,
                            chat_id,
                            reason_code = "send_typing_failed",
                            "typing indicator still failing"
                        );
                    }
                },
            }
        }

        let mut typing_failed = false;
        tokio::pin!(completion);

        tokio::select! {
            _ = &mut completion => return,
            _ = send_typing_once(
                &outbound,
                &chan_account_key,
                &chat_id,
                op,
                &mut typing_failed,
            ) => {}
        }

        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut completion => break,
                _ = keepalive.tick() => {
                    send_typing_once(
                        &outbound,
                        &chan_account_key,
                        &chat_id,
                        op,
                        &mut typing_failed,
                    )
                    .await;
                },
            }
        }
    });
}

/// Broadcasts channel events over the gateway WebSocket.
///
/// Uses a deferred `OnceCell` reference so the sink can be created before
/// `GatewayState` exists (same pattern as cron callbacks).
pub struct GatewayChannelEventSink {
    state: Arc<tokio::sync::OnceCell<Arc<GatewayState>>>,
}

impl GatewayChannelEventSink {
    pub fn new(state: Arc<tokio::sync::OnceCell<Arc<GatewayState>>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ChannelEventSink for GatewayChannelEventSink {
    async fn emit(&self, event: ChannelEvent) {
        if let Some(state) = self.state.get() {
            let payload = match serde_json::to_value(&event) {
                Ok(v) => v,
                Err(e) => {
                    warn!("failed to serialize channel event: {e}");
                    return;
                },
            };
            broadcast(
                state,
                "channel",
                payload,
                BroadcastOpts {
                    drop_if_slow: true,
                    ..Default::default()
                },
            )
            .await;
        }
    }

    async fn dispatch_to_chat(
        &self,
        text: &str,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        if let Some(state) = self.state.get() {
            let chan_chat_key = default_chan_chat_key(&reply_to);
            let session_id = if let Some(ref sm) = state.services.session_metadata {
                ensure_channel_session_id(&reply_to, sm).await
            } else {
                chan_chat_key.clone()
            };
            let trigger_id = crate::ids::new_trigger_id();

            // Broadcast a "chat" event so the web UI shows the user message
            // in real-time (like typing from the UI).
            // Include messageIndex so the client can deduplicate against history.
            let msg_index = if let Some(ref store) = state.services.session_store {
                store.count(&session_id).await.unwrap_or(0)
            } else {
                0
            };
            let payload = serde_json::json!({
                "state": "channel_user",
                "text": text,
                "channel": &meta,
                "sessionId": &session_id,
                "chanChatKey": &chan_chat_key,
                "messageIndex": msg_index,
            });
            broadcast(
                state,
                "chat",
                payload,
                BroadcastOpts {
                    drop_if_slow: true,
                    ..Default::default()
                },
            )
            .await;

            // Register the reply target so the chat "final" broadcast can
            // route the response back to the originating channel.
            state
                .push_channel_reply(&session_id, &trigger_id, reply_to.clone())
                .await;

            // Persist channel binding so web UI messages on this session
            // can be echoed back to the channel.
            if let Ok(binding_json) = serde_json::to_string(&reply_to)
                && let Some(ref session_meta) = state.services.session_metadata
            {
                // Ensure the session row exists and label it on first use.
                // `set_channel_binding` is an UPDATE, so the row must exist
                // before we can set the binding column.
                let entry = session_meta.get(&session_id).await;
                if entry.as_ref().is_none_or(|e| e.channel_binding.is_none()) {
                    let label = if reply_to.chan_type == ChannelType::Telegram {
                        let snapshots = state
                            .services
                            .channel
                            .telegram_bus_accounts_snapshot()
                            .await;
                        let u = crate::session_labels::resolve_telegram_bot_username(
                            &snapshots,
                            &reply_to.chan_account_key,
                        )
                        .or(reply_to.chan_user_name.as_deref());
                        crate::session_labels::format_telegram_session_label(
                            &reply_to.chan_account_key,
                            u,
                            &reply_to.chat_id,
                        )
                    } else {
                        session_id.clone()
                    };
                    let _ = session_meta.upsert(&session_id, Some(label)).await;
                }
                session_meta
                    .set_channel_binding(&session_id, Some(binding_json))
                    .await;
            }

            let chat = state.chat().await;
            let mut params = serde_json::json!({
                "text": text,
                "channel": &meta,
                "_sessionId": &session_id,
                "_chanChatKey": &chan_chat_key,
                "_triggerId": &trigger_id,
            });
            // Forward the channel's default model to chat.send() if configured.
            // If no channel model is set, check if the session already has a model.
            // If neither exists, assign the first registered model so the session
            // behaves the same as the web UI (which always sends an explicit model).
            if let Some(ref model) = meta.model {
                params["model"] = serde_json::json!(model);

                // Notify the user which model was assigned from the channel config
                // on the first message of a new session (no model set yet).
                let session_has_model = if let Some(ref sm) = state.services.session_metadata {
                    sm.get(&session_id).await.and_then(|e| e.model).is_some()
                } else {
                    false
                };
                if !session_has_model {
                    // Persist channel model on the session.
                    let _ = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &session_id,
                            "model": model,
                        }))
                        .await;

                    // Buffer model notification for the logbook instead of sending separately.
                    let display: String = if let Ok(models_val) = state.services.model.list().await
                        && let Some(models) = models_val.as_array()
                    {
                        models
                            .iter()
                            .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(model))
                            .and_then(|m| m.get("displayName").and_then(|v| v.as_str()))
                            .unwrap_or(model)
                            .to_string()
                    } else {
                        model.clone()
                    };
                    let msg = format!("Using {display}. Use /model to change.");
                    state
                        .push_channel_status_log(&session_id, &trigger_id, msg)
                        .await;
                }
            } else {
                let session_has_model = if let Some(ref sm) = state.services.session_metadata {
                    sm.get(&session_id).await.and_then(|e| e.model).is_some()
                } else {
                    false
                };
                if !session_has_model
                    && let Ok(models_val) = state.services.model.list().await
                    && let Some(models) = models_val.as_array()
                    && let Some(first) = models.first()
                    && let Some(id) = first.get("id").and_then(|v| v.as_str())
                {
                    params["model"] = serde_json::json!(id);
                    let _ = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &session_id,
                            "model": id,
                        }))
                        .await;

                    // Buffer model notification for the logbook.
                    let display = first
                        .get("displayName")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id);
                    let msg = format!("Using {display}. Use /model to change.");
                    state
                        .push_channel_status_log(&session_id, &trigger_id, msg)
                        .await;
                }
            }

            if let Some(outbound) = state.services.channel_outbound_arc() {
                let feedback_outbound = Arc::clone(&outbound);
                let chan_account_key = reply_to.chan_account_key.clone();
                let chat_id = reply_to.chat_id.clone();
                let reply_to_message_id = reply_to.message_id.clone();
                let failure_feedback_outbound = Arc::clone(&feedback_outbound);
                let failure_chan_account_key = chan_account_key.clone();
                let failure_chat_id = chat_id.clone();
                let failure_session_id = session_id.clone();
                let failure_trigger_id = trigger_id.clone();
                let send_result = run_with_telegram_typing_loop(
                    outbound,
                    chan_account_key.clone(),
                    chat_id.clone(),
                    "dispatch_to_chat_start",
                    async move {
                        let send_result = chat.send(params).await;
                        if let Err(e) = &send_result {
                            error!("channel dispatch_to_chat failed: {e}");
                            let error_msg =
                                "⚠️ Something went wrong. Please try again.".to_string();
                            if let Err(send_err) = failure_feedback_outbound
                                .send_text(
                                    &failure_chan_account_key,
                                    &failure_chat_id,
                                    &error_msg,
                                    reply_to_message_id.as_deref(),
                                )
                                .await
                            {
                                warn!("failed to send error back to channel: {send_err}");
                            }
                            let _ = state
                                .drain_channel_replies(&failure_session_id, &failure_trigger_id)
                                .await;
                            let _ = state
                                .drain_channel_status_log(&failure_session_id, &failure_trigger_id)
                                .await;
                        }
                        send_result
                    },
                )
                .await;

                match send_result {
                    Ok(result) => {
                        if let Some(run_id) = result.get("runId").and_then(|v| v.as_str()) {
                            let run_id = run_id.to_string();
                            let typing_chat = state.chat().await;
                            spawn_telegram_typing_loop_until(
                                feedback_outbound,
                                chan_account_key,
                                chat_id,
                                "dispatch_to_chat_run",
                                async move {
                                    if let Err(error) =
                                        typing_chat.wait_run_completion(&run_id).await
                                    {
                                        warn!(
                                            event = "telegram.typing.wait_failed",
                                            op = "dispatch_to_chat_run",
                                            run_id = run_id.as_str(),
                                            reason_code = "wait_run_completion_failed",
                                            error,
                                            "failed waiting for chat run completion"
                                        );
                                    }
                                },
                            );
                        } else if result.get("queued").and_then(|v| v.as_bool()) == Some(true) {
                            info!(
                                event = "telegram.typing.skipped",
                                op = "dispatch_to_chat_run",
                                session_id,
                                trigger_id,
                                reason_code = "queued_without_run_id",
                                "telegram typing lifecycle not extended because chat send queued without run id"
                            );
                        }
                    },
                    Err(_e) => {},
                }
            } else if let Err(e) = chat.send(params).await {
                error!("channel dispatch_to_chat failed: {e}");
                let _ = state.drain_channel_replies(&session_id, &trigger_id).await;
                let _ = state
                    .drain_channel_status_log(&session_id, &trigger_id)
                    .await;
            }
        } else {
            warn!("channel dispatch_to_chat: gateway not ready");
        }
    }

    async fn ingest_only(
        &self,
        text: &str,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        let Some(state) = self.state.get() else {
            warn!("channel ingest_only: gateway not ready");
            return;
        };

        let chan_chat_key = default_chan_chat_key(&reply_to);
        let session_id = if let Some(ref sm) = state.services.session_metadata {
            ensure_channel_session_id(&reply_to, sm).await
        } else {
            chan_chat_key.clone()
        };

        // Real-time UI: show the inbound message as "channel_user", but mark it ingest-only.
        let msg_index = if let Some(ref store) = state.services.session_store {
            store.count(&session_id).await.unwrap_or(0)
        } else {
            0
        };
        let payload = serde_json::json!({
            "state": "channel_user",
            "text": text,
            "channel": &meta,
            "sessionId": &session_id,
            "chanChatKey": &chan_chat_key,
            "messageIndex": msg_index,
            "ingestOnly": true,
        });
        broadcast(
            state,
            "chat",
            payload,
            BroadcastOpts {
                drop_if_slow: true,
                ..Default::default()
            },
        )
        .await;

        // Persist channel binding so the session is treated as channel-bound.
        if let Ok(binding_json) = serde_json::to_string(&reply_to)
            && let Some(ref session_meta) = state.services.session_metadata
        {
            // Ensure the session row exists and label it on first use.
            let entry = session_meta.get(&session_id).await;
            if entry.as_ref().is_none_or(|e| e.channel_binding.is_none()) {
                let label = if reply_to.chan_type == ChannelType::Telegram {
                    let snapshots = state
                        .services
                        .channel
                        .telegram_bus_accounts_snapshot()
                        .await;
                    let u = crate::session_labels::resolve_telegram_bot_username(
                        &snapshots,
                        &reply_to.chan_account_key,
                    )
                    .or(reply_to.chan_user_name.as_deref());
                    crate::session_labels::format_telegram_session_label(
                        &reply_to.chan_account_key,
                        u,
                        &reply_to.chat_id,
                    )
                } else {
                    session_id.clone()
                };
                let _ = session_meta.upsert(&session_id, Some(label)).await;
            }
            session_meta
                .set_channel_binding(&session_id, Some(binding_json))
                .await;
        }

        // If a channel default model is configured, persist it on first use so that
        // the next addressed message uses the expected model.
        if let Some(ref model) = meta.model {
            let session_has_model = if let Some(ref sm) = state.services.session_metadata {
                sm.get(&session_id).await.and_then(|e| e.model).is_some()
            } else {
                false
            };
            if !session_has_model {
                let _ = state
                    .services
                    .session
                    .patch(serde_json::json!({
                        "sessionId": &session_id,
                        "model": model,
                    }))
                    .await;
            }
        }

        let Some(store) = state.services.session_store.as_ref() else {
            warn!(
                session_id,
                "channel ingest_only: session store not available"
            );
            return;
        };

        let channel_meta = match serde_json::to_value(&meta) {
            Ok(v) => v,
            Err(e) => {
                warn!(session_id, error = %e, "channel ingest_only: failed to serialize channel meta");
                return;
            },
        };
        let user_msg = moltis_sessions::PersistedMessage::user_with_channel(text, channel_meta);

        if let Err(e) = store.append(&session_id, &user_msg.to_value()).await {
            warn!(session_id, error = %e, "channel ingest_only: failed to append message");
            return;
        }

        // Update session metadata counters/preview (best-effort).
        if let Some(ref sm) = state.services.session_metadata {
            sm.touch(&session_id, (msg_index + 1) as u32).await;
            if msg_index == 0 {
                let preview = extract_preview_from_value(&user_msg.to_value());
                sm.set_preview(&session_id, preview.as_deref()).await;
            }
        }
    }

    async fn request_disable_account(
        &self,
        channel_type: &str,
        account_handle: &str,
        reason: &str,
    ) {
        warn!(
            channel_type,
            account_handle,
            reason,
            "stopping local polling: detected bot already running on another instance"
        );

        if let Some(state) = self.state.get() {
            // Note: We intentionally do NOT remove the channel from the database.
            // The channel config should remain persisted so other moltis instances
            // sharing the same database can still use it. The polling loop will
            // cancel itself after this call returns.

            // Broadcast an event so the UI can update.
            let chan_type: moltis_channels::ChannelType = match channel_type.parse() {
                Ok(ct) => ct,
                Err(e) => {
                    warn!("request_disable_account: {e}");
                    return;
                },
            };
            let event = ChannelEvent::AccountDisabled {
                chan_type,
                chan_account_key: account_handle.to_string(),
                reason: reason.to_string(),
            };
            let payload = match serde_json::to_value(&event) {
                Ok(v) => v,
                Err(e) => {
                    warn!("failed to serialize AccountDisabled event: {e}");
                    return;
                },
            };
            broadcast(
                state,
                "channel",
                payload,
                BroadcastOpts {
                    drop_if_slow: true,
                    ..Default::default()
                },
            )
            .await;
        } else {
            warn!("request_disable_account: gateway not ready");
        }
    }

    async fn request_sender_approval(
        &self,
        _channel_type: &str,
        account_handle: &str,
        identifier: &str,
    ) {
        if let Some(state) = self.state.get() {
            let params = serde_json::json!({
                "chanAccountKey": account_handle,
                "identifier": identifier,
            });
            match state.services.channel.sender_approve(params).await {
                Ok(_) => {
                    info!(
                        account_handle,
                        identifier, "OTP self-approval: sender approved"
                    );
                },
                Err(e) => {
                    warn!(
                        account_handle,
                        identifier,
                        error = %e,
                        "OTP self-approval: failed to approve sender"
                    );
                },
            }
        } else {
            warn!("request_sender_approval: gateway not ready");
        }
    }

    async fn transcribe_voice(&self, audio_data: &[u8], format: &str) -> Result<String> {
        let state = self
            .state
            .get()
            .ok_or_else(|| anyhow!("gateway not ready"))?;

        let result = state
            .services
            .stt
            .transcribe_bytes(
                bytes::Bytes::copy_from_slice(audio_data),
                format,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| anyhow!("transcription failed: {}", e))?;

        let text = result
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("transcription result missing text"))?;

        Ok(text.to_string())
    }

    async fn voice_stt_available(&self) -> bool {
        let Some(state) = self.state.get() else {
            return false;
        };

        match state.services.stt.status().await {
            Ok(status) => status
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    async fn update_location(
        &self,
        reply_to: &ChannelReplyTarget,
        latitude: f64,
        longitude: f64,
    ) -> bool {
        let Some(state) = self.state.get() else {
            warn!("update_location: gateway not ready");
            return false;
        };

        let session_id = if let Some(ref sm) = state.services.session_metadata {
            ensure_channel_session_id(reply_to, sm).await
        } else {
            default_chan_chat_key(reply_to)
        };

        // Update in-memory cache.
        let geo = moltis_config::GeoLocation::now(latitude, longitude, None);
        state.inner.write().await.cached_location = Some(geo.clone());

        // Persist to USER.md (best-effort).
        let mut user = moltis_config::load_user().unwrap_or_default();
        user.location = Some(geo);
        if let Err(e) = moltis_config::save_user(&user) {
            warn!(error = %e, "failed to persist location to USER.md");
        }

        // Check for a pending tool-triggered location request.
        let pending_key = format!("channel_location:{session_id}");
        let pending = state
            .inner
            .write()
            .await
            .pending_invokes
            .remove(&pending_key);
        if let Some(invoke) = pending {
            let result = serde_json::json!({
                "location": {
                    "latitude": latitude,
                    "longitude": longitude,
                    "accuracy": 0.0,
                }
            });
            let _ = invoke.sender.send(result);
            info!(session_id, "resolved pending channel location request");
            return true;
        }

        false
    }

    async fn dispatch_to_chat_with_attachments(
        &self,
        text: &str,
        attachments: Vec<ChannelAttachment>,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        let attachment_count = attachments.len();
        let has_non_image_attachment = attachments
            .iter()
            .any(|attachment| !attachment.media_type.starts_with("image/"));
        let image_attachments: Vec<ChannelAttachment> = attachments
            .into_iter()
            .filter(|a| a.media_type.starts_with("image/"))
            .collect();
        if has_non_image_attachment || image_attachments.is_empty() {
            warn!(
                event = "channel.attachment.rejected",
                chan_type = ?reply_to.chan_type,
                chan_account_key = %reply_to.chan_account_key,
                chat_id = %reply_to.chat_id,
                attachment_count,
                reason_code = "non_image_attachment",
                "dispatch_to_chat_with_attachments rejected non-image attachments"
            );
            let Some(state) = self.state.get() else {
                warn!("channel dispatch_to_chat_with_attachments: gateway not ready");
                return;
            };
            if let Some(outbound) = state.services.channel_outbound_arc() {
                let error_msg = "⚠️ This attachment type isn't supported yet.".to_string();
                if let Err(_send_err) = outbound
                    .send_text(
                        &reply_to.chan_account_key,
                        &reply_to.chat_id,
                        &error_msg,
                        reply_to.message_id.as_deref(),
                    )
                    .await
                {
                    warn!(
                        event = "channel.user_feedback.failed",
                        chan_type = ?reply_to.chan_type,
                        chan_account_key = %reply_to.chan_account_key,
                        chat_id = %reply_to.chat_id,
                        reason_code = "send_text_failed",
                        "failed to send unsupported attachment feedback"
                    );
                }
            }
            return;
        }

        let Some(state) = self.state.get() else {
            warn!("channel dispatch_to_chat_with_attachments: gateway not ready");
            return;
        };

        let chan_chat_key = default_chan_chat_key(&reply_to);
        let session_id = if let Some(ref sm) = state.services.session_metadata {
            ensure_channel_session_id(&reply_to, sm).await
        } else {
            chan_chat_key.clone()
        };
        let trigger_id = crate::ids::new_trigger_id();

        // Build multimodal content array (OpenAI format)
        let mut content_parts: Vec<serde_json::Value> = Vec::new();

        // Add text part if not empty
        if !text.is_empty() {
            content_parts.push(serde_json::json!({
                "type": "text",
                "text": text,
            }));
        }

        // Add image parts
        for attachment in &image_attachments {
            let base64_data = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &attachment.data,
            );
            let data_uri = format!("data:{};base64,{}", attachment.media_type, base64_data);
            content_parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": data_uri,
                },
            }));
        }

        debug!(
            chan_chat_key = %chan_chat_key,
            text_len = text.len(),
            attachment_count = image_attachments.len(),
            "dispatching multimodal message to chat"
        );

        // Broadcast a "chat" event so the web UI shows the user message
        let msg_index = if let Some(ref store) = state.services.session_store {
            store.count(&session_id).await.unwrap_or(0)
        } else {
            0
        };

        // For the broadcast, just show the text portion
        let payload = serde_json::json!({
            "state": "channel_user",
            "text": if text.is_empty() { "[Image]" } else { text },
            "channel": &meta,
            "sessionId": &session_id,
            "chanChatKey": &chan_chat_key,
            "messageIndex": msg_index,
            "hasAttachments": true,
        });
        broadcast(
            state,
            "chat",
            payload,
            BroadcastOpts {
                drop_if_slow: true,
                ..Default::default()
            },
        )
        .await;

        // Register the reply target
        state
            .push_channel_reply(&session_id, &trigger_id, reply_to.clone())
            .await;

        // Persist channel binding (ensure session row exists first —
        // set_channel_binding is an UPDATE so the row must already be present).
        if let Ok(binding_json) = serde_json::to_string(&reply_to)
            && let Some(ref session_meta) = state.services.session_metadata
        {
            let entry = session_meta.get(&session_id).await;
            if entry.as_ref().is_none_or(|e| e.channel_binding.is_none()) {
                let label = if reply_to.chan_type == ChannelType::Telegram {
                    let snapshots = state
                        .services
                        .channel
                        .telegram_bus_accounts_snapshot()
                        .await;
                    let u = crate::session_labels::resolve_telegram_bot_username(
                        &snapshots,
                        &reply_to.chan_account_key,
                    )
                    .or(reply_to.chan_user_name.as_deref());
                    crate::session_labels::format_telegram_session_label(
                        &reply_to.chan_account_key,
                        u,
                        &reply_to.chat_id,
                    )
                } else {
                    session_id.clone()
                };
                let _ = session_meta.upsert(&session_id, Some(label)).await;
            }
            session_meta
                .set_channel_binding(&session_id, Some(binding_json))
                .await;
        }

        let chat = state.chat().await;
        let mut params = serde_json::json!({
            "content": content_parts,
            "channel": &meta,
            "_sessionId": &session_id,
            "_chanChatKey": &chan_chat_key,
            "_triggerId": &trigger_id,
        });

        // Forward the channel's default model if configured
        if let Some(ref model) = meta.model {
            params["model"] = serde_json::json!(model);

            let session_has_model = if let Some(ref sm) = state.services.session_metadata {
                sm.get(&session_id).await.and_then(|e| e.model).is_some()
            } else {
                false
            };
            if !session_has_model {
                let _ = state
                    .services
                    .session
                    .patch(serde_json::json!({
                        "sessionId": &session_id,
                        "model": model,
                    }))
                    .await;

                let display: String = if let Ok(models_val) = state.services.model.list().await
                    && let Some(models) = models_val.as_array()
                {
                    models
                        .iter()
                        .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(model))
                        .and_then(|m| m.get("displayName").and_then(|v| v.as_str()))
                        .unwrap_or(model)
                        .to_string()
                } else {
                    model.clone()
                };
                let msg = format!("Using {display}. Use /model to change.");
                state
                    .push_channel_status_log(&session_id, &trigger_id, msg)
                    .await;
            }
        } else {
            let session_has_model = if let Some(ref sm) = state.services.session_metadata {
                sm.get(&session_id).await.and_then(|e| e.model).is_some()
            } else {
                false
            };
            if !session_has_model
                && let Ok(models_val) = state.services.model.list().await
                && let Some(models) = models_val.as_array()
                && let Some(first) = models.first()
                && let Some(id) = first.get("id").and_then(|v| v.as_str())
            {
                params["model"] = serde_json::json!(id);
                let _ = state
                    .services
                    .session
                    .patch(serde_json::json!({
                        "sessionId": &session_id,
                        "model": id,
                    }))
                    .await;

                let display = first
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id);
                let msg = format!("Using {display}. Use /model to change.");
                state
                    .push_channel_status_log(&session_id, &trigger_id, msg)
                    .await;
            }
        }

        if let Some(outbound) = state.services.channel_outbound_arc() {
            let feedback_outbound = Arc::clone(&outbound);
            let chan_account_key = reply_to.chan_account_key.clone();
            let chat_id = reply_to.chat_id.clone();
            let reply_to_message_id = reply_to.message_id.clone();
            let failure_feedback_outbound = Arc::clone(&feedback_outbound);
            let failure_chan_account_key = chan_account_key.clone();
            let failure_chat_id = chat_id.clone();
            let failure_session_id = session_id.clone();
            let failure_trigger_id = trigger_id.clone();
            let send_result = run_with_telegram_typing_loop(
                outbound,
                chan_account_key.clone(),
                chat_id.clone(),
                "dispatch_to_chat_with_attachments_start",
                async move {
                    let send_result = chat.send(params).await;
                    if let Err(e) = &send_result {
                        error!("channel dispatch_to_chat_with_attachments failed: {e}");
                        let error_msg = "⚠️ Something went wrong. Please try again.".to_string();
                        if let Err(send_err) = failure_feedback_outbound
                            .send_text(
                                &failure_chan_account_key,
                                &failure_chat_id,
                                &error_msg,
                                reply_to_message_id.as_deref(),
                            )
                            .await
                        {
                            warn!("failed to send error back to channel: {send_err}");
                        }
                        let _ = state
                            .drain_channel_replies(&failure_session_id, &failure_trigger_id)
                            .await;
                        let _ = state
                            .drain_channel_status_log(&failure_session_id, &failure_trigger_id)
                            .await;
                    }
                    send_result
                },
            )
            .await;

            match send_result {
                Ok(result) => {
                    if let Some(run_id) = result.get("runId").and_then(|v| v.as_str()) {
                        let run_id = run_id.to_string();
                        let typing_chat = state.chat().await;
                        spawn_telegram_typing_loop_until(
                            feedback_outbound,
                            chan_account_key,
                            chat_id,
                            "dispatch_to_chat_with_attachments_run",
                            async move {
                                if let Err(error) = typing_chat.wait_run_completion(&run_id).await {
                                    warn!(
                                        event = "telegram.typing.wait_failed",
                                        op = "dispatch_to_chat_with_attachments_run",
                                        run_id = run_id.as_str(),
                                        reason_code = "wait_run_completion_failed",
                                        error,
                                        "failed waiting for chat run completion"
                                    );
                                }
                            },
                        );
                    } else if result.get("queued").and_then(|v| v.as_bool()) == Some(true) {
                        info!(
                            event = "telegram.typing.skipped",
                            op = "dispatch_to_chat_with_attachments_run",
                            session_id,
                            trigger_id,
                            reason_code = "queued_without_run_id",
                            "telegram typing lifecycle not extended because chat send queued without run id"
                        );
                    }
                },
                Err(_e) => {},
            }
        } else if let Err(e) = chat.send(params).await {
            error!("channel dispatch_to_chat_with_attachments failed: {e}");
            let _ = state.drain_channel_replies(&session_id, &trigger_id).await;
            let _ = state
                .drain_channel_status_log(&session_id, &trigger_id)
                .await;
        }
    }

    async fn dispatch_command(
        &self,
        command: &str,
        reply_to: ChannelReplyTarget,
    ) -> anyhow::Result<String> {
        let state = self
            .state
            .get()
            .ok_or_else(|| anyhow!("gateway not ready"))?;
        let session_metadata = state
            .services
            .session_metadata
            .as_ref()
            .ok_or_else(|| anyhow!("session metadata not available"))?;
        let chan_chat_key = default_chan_chat_key(&reply_to);
        let session_id = ensure_channel_session_id(&reply_to, session_metadata).await;
        let chat = state.chat().await;

        // Extract the command name (first word) and args (rest).
        let cmd = command.split_whitespace().next().unwrap_or("");
        let args = command[cmd.len()..].trim();

        match cmd {
            "new" => {
                // Create a new session with a fresh UUID key.
                let new_id = format!("session:{}", uuid::Uuid::new_v4());
                let binding_json = serde_json::to_string(&reply_to)
                    .map_err(|e| anyhow!("failed to serialize binding: {e}"))?;

                let label = if reply_to.chan_type == ChannelType::Telegram {
                    let snapshots = state
                        .services
                        .channel
                        .telegram_bus_accounts_snapshot()
                        .await;
                    let u = crate::session_labels::resolve_telegram_bot_username(
                        &snapshots,
                        &reply_to.chan_account_key,
                    )
                    .or(reply_to.chan_user_name.as_deref());
                    crate::session_labels::format_telegram_session_label(
                        &reply_to.chan_account_key,
                        u,
                        &reply_to.chat_id,
                    )
                } else {
                    new_id.clone()
                };

                // Create the new session entry with channel binding.
                session_metadata
                    .upsert(&new_id, Some(label))
                    .await
                    .map_err(|e| anyhow!("failed to create session: {e}"))?;
                session_metadata
                    .set_channel_binding(&new_id, Some(binding_json.clone()))
                    .await;

                // Ensure the old session also has a channel binding (for listing).
                let old_entry = session_metadata.get(&session_id).await;
                if old_entry
                    .as_ref()
                    .and_then(|e| e.channel_binding.as_ref())
                    .is_none()
                {
                    session_metadata
                        .set_channel_binding(&session_id, Some(binding_json))
                        .await;
                }

                // Update forward mapping.
                session_metadata
                    .set_active_session_id(
                        reply_to.chan_type.as_str(),
                        &reply_to.chan_account_key,
                        &reply_to.chat_id,
                        &new_id,
                    )
                    .await;

                info!(
                    chan_chat_key = %chan_chat_key,
                    old_session_id = %session_id,
                    new_session_id = %new_id,
                    "channel /new: created new session"
                );

                // Assign a model to the new session: prefer the channel's
                // configured model, fall back to the first registered model.
                let channel_model: Option<String> =
                    state.services.channel.status().await.ok().and_then(|v| {
                        let channels = v.get("channels")?.as_array()?;
                        channels
                            .iter()
                            .find(|ch| {
                                ch.get("chanAccountKey").and_then(|v| v.as_str())
                                    == Some(reply_to.chan_account_key.as_str())
                            })
                            .and_then(|ch| {
                                ch.get("config")?.get("model")?.as_str().map(String::from)
                            })
                    });

                let models_val = state.services.model.list().await.ok();
                let models = models_val.as_ref().and_then(|v| v.as_array());

                let (model_id, model_display): (Option<String>, String) = if let Some(ref cm) =
                    channel_model
                {
                    let d = models
                        .and_then(|ms| {
                            ms.iter()
                                .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(cm.as_str()))
                                .and_then(|m| m.get("displayName").and_then(|v| v.as_str()))
                        })
                        .unwrap_or(cm.as_str());
                    (Some(cm.clone()), d.to_string())
                } else if let Some(ms) = models
                    && let Some(first) = ms.first()
                    && let Some(id) = first.get("id").and_then(|v| v.as_str())
                {
                    let d = first
                        .get("displayName")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id);
                    (Some(id.to_string()), d.to_string())
                } else {
                    (None, String::new())
                };

                if let Some(ref mid) = model_id {
                    let _ = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &new_id,
                            "model": mid,
                        }))
                        .await;
                }

                // Notify web UI so the session list refreshes.
                broadcast(
                    state,
                    "session",
                    serde_json::json!({
                        "kind": "created",
                        "sessionId": &new_id,
                    }),
                    BroadcastOpts {
                        drop_if_slow: true,
                        ..Default::default()
                    },
                )
                .await;

                if model_display.is_empty() {
                    Ok("New session started.".to_string())
                } else {
                    Ok(format!(
                        "New session started. Using *{model_display}*. Use /model to change."
                    ))
                }
            },
            "clear" => {
                let params = serde_json::json!({ "_sessionId": &session_id });
                chat.clear(params).await.map_err(|e| anyhow!("{e}"))?;
                Ok("Session cleared.".to_string())
            },
            "compact" => {
                let params = serde_json::json!({ "_sessionId": &session_id });
                chat.compact(params).await.map_err(|e| anyhow!("{e}"))?;
                Ok("Session compacted.".to_string())
            },
            "context" => {
                let params = serde_json::json!({ "_sessionId": &session_id });
                let res = chat.context(params).await.map_err(|e| anyhow!("{e}"))?;

                // Telegram renders /context via a structured HTML card. Returning a stable,
                // versioned JSON contract avoids brittle markdown label parsing.
                //
                // Other channels may still expect plain text; for now we only switch Telegram.
                if reply_to.chan_type.as_str() == "telegram" {
                    return Ok(format_context_v1_payload(res));
                }

                let session_info = res.get("session").cloned().unwrap_or_default();
                let msg_count = session_info
                    .get("messageCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let provider = session_info
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let model = session_info
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");

                let token_debug = res.get("tokenDebug").cloned().unwrap_or_default();
                let last = token_debug.get("lastRequest").cloned().unwrap_or_default();
                let next = token_debug.get("nextRequest").cloned().unwrap_or_default();

                let last_in = last.get("inputTokens").and_then(|v| v.as_u64());
                let last_out = last.get("outputTokens").and_then(|v| v.as_u64());
                let last_cached = last.get("cachedTokens").and_then(|v| v.as_u64());

                let context_window = next.get("contextWindow").and_then(|v| v.as_u64());
                let prompt_est = next.get("promptInputToksEst").and_then(|v| v.as_u64());
                let compact_thred = next.get("autoCompactToksThred").and_then(|v| v.as_u64());
                let compact_pct = match (prompt_est, compact_thred) {
                    (Some(p), Some(t)) if t > 0 => Some(((p.saturating_mul(100)) + (t / 2)) / t),
                    _ => None,
                };

                // Sandbox section
                let sandbox = res.get("sandbox").cloned().unwrap_or_default();
                let sandbox_enabled = sandbox
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let sandbox_line = if sandbox_enabled {
                    let image = sandbox
                        .get("image")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    format!("**Sandbox:** on · `{image}`")
                } else {
                    "**Sandbox:** off".to_string()
                };

                // Skills/plugins section
                let skills = res
                    .get("skills")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let skills_line = if skills.is_empty() {
                    "**Plugins:** none".to_string()
                } else {
                    let names: Vec<_> = skills
                        .iter()
                        .filter_map(|s| s.get("name").and_then(|v| v.as_str()))
                        .collect();
                    format!("**Plugins:** {}", names.join(", "))
                };

                let last_line = format!(
                    "**Last:** in={} out={} cached={}",
                    last_in.map_or("?".to_string(), |v| v.to_string()),
                    last_out.map_or("?".to_string(), |v| v.to_string()),
                    last_cached.map_or("?".to_string(), |v| v.to_string()),
                );
                let cw_suffix = context_window
                    .map(|cw| format!(" (cw {cw})"))
                    .unwrap_or_default();
                let next_line = format!(
                    "**Next (est):** prompt={} · compact={}{}",
                    prompt_est.map_or("?".to_string(), |v| v.to_string()),
                    compact_pct.map_or("?".to_string(), |v| format!("{v}%")),
                    compact_thred.map_or_else(|| "".to_string(), |t| format!(" of {t}{cw_suffix}")),
                );

                Ok(format!(
                    "**SessionId:** `{session_id}`\n**ChanChatKey:** `{chan_chat_key}`\n**Messages:** {msg_count}\n**Provider:** {provider}\n**Model:** `{model}`\n{sandbox_line}\n{skills_line}\n{last_line}\n{next_line}"
                ))
            },
            "sessions" => {
                let sessions = session_metadata
                    .list_channel_sessions(
                        reply_to.chan_type.as_str(),
                        &reply_to.chan_account_key,
                        &reply_to.chat_id,
                    )
                    .await;

                if sessions.is_empty() {
                    return Ok("No sessions found. Send a message to start one.".to_string());
                }

                if args.is_empty() {
                    // List mode.
                    let mut lines = Vec::new();
                    for (i, s) in sessions.iter().enumerate() {
                        let label = s.label.as_deref().unwrap_or(&s.key);
                        let marker = if s.key == session_id {
                            " *"
                        } else {
                            ""
                        };
                        lines.push(format!(
                            "{}. {} ({} msgs){}",
                            i + 1,
                            label,
                            s.message_count,
                            marker,
                        ));
                    }
                    lines.push("\nUse /sessions N to switch.".to_string());
                    Ok(lines.join("\n"))
                } else {
                    // Switch mode.
                    let n: usize = args
                        .parse()
                        .map_err(|_| anyhow!("usage: /sessions [number]"))?;
                    if n == 0 || n > sessions.len() {
                        return Err(anyhow!("invalid session number. Use 1–{}.", sessions.len()));
                    }
                    let target_session = &sessions[n - 1];

                    // Update forward mapping.
                    session_metadata
                        .set_active_session_id(
                            reply_to.chan_type.as_str(),
                            &reply_to.chan_account_key,
                            &reply_to.chat_id,
                            &target_session.key,
                        )
                        .await;

                    let label = target_session
                        .label
                        .as_deref()
                        .unwrap_or(&target_session.key);
                    info!(
                        session = %target_session.key,
                        "channel /sessions: switched session"
                    );

                    broadcast(
                        state,
                        "session",
                        serde_json::json!({
                            "kind": "switched",
                            "sessionId": &target_session.key,
                        }),
                        BroadcastOpts {
                            drop_if_slow: true,
                            ..Default::default()
                        },
                    )
                    .await;

                    Ok(format!("Switched to: {label}"))
                }
            },
            "model" => {
                let models_val = state
                    .services
                    .model
                    .list()
                    .await
                    .map_err(|e| anyhow!("{e}"))?;
                let models = models_val
                    .as_array()
                    .ok_or_else(|| anyhow!("bad model list"))?;

                let current_model = {
                    let entry = session_metadata.get(&session_id).await;
                    entry.and_then(|e| e.model.clone())
                };

                if args.is_empty() {
                    // List unique providers.
                    let mut providers: Vec<String> = models
                        .iter()
                        .filter_map(|m| {
                            m.get("provider").and_then(|v| v.as_str()).map(String::from)
                        })
                        .collect();
                    providers.dedup();

                    if providers.len() <= 1 {
                        // Single provider — list models directly.
                        return Ok(format_model_list(models, current_model.as_deref(), None));
                    }

                    // Multiple providers — list them for selection.
                    // Prefix with "providers:" so Telegram handler knows.
                    let current_provider = current_model.as_deref().and_then(|cm| {
                        models.iter().find_map(|m| {
                            let id = m.get("id").and_then(|v| v.as_str())?;
                            if id == cm {
                                m.get("provider").and_then(|v| v.as_str()).map(String::from)
                            } else {
                                None
                            }
                        })
                    });
                    let mut lines = vec!["providers:".to_string()];
                    for (i, p) in providers.iter().enumerate() {
                        let count = models
                            .iter()
                            .filter(|m| m.get("provider").and_then(|v| v.as_str()) == Some(p))
                            .count();
                        let marker = if current_provider.as_deref() == Some(p) {
                            " *"
                        } else {
                            ""
                        };
                        lines.push(format!("{}. {} ({} models){}", i + 1, p, count, marker));
                    }
                    Ok(lines.join("\n"))
                } else if let Some(provider) = args.strip_prefix("provider:") {
                    // List models for a specific provider.
                    Ok(format_model_list(
                        models,
                        current_model.as_deref(),
                        Some(provider),
                    ))
                } else {
                    // Switch mode — arg is a 1-based global index.
                    let n: usize = args
                        .parse()
                        .map_err(|_| anyhow!("usage: /model [number]"))?;
                    if n == 0 || n > models.len() {
                        return Err(anyhow!("invalid model number. Use 1–{}.", models.len()));
                    }
                    let chosen = &models[n - 1];
                    let model_id = chosen
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("model has no id"))?;
                    let display = chosen
                        .get("displayName")
                        .and_then(|v| v.as_str())
                        .unwrap_or(model_id);

                    let patch_res = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &session_id,
                            "model": model_id,
                        }))
                        .await
                        .map_err(|e| anyhow!("{e}"))?;
                    let version = patch_res
                        .get("version")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    broadcast(
                        state,
                        "session",
                        serde_json::json!({
                            "kind": "patched",
                            "sessionId": &session_id,
                            "version": version,
                        }),
                        BroadcastOpts {
                            drop_if_slow: true,
                            ..Default::default()
                        },
                    )
                    .await;

                    Ok(format!("Model switched to: {display}"))
                }
            },
            "sandbox" => {
                let is_enabled = if let Some(ref router) = state.sandbox_router {
                    router.is_sandboxed(&chan_chat_key).await
                } else {
                    false
                };

                if args.is_empty() {
                    // Show current status and image list.
                    let default_img = if let Some(ref router) = state.sandbox_router {
                        router.default_image().await
                    } else {
                        moltis_tools::sandbox::DEFAULT_SANDBOX_IMAGE.to_string()
                    };
                    let current_image = if let Some(ref router) = state.sandbox_router {
                        router.resolve_image(&chan_chat_key, None).await
                    } else {
                        default_img.clone()
                    };

                    let status = if is_enabled {
                        "on"
                    } else {
                        "off"
                    };

                    // List available images.
                    let builder = moltis_tools::image_cache::DockerImageBuilder::new();
                    let cached = builder.list_cached().await.unwrap_or_default();

                    let mut images: Vec<(String, Option<String>)> =
                        vec![(default_img.clone(), None)];
                    for img in &cached {
                        if img.tag == default_img {
                            continue;
                        }
                        images.push((
                            img.tag.clone(),
                            Some(format!("{} ({})", img.skill_name, img.size)),
                        ));
                    }

                    let mut lines = vec![format!("status:{status}")];
                    for (i, (tag, subtitle)) in images.iter().enumerate() {
                        let marker = if *tag == current_image {
                            " *"
                        } else {
                            ""
                        };
                        let label = if let Some(sub) = subtitle {
                            format!("{}. {} — {}{}", i + 1, tag, sub, marker)
                        } else {
                            format!("{}. {}{}", i + 1, tag, marker)
                        };
                        lines.push(label);
                    }
                    Ok(lines.join("\n"))
                } else if args == "on" || args == "off" {
                    let new_val = args == "on";
                    let patch_res = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &session_id,
                            "sandboxEnabled": new_val,
                        }))
                        .await
                        .map_err(|e| anyhow!("{e}"))?;
                    let version = patch_res
                        .get("version")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    broadcast(
                        state,
                        "session",
                        serde_json::json!({
                            "kind": "patched",
                            "sessionId": &session_id,
                            "version": version,
                        }),
                        BroadcastOpts {
                            drop_if_slow: true,
                            ..Default::default()
                        },
                    )
                    .await;
                    let label = if new_val {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    Ok(format!("Sandbox {label}."))
                } else if let Some(rest) = args.strip_prefix("image ") {
                    let n: usize = rest
                        .parse()
                        .map_err(|_| anyhow!("usage: /sandbox image [number]"))?;

                    let default_img = if let Some(ref router) = state.sandbox_router {
                        router.default_image().await
                    } else {
                        moltis_tools::sandbox::DEFAULT_SANDBOX_IMAGE.to_string()
                    };
                    let builder = moltis_tools::image_cache::DockerImageBuilder::new();
                    let cached = builder.list_cached().await.unwrap_or_default();
                    let mut images: Vec<String> = vec![default_img];
                    for img in &cached {
                        if img.tag == images[0] {
                            continue;
                        }
                        images.push(img.tag.clone());
                    }

                    if n == 0 || n > images.len() {
                        return Err(anyhow!("invalid image number. Use 1–{}.", images.len()));
                    }
                    let chosen = &images[n - 1];

                    // If choosing the default image, clear the session override.
                    let patch_value = if n == 1 {
                        ""
                    } else {
                        chosen.as_str()
                    };
                    let patch_res = state
                        .services
                        .session
                        .patch(serde_json::json!({
                            "sessionId": &session_id,
                            "sandboxImage": patch_value,
                        }))
                        .await
                        .map_err(|e| anyhow!("{e}"))?;
                    let version = patch_res
                        .get("version")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    broadcast(
                        state,
                        "session",
                        serde_json::json!({
                            "kind": "patched",
                            "sessionId": &session_id,
                            "version": version,
                        }),
                        BroadcastOpts {
                            drop_if_slow: true,
                            ..Default::default()
                        },
                    )
                    .await;

                    Ok(format!("Image set to: {chosen}"))
                } else {
                    Err(anyhow!("usage: /sandbox [on|off|image N]"))
                }
            },
            _ => Err(anyhow!("unknown command: /{cmd}")),
        }
    }
}

/// Format a numbered model list, optionally filtered by provider.
///
/// Each line is: `N. DisplayName [provider] *` (where `*` marks the current model).
/// Uses the global index (across all models) so the switch command works with
/// the same numbering regardless of filtering.
fn format_model_list(
    models: &[serde_json::Value],
    current_model: Option<&str>,
    provider_filter: Option<&str>,
) -> String {
    let mut lines = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
        let display = m.get("displayName").and_then(|v| v.as_str()).unwrap_or(id);
        if let Some(filter) = provider_filter
            && provider != filter
        {
            continue;
        }
        let marker = if current_model == Some(id) {
            " *"
        } else {
            ""
        };
        lines.push(format!("{}. {} [{}]{}", i + 1, display, provider, marker));
    }
    lines.join("\n")
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        async_trait::async_trait,
        moltis_channels::{ChannelType, plugin::ChannelOutbound},
        moltis_common::types::ReplyPayload,
        moltis_telegram::config::TelegramBusAccountSnapshot,
        std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        tokio::sync::OnceCell,
    };

    async fn sqlite_metadata() -> Arc<moltis_sessions::metadata::SqliteSessionMetadata> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // The sessions table has a foreign key to projects(id); tests that use
        // in-memory sqlite must provide a minimal projects table.
        sqlx::query("CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        moltis_sessions::metadata::SqliteSessionMetadata::init(&pool)
            .await
            .unwrap();
        Arc::new(moltis_sessions::metadata::SqliteSessionMetadata::new(pool))
    }

    #[test]
    fn format_context_v1_payload_wraps_payload_with_versioned_contract() {
        let payload = serde_json::json!({
            "session": { "key": "telegram:bot:chat", "messageCount": 3 },
            "tokenDebug": { "lastRequest": { "inputTokens": 1 } }
        });
        let out = format_context_v1_payload(payload.clone());
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(
            parsed.get("format").and_then(|v| v.as_str()),
            Some("context.v1")
        );
        assert_eq!(parsed.get("payload"), Some(&payload));
    }

    #[test]
    fn channel_event_serialization() {
        let event = ChannelEvent::InboundMessage {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:bot1".into(),
            peer_id: "123".into(),
            username: Some("alice".into()),
            sender_name: Some("Alice".into()),
            message_count: Some(5),
            access_granted: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "inbound_message");
        assert_eq!(json["chanType"], "telegram");
        assert_eq!(json["chanAccountKey"], "telegram:bot1");
        assert_eq!(json["peerId"], "123");
        assert_eq!(json["username"], "alice");
        assert_eq!(json["senderName"], "Alice");
        assert_eq!(json["messageCount"], 5);
        assert_eq!(json["accessGranted"], true);
    }

    #[test]
    fn channel_session_key_format() {
        let target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:bot1".into(),
            chan_user_name: None,
            chat_id: "12345".into(),
            message_id: None,
        };
        assert_eq!(default_chan_chat_key(&target), "telegram:bot1:12345");
    }

    #[test]
    fn channel_session_key_strips_telegram_prefix_from_account_id() {
        let target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:8576199590".into(),
            chan_user_name: None,
            chat_id: "12345".into(),
            message_id: None,
        };
        assert_eq!(default_chan_chat_key(&target), "telegram:8576199590:12345");
    }

    #[test]
    fn channel_session_key_group() {
        let target = ChannelReplyTarget {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:bot1".into(),
            chan_user_name: None,
            chat_id: "-100999".into(),
            message_id: None,
        };
        assert_eq!(default_chan_chat_key(&target), "telegram:bot1:-100999");
    }

    #[test]
    fn channel_event_serialization_nulls() {
        let event = ChannelEvent::InboundMessage {
            chan_type: ChannelType::Telegram,
            chan_account_key: "telegram:bot1".into(),
            peer_id: "123".into(),
            username: None,
            sender_name: None,
            message_count: None,
            access_granted: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "inbound_message");
        assert!(json["username"].is_null());
        assert_eq!(json["accessGranted"], false);
    }

    struct ErrChatService;

    #[async_trait]
    impl crate::services::ChatService for ErrChatService {
        async fn send(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("boom".into())
        }

        async fn abort(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn cancel_queued(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "cleared": 0 }))
        }

        async fn history(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }

        async fn inject(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn clear(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn compact(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn raw_prompt(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn full_context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }
    }

    #[derive(Default)]
    struct RecordingOutbound {
        texts: tokio::sync::Mutex<Vec<(String, String, String, Option<String>)>>,
        typings: AtomicUsize,
    }

    #[async_trait]
    impl ChannelOutbound for RecordingOutbound {
        async fn send_text(
            &self,
            chan_account_key: &str,
            to: &str,
            text: &str,
            reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            self.texts.lock().await.push((
                chan_account_key.to_string(),
                to.to_string(),
                text.to_string(),
                reply_to.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> anyhow::Result<()> {
            self.typings.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ErrChatServiceWithLog {
        state: Arc<crate::state::GatewayState>,
    }

    #[async_trait]
    impl crate::services::ChatService for ErrChatServiceWithLog {
        async fn send(&self, params: serde_json::Value) -> crate::services::ServiceResult {
            let session_key = params
                .get("_sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("main");
            let trigger_id = params
                .get("_triggerId")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            self.state
                .push_channel_status_log(session_key, trigger_id, "tool status".to_string())
                .await;
            Err("boom".into())
        }

        async fn abort(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn cancel_queued(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "cleared": 0 }))
        }

        async fn history(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }

        async fn inject(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn clear(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn compact(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }

        async fn context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn raw_prompt(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn full_context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }
    }

    #[tokio::test]
    async fn dispatch_to_chat_immediate_failure_drains_reply_targets_and_logbook() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = rec.clone();

        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));

        state
            .set_chat(Arc::new(ErrChatServiceWithLog {
                state: Arc::clone(&state),
            }))
            .await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat(
            "hi",
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:acct".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: Some("1".into()),
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        let chan_chat_key = "telegram:acct:123";
        assert!(
            state
                .drain_all_channel_replies(chan_chat_key)
                .await
                .is_empty()
        );
        assert!(
            state
                .drain_all_channel_status_log(chan_chat_key)
                .await
                .is_empty()
        );

        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].0, "telegram:acct");
        assert_eq!(texts[0].1, "123");
        assert_eq!(texts[0].3.as_deref(), Some("1"));
        assert!(texts[0].2.starts_with("⚠️"));
        assert!(
            !texts[0].2.contains("boom"),
            "error message must not include internal error details"
        );
    }

    struct RecordingChatService {
        last_params: tokio::sync::Mutex<Option<serde_json::Value>>,
    }

    #[async_trait]
    impl crate::services::ChatService for RecordingChatService {
        async fn send(&self, params: serde_json::Value) -> crate::services::ServiceResult {
            *self.last_params.lock().await = Some(params);
            Ok(serde_json::json!({}))
        }

        async fn abort(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn cancel_queued(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "cleared": 0 }))
        }

        async fn history(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }

        async fn inject(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn clear(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn compact(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn raw_prompt(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn full_context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }
    }

    struct AsyncRunChatService {
        started: tokio::sync::Notify,
        finish: tokio::sync::Notify,
    }

    #[async_trait]
    impl crate::services::ChatService for AsyncRunChatService {
        async fn send(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            self.started.notify_one();
            Ok(serde_json::json!({ "runId": "run-1" }))
        }

        async fn wait_run_completion(&self, run_id: &str) -> crate::services::ServiceResult<()> {
            assert_eq!(run_id, "run-1");
            self.finish.notified().await;
            Ok(())
        }

        async fn abort(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn cancel_queued(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "cleared": 0 }))
        }

        async fn history(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }

        async fn inject(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn clear(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn compact(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({ "ok": true }))
        }

        async fn context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn raw_prompt(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!({}))
        }

        async fn full_context(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Ok(serde_json::json!([]))
        }
    }

    struct DelayedErrorFeedbackOutbound {
        typings: AtomicUsize,
        text_calls: AtomicUsize,
        feedback_gate: tokio::sync::Notify,
        texts: tokio::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ChannelOutbound for DelayedErrorFeedbackOutbound {
        async fn send_text(
            &self,
            _chan_account_key: &str,
            _to: &str,
            text: &str,
            _reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            self.texts.lock().await.push(text.to_string());
            self.feedback_gate.notified().await;
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> anyhow::Result<()> {
            self.typings.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatch_to_chat_with_attachments_rejects_non_image_media_types() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = rec.clone();
        let chat = Arc::new(RecordingChatService {
            last_params: tokio::sync::Mutex::new(None),
        });

        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        state.set_chat(chat.clone()).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat_with_attachments(
            "hi",
            vec![ChannelAttachment {
                media_type: "application/pdf".into(),
                data: vec![0, 1, 2, 3],
            }],
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:acct".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: Some("1".into()),
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        assert!(
            chat.last_params.lock().await.is_none(),
            "non-image attachments must not be dispatched to chat"
        );
        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1, "user should receive direct feedback");
        assert_eq!(texts[0].0, "telegram:acct");
        assert_eq!(texts[0].1, "123");
        assert_eq!(texts[0].3.as_deref(), Some("1"));
        assert!(
            texts[0].2.contains("attachment type isn't supported"),
            "expected unsupported attachment feedback, got {:?}",
            texts[0]
        );
    }

    #[tokio::test]
    async fn dispatch_to_chat_keeps_typing_for_entire_run() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = rec.clone();
        let chat = Arc::new(AsyncRunChatService {
            started: tokio::sync::Notify::new(),
            finish: tokio::sync::Notify::new(),
        });

        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        state.set_chat(chat.clone()).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = Arc::new(GatewayChannelEventSink::new(Arc::clone(&cell)));
        let started = chat.started.notified();
        let sink_task = {
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                sink.dispatch_to_chat(
                    "hi",
                    ChannelReplyTarget {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: "telegram:acct".into(),
                        chan_user_name: None,
                        chat_id: "123".into(),
                        message_id: Some("1".into()),
                    },
                    ChannelMessageMeta {
                        chan_type: ChannelType::Telegram,
                        sender_name: None,
                        username: None,
                        message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                        model: None,
                    },
                )
                .await;
            })
        };

        started.await;
        tokio::time::timeout(std::time::Duration::from_millis(50), sink_task)
            .await
            .expect("dispatch_to_chat should return after scheduling background typing")
            .unwrap();

        tokio::task::yield_now().await;
        let initial_typings = rec.typings.load(Ordering::SeqCst);
        assert!(
            initial_typings >= 1,
            "typing must start before dispatch_to_chat returns"
        );

        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        assert!(rec.typings.load(Ordering::SeqCst) > initial_typings);

        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        assert!(rec.typings.load(Ordering::SeqCst) >= 3);

        chat.finish.notify_waiters();
        tokio::task::yield_now().await;

        let final_typings = rec.typings.load(Ordering::SeqCst);
        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        assert_eq!(rec.typings.load(Ordering::SeqCst), final_typings);
    }

    #[tokio::test]
    async fn dispatch_to_chat_failure_keeps_typing_until_error_feedback_finishes() {
        let outbound = Arc::new(DelayedErrorFeedbackOutbound {
            typings: AtomicUsize::new(0),
            text_calls: AtomicUsize::new(0),
            feedback_gate: tokio::sync::Notify::new(),
            texts: tokio::sync::Mutex::new(Vec::new()),
        });
        let outbound_trait: Arc<dyn ChannelOutbound> = outbound.clone();

        let services =
            crate::services::GatewayServices::noop().with_channel_outbound(outbound_trait);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));

        state.set_chat(Arc::new(ErrChatService)).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = Arc::new(GatewayChannelEventSink::new(Arc::clone(&cell)));
        let sink_task = {
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                sink.dispatch_to_chat(
                    "hi",
                    ChannelReplyTarget {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: "telegram:acct".into(),
                        chan_user_name: None,
                        chat_id: "123".into(),
                        message_id: Some("1".into()),
                    },
                    ChannelMessageMeta {
                        chan_type: ChannelType::Telegram,
                        sender_name: None,
                        username: None,
                        message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                        model: None,
                    },
                )
                .await;
            })
        };

        tokio::task::yield_now().await;
        assert_eq!(outbound.typings.load(Ordering::SeqCst), 1);
        assert_eq!(outbound.text_calls.load(Ordering::SeqCst), 1);

        tokio::time::sleep(
            TELEGRAM_TYPING_KEEPALIVE_INTERVAL + std::time::Duration::from_millis(5),
        )
        .await;
        assert!(
            outbound.typings.load(Ordering::SeqCst) >= 2,
            "typing must remain active until error feedback finishes"
        );

        outbound.feedback_gate.notify_one();
        sink_task.await.unwrap();

        let texts = outbound.texts.lock().await;
        assert_eq!(texts.len(), 1);
        assert!(texts[0].starts_with("⚠️"));
    }

    struct BlockingTypingOutbound {
        send_typing_started: tokio::sync::Notify,
    }

    #[async_trait]
    impl ChannelOutbound for BlockingTypingOutbound {
        async fn send_text(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _text: &str,
            _reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_media(
            &self,
            _chan_account_key: &str,
            _to: &str,
            _payload: &ReplyPayload,
            _reply_to: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_typing(&self, _chan_account_key: &str, _to: &str) -> anyhow::Result<()> {
            self.send_typing_started.notify_waiters();
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatch_to_chat_run_is_not_blocked_by_slow_typing_request() {
        let outbound = Arc::new(BlockingTypingOutbound {
            send_typing_started: tokio::sync::Notify::new(),
        });
        let outbound_trait: Arc<dyn ChannelOutbound> = outbound.clone();
        let chat = Arc::new(AsyncRunChatService {
            started: tokio::sync::Notify::new(),
            finish: tokio::sync::Notify::new(),
        });

        let services =
            crate::services::GatewayServices::noop().with_channel_outbound(outbound_trait);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        state.set_chat(chat.clone()).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = Arc::new(GatewayChannelEventSink::new(Arc::clone(&cell)));
        let started = chat.started.notified();
        let typing_started = outbound.send_typing_started.notified();

        let sink_task = {
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                sink.dispatch_to_chat(
                    "hi",
                    ChannelReplyTarget {
                        chan_type: ChannelType::Telegram,
                        chan_account_key: "telegram:acct".into(),
                        chan_user_name: None,
                        chat_id: "123".into(),
                        message_id: Some("1".into()),
                    },
                    ChannelMessageMeta {
                        chan_type: ChannelType::Telegram,
                        sender_name: None,
                        username: None,
                        message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                        model: None,
                    },
                )
                .await;
            })
        };

        tokio::time::timeout(std::time::Duration::from_millis(50), typing_started)
            .await
            .expect("typing loop must start");
        tokio::time::timeout(std::time::Duration::from_millis(50), started)
            .await
            .expect("chat send must still start while typing is blocked");
        tokio::time::timeout(std::time::Duration::from_millis(50), sink_task)
            .await
            .expect("dispatch_to_chat must not wait for run completion or blocked typing send")
            .unwrap();

        chat.finish.notify_waiters();
    }

    #[tokio::test]
    async fn dispatch_to_chat_with_attachments_rejects_mixed_image_and_non_image_media_types() {
        let rec = Arc::new(RecordingOutbound::default());
        let outbound: Arc<dyn ChannelOutbound> = rec.clone();
        let chat = Arc::new(RecordingChatService {
            last_params: tokio::sync::Mutex::new(None),
        });

        let services = crate::services::GatewayServices::noop().with_channel_outbound(outbound);
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        state.set_chat(chat.clone()).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat_with_attachments(
            "hi",
            vec![
                ChannelAttachment {
                    media_type: "image/png".into(),
                    data: vec![137, 80, 78, 71],
                },
                ChannelAttachment {
                    media_type: "application/pdf".into(),
                    data: vec![0, 1, 2, 3],
                },
            ],
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:acct".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: Some("1".into()),
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        assert!(
            chat.last_params.lock().await.is_none(),
            "mixed attachments must not be partially dispatched to chat"
        );
        let texts = rec.texts.lock().await;
        assert_eq!(texts.len(), 1, "user should receive direct feedback");
        assert!(
            texts[0].2.contains("attachment type isn't supported"),
            "expected unsupported attachment feedback, got {:?}",
            texts[0]
        );
    }

    #[tokio::test]
    async fn dispatch_to_chat_with_attachments_accepts_image_media_types() {
        let chat = Arc::new(RecordingChatService {
            last_params: tokio::sync::Mutex::new(None),
        });

        let services = crate::services::GatewayServices::noop();
        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        state.set_chat(chat.clone()).await;

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat_with_attachments(
            "hi",
            vec![ChannelAttachment {
                media_type: "image/png".into(),
                data: vec![137, 80, 78, 71],
            }],
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:acct".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: Some("1".into()),
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        let params = chat
            .last_params
            .lock()
            .await
            .clone()
            .expect("captured params");
        let content = params
            .get("content")
            .and_then(|v| v.as_array())
            .expect("multimodal content must be present for images");
        assert!(
            content
                .iter()
                .any(|p| p.get("type").and_then(|v| v.as_str()) == Some("image_url")),
            "expected an image_url part"
        );
        let first_image = content
            .iter()
            .find(|p| p.get("type").and_then(|v| v.as_str()) == Some("image_url"))
            .unwrap();
        let url = first_image
            .get("image_url")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    struct SnapshotChannelService {
        snapshots: Vec<TelegramBusAccountSnapshot>,
    }

    #[async_trait]
    impl crate::services::ChannelService for SnapshotChannelService {
        async fn status(&self) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn logout(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn send(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn add(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn remove(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn update(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn senders_list(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn sender_approve(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }
        async fn sender_deny(&self, _params: serde_json::Value) -> crate::services::ServiceResult {
            Err("not implemented".into())
        }

        async fn telegram_bus_accounts_snapshot(&self) -> Vec<TelegramBusAccountSnapshot> {
            self.snapshots.clone()
        }
    }

    #[tokio::test]
    async fn dispatch_to_chat_labels_telegram_session_with_bot_username_and_dm_chat_id() {
        let metadata = sqlite_metadata().await;
        metadata
            .upsert("session:test", Some("ok".into()))
            .await
            .expect("metadata upsert works");
        assert!(
            metadata.get("session:test").await.is_some(),
            "metadata get works"
        );

        let mut services = crate::services::GatewayServices::noop()
            .with_chat(Arc::new(ErrChatService))
            .with_session_metadata(Arc::clone(&metadata));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                chan_user_name: Some("lovely_apple_bot".into()),
                relay_chain_enabled: false,
                relay_hop_limit: 0,
                epoch_relay_budget: 128,
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
                group_session_transcript_format:
                    moltis_telegram::config::GroupSessionTranscriptFormat::Legacy,
            }],
        });

        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));
        assert!(
            state.services.session_metadata.is_some(),
            "test must have session_metadata wired"
        );

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat(
            "hi",
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:845".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: None,
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        let session_id = metadata
            .get_active_session_id("telegram", "telegram:845", "123")
            .await
            .expect("active session id");
        let entry = metadata.get(&session_id).await.expect("session row");
        assert_eq!(
            entry.label.as_deref(),
            Some("TG @lovely_apple_bot · dm:123")
        );
    }

    #[tokio::test]
    async fn dispatch_to_chat_labels_telegram_session_group_chat_as_grp() {
        let metadata = sqlite_metadata().await;

        let mut services = crate::services::GatewayServices::noop()
            .with_chat(Arc::new(ErrChatService))
            .with_session_metadata(Arc::clone(&metadata));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                chan_user_name: Some("lovely_apple_bot".into()),
                relay_chain_enabled: false,
                relay_hop_limit: 0,
                epoch_relay_budget: 128,
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
                group_session_transcript_format:
                    moltis_telegram::config::GroupSessionTranscriptFormat::Legacy,
            }],
        });

        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat(
            "hi",
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:845".into(),
                chan_user_name: None,
                chat_id: "-100".into(),
                message_id: None,
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        let session_id = metadata
            .get_active_session_id("telegram", "telegram:845", "-100")
            .await
            .expect("active session id");
        let entry = metadata.get(&session_id).await.expect("session row");
        assert_eq!(
            entry.label.as_deref(),
            Some("TG @lovely_apple_bot · grp:-100")
        );
    }

    #[tokio::test]
    async fn dispatch_to_chat_labels_telegram_session_falls_back_to_chan_user_id_when_username_missing()
     {
        let metadata = sqlite_metadata().await;

        let mut services = crate::services::GatewayServices::noop()
            .with_chat(Arc::new(ErrChatService))
            .with_session_metadata(Arc::clone(&metadata));
        services.channel = Arc::new(SnapshotChannelService {
            snapshots: vec![TelegramBusAccountSnapshot {
                account_handle: "telegram:845".into(),
                chan_user_name: None,
                relay_chain_enabled: false,
                relay_hop_limit: 0,
                epoch_relay_budget: 128,
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
                group_session_transcript_format:
                    moltis_telegram::config::GroupSessionTranscriptFormat::Legacy,
            }],
        });

        let state = Arc::new(crate::state::GatewayState::new(
            crate::auth::ResolvedAuth {
                mode: crate::auth::AuthMode::Token,
                token: None,
                password: None,
            },
            services,
        ));

        let cell: Arc<OnceCell<Arc<crate::state::GatewayState>>> = Arc::new(OnceCell::new());
        if cell.set(Arc::clone(&state)).is_err() {
            panic!("failed to set gateway state oncecell for test");
        }
        let sink = GatewayChannelEventSink::new(Arc::clone(&cell));

        sink.dispatch_to_chat(
            "hi",
            ChannelReplyTarget {
                chan_type: ChannelType::Telegram,
                chan_account_key: "telegram:845".into(),
                chan_user_name: None,
                chat_id: "123".into(),
                message_id: None,
            },
            ChannelMessageMeta {
                chan_type: ChannelType::Telegram,
                sender_name: None,
                username: None,
                message_kind: Some(moltis_channels::plugin::ChannelMessageKind::Text),
                model: None,
            },
        )
        .await;

        let session_id = metadata
            .get_active_session_id("telegram", "telegram:845", "123")
            .await
            .expect("active session id");
        let entry = metadata.get(&session_id).await.expect("session row");
        assert_eq!(entry.label.as_deref(), Some("TG 845 · dm:123"));
    }
}

use std::sync::Arc;

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

            // Send a repeating "typing" indicator every 4s until chat.send()
            // completes. Telegram's typing status expires after ~5s.
            let send_result = if let Some(outbound) = state.services.channel_outbound_arc() {
                let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
                let chan_account_key = reply_to.chan_account_key.clone();
                let chat_id = reply_to.chat_id.clone();
                tokio::spawn(async move {
                    debug!(
                        chan_account_key = chan_account_key,
                        chat_id = chat_id,
                        "starting typing indicator loop"
                    );
                    loop {
                        if let Err(e) = outbound.send_typing(&chan_account_key, &chat_id).await {
                            debug!(
                                chan_account_key = chan_account_key,
                                chat_id = chat_id,
                                "typing indicator failed: {e}"
                            );
                        } else {
                            debug!(
                                chan_account_key = chan_account_key,
                                chat_id = chat_id,
                                "typing indicator sent"
                            );
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {
                                debug!(
                                    chan_account_key = chan_account_key,
                                    chat_id = chat_id,
                                    "typing loop: 4s elapsed, sending again"
                                );
                            },
                            _ = &mut done_rx => {
                                debug!(
                                    chan_account_key = chan_account_key,
                                    chat_id = chat_id,
                                    "typing loop: chat completed, stopping"
                                );
                                break;
                            },
                        }
                    }
                });
                let result = chat.send(params).await;
                let _ = done_tx.send(());
                result
            } else {
                chat.send(params).await
            };

            if let Err(e) = send_result {
                error!("channel dispatch_to_chat failed: {e}");
                // Send the error back to the originating channel so the user
                // knows something went wrong.
                if let Some(outbound) = state.services.channel_outbound_arc() {
                    let error_msg = format!("⚠️ {e}");
                    if let Err(send_err) = outbound
                        .send_text(
                            &reply_to.chan_account_key,
                            &reply_to.chat_id,
                            &error_msg,
                            reply_to.message_id.as_deref(),
                        )
                        .await
                    {
                        warn!("failed to send error back to channel: {send_err}");
                    }
                }
                // Drain any pending channel delivery state (reply targets + logbook)
                // so later replies can't get cross-wired to this failed trigger.
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
        if attachments.is_empty() {
            // No attachments, use the regular dispatch
            self.dispatch_to_chat(text, reply_to, meta).await;
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
        for attachment in &attachments {
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
            attachment_count = attachments.len(),
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

        // Send typing indicator and dispatch to chat
        let send_result = if let Some(outbound) = state.services.channel_outbound_arc() {
            let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
            let chan_account_key = reply_to.chan_account_key.clone();
            let chat_id = reply_to.chat_id.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) = outbound.send_typing(&chan_account_key, &chat_id).await {
                        debug!(chan_account_key, chat_id, "typing indicator failed: {e}");
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {},
                        _ = &mut done_rx => break,
                    }
                }
            });
            let result = chat.send(params).await;
            let _ = done_tx.send(());
            result
        } else {
            chat.send(params).await
        };

        if let Err(e) = send_result {
            error!("channel dispatch_to_chat_with_attachments failed: {e}");
            if let Some(outbound) = state.services.channel_outbound_arc() {
                let error_msg = format!("⚠️ {e}");
                if let Err(send_err) = outbound
                    .send_text(
                        &reply_to.chan_account_key,
                        &reply_to.chat_id,
                        &error_msg,
                        reply_to.message_id.as_deref(),
                    )
                    .await
                {
                    warn!("failed to send error back to channel: {send_err}");
                }
            }
            // Drain any pending channel delivery state (reply targets + logbook)
            // so later replies can't get cross-wired to this failed trigger.
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

        async fn full_context(
            &self,
            _params: serde_json::Value,
        ) -> crate::services::ServiceResult {
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
        assert!(state.drain_all_channel_replies(chan_chat_key).await.is_empty());
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
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
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
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
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
                relay_strictness: moltis_telegram::config::RelayStrictness::Strict,
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

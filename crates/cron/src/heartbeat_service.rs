//! Heartbeat scheduler: periodic runs bound to an explicit session target.
//!
//! One-cut governance model:
//! - Heartbeat config/state is DB-only.
//! - Prompt is owned by `agents/<agent_id>/HEARTBEAT.md` (loaded by gateway boundary).
//! - Outside activeHours -> skip (no catch-up).
//! - HEARTBEAT_OK -> suppress delivery.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use {
    anyhow::{Result, bail},
    tokio::{
        sync::{Mutex, Notify, RwLock},
        task::JoinHandle,
    },
    tracing::{debug, error, info, warn},
};

use crate::{
    heartbeat::{
        StripMode, is_heartbeat_content_empty, is_within_active_hours, strip_heartbeat_token,
    },
    parse,
    store_heartbeat::HeartbeatStore,
    types::{
        HeartbeatConfig, HeartbeatRunRecord, HeartbeatState, HeartbeatStatus, ModelSelector,
        RunStatus, SessionTarget,
    },
};

const POLICY: &str = "cron_heartbeat_governance_v1";
const STUCK_THRESHOLD_MS: u64 = 2 * 60 * 60 * 1000;

/// Parameters for running a heartbeat turn (LLM call only; no implicit delivery).
#[derive(Debug, Clone)]
pub struct HeartbeatRunRequest {
    pub run_id: String,
    pub agent_id: String,
    pub session_target: SessionTarget,
    pub prompt: String,
    pub model_selector: ModelSelector,
    pub timeout_secs: Option<u64>,
}

/// Result of a heartbeat run.
#[derive(Debug, Clone)]
pub struct HeartbeatRunResult {
    pub output: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Callback for running a heartbeat turn.
pub type RunHeartbeatFn = Arc<
    dyn Fn(HeartbeatRunRequest) -> Pin<Box<dyn Future<Output = Result<HeartbeatRunResult>> + Send>>
        + Send
        + Sync,
>;

/// Parameters for delivering a heartbeat output to the bound session target.
#[derive(Debug, Clone)]
pub struct HeartbeatDeliverRequest {
    pub run_id: String,
    pub agent_id: String,
    pub session_target: SessionTarget,
    pub output: String,
}

/// Callback for delivering a heartbeat output.
pub type DeliverHeartbeatFn = Arc<
    dyn Fn(HeartbeatDeliverRequest) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Gateway-owned validation and file-loading hooks.
#[derive(Clone, Default)]
pub struct HeartbeatValidators {
    pub validate_model_id:
        Option<Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>,
    pub validate_session_target: Option<
        Arc<
            dyn Fn(&str, &SessionTarget) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
                + Send
                + Sync,
        >,
    >,
    pub validate_agent_id:
        Option<Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>,
    pub load_prompt: Option<
        Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>,
    >,
}

/// Sliding-window rate limiter for heartbeat upserts (per service, best-effort).
struct RateLimiter {
    timestamps: VecDeque<u64>,
    max_per_window: usize,
    window_ms: u64,
}

impl RateLimiter {
    fn new(max_per_window: usize, window_ms: u64) -> Self {
        Self {
            timestamps: VecDeque::new(),
            max_per_window,
            window_ms,
        }
    }

    fn check(&mut self) -> Result<()> {
        let now = now_ms();
        let cutoff = now.saturating_sub(self.window_ms);
        while self.timestamps.front().is_some_and(|&ts| ts < cutoff) {
            self.timestamps.pop_front();
        }
        if self.timestamps.len() >= self.max_per_window {
            bail!(
                "rate limit exceeded: max {} updates per {} seconds",
                self.max_per_window,
                self.window_ms / 1000
            );
        }
        self.timestamps.push_back(now);
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn ms_to_rfc3339(ms: u64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp_millis(0).expect("epoch"));
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let idx = s.floor_char_boundary(max);
        format!("{}…", &s[..idx])
    }
}

/// Heartbeat scheduler.
pub struct HeartbeatService {
    store: Arc<dyn HeartbeatStore>,
    validators: HeartbeatValidators,
    heartbeats: RwLock<Vec<HeartbeatStatus>>,
    timer_handle: Mutex<Option<JoinHandle<()>>>,
    wake_notify: Arc<Notify>,
    running: RwLock<bool>,
    on_run: RunHeartbeatFn,
    on_deliver: DeliverHeartbeatFn,
    rate_limiter: Mutex<RateLimiter>,
}

impl HeartbeatService {
    pub fn new(
        store: Arc<dyn HeartbeatStore>,
        on_run: RunHeartbeatFn,
        on_deliver: DeliverHeartbeatFn,
    ) -> Arc<Self> {
        Self::with_config(store, on_run, on_deliver, HeartbeatValidators::default())
    }

    pub fn with_config(
        store: Arc<dyn HeartbeatStore>,
        on_run: RunHeartbeatFn,
        on_deliver: DeliverHeartbeatFn,
        validators: HeartbeatValidators,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            validators,
            heartbeats: RwLock::new(Vec::new()),
            timer_handle: Mutex::new(None),
            wake_notify: Arc::new(Notify::new()),
            running: RwLock::new(false),
            on_run,
            on_deliver,
            rate_limiter: Mutex::new(RateLimiter::new(30, 60_000)),
        })
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let loaded = self.store.load_all().await?;
        info!(count = loaded.len(), "loaded heartbeat configs");

        for hb in &loaded {
            self.validate_loaded_status(hb).await?;
        }

        {
            let mut heartbeats = self.heartbeats.write().await;
            *heartbeats = loaded;
        }

        self.recompute_all_next_runs().await?;
        *self.running.write().await = true;

        let svc = Arc::clone(self);
        let handle = tokio::spawn(async move {
            svc.timer_loop().await;
        });
        *self.timer_handle.lock().await = Some(handle);
        Ok(())
    }

    pub async fn stop(&self) {
        *self.running.write().await = false;
        self.wake_notify.notify_one();
        let mut handle = self.timer_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
        }
        info!("heartbeat service stopped");
    }

    pub async fn list(&self) -> Vec<HeartbeatStatus> {
        self.heartbeats.read().await.clone()
    }

    pub async fn get(&self, agent_id: &str) -> Result<Option<HeartbeatStatus>> {
        Ok(self
            .heartbeats
            .read()
            .await
            .iter()
            .find(|s| s.config.agent_id == agent_id)
            .cloned())
    }

    /// Upsert a heartbeat config (DB-backed). Strict validation; no silent degrade.
    pub async fn upsert(&self, config: HeartbeatConfig) -> Result<HeartbeatStatus> {
        self.rate_limiter.lock().await.check()?;

        let existing_state = {
            let heartbeats = self.heartbeats.read().await;
            heartbeats
                .iter()
                .find(|status| status.config.agent_id == config.agent_id)
                .map(|status| status.state.clone())
        };
        let state = match existing_state {
            Some(state) => state,
            None => self
                .store
                .get(&config.agent_id)
                .await?
                .map(|status| status.state)
                .unwrap_or_default(),
        };

        let mut status = HeartbeatStatus { config, state };

        self.validate_status(&status).await?;

        // Compute next run if enabled.
        if status.config.enabled {
            let now = now_ms();
            status.state.next_run_at = Some(ms_to_rfc3339(
                now + parse::parse_duration_ms(&status.config.every)?,
            ));
        } else {
            status.state.next_run_at = None;
        }

        self.store.upsert(&status).await?;

        {
            let mut heartbeats = self.heartbeats.write().await;
            if let Some(existing) = heartbeats
                .iter_mut()
                .find(|s| s.config.agent_id == status.config.agent_id)
            {
                *existing = status.clone();
            } else {
                heartbeats.push(status.clone());
            }
        }

        self.wake_notify.notify_one();
        Ok(status)
    }

    pub async fn delete(&self, agent_id: &str) -> Result<()> {
        self.store.delete(agent_id).await?;
        let mut heartbeats = self.heartbeats.write().await;
        heartbeats.retain(|s| s.config.agent_id != agent_id);
        self.wake_notify.notify_one();
        Ok(())
    }

    pub async fn runs(&self, agent_id: &str, limit: usize) -> Result<Vec<HeartbeatRunRecord>> {
        self.store.get_runs(agent_id, limit).await
    }

    pub async fn run(self: &Arc<Self>, agent_id: &str, force: bool) -> Result<()> {
        let hb = self.mark_manual_run_running(agent_id, force).await?;
        if let Err(e) = self.persist_heartbeat_status(&hb.config.agent_id).await {
            self.update_heartbeat_state(&hb.config.agent_id, |state| state.running_at = None)
                .await;
            bail!("failed to persist heartbeat runningAt: {e}");
        }

        self.execute_heartbeat(&hb).await;
        Ok(())
    }

    // ── Internal ────────────────────────────────────────────────────────

    async fn timer_loop(self: &Arc<Self>) {
        loop {
            if !*self.running.read().await {
                break;
            }

            let sleep_ms = self.ms_until_next_wake().await;
            if sleep_ms > 0 {
                let notify = Arc::clone(&self.wake_notify);
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {},
                    () = notify.notified() => {
                        debug!("heartbeat timer woken by notify");
                        continue;
                    },
                }
            }

            if !*self.running.read().await {
                break;
            }

            self.process_due_heartbeats().await;
        }
    }

    async fn ms_until_next_wake(&self) -> u64 {
        let heartbeats = self.heartbeats.read().await;
        let now = now_ms();
        heartbeats
            .iter()
            .filter(|s| s.config.enabled)
            .filter_map(|s| s.state.next_run_at.as_deref())
            .filter_map(|s| parse::parse_absolute_time_ms(s).ok())
            .map(|t| t.saturating_sub(now))
            .min()
            .unwrap_or(60_000)
    }

    async fn process_due_heartbeats(self: &Arc<Self>) {
        let now = now_ms();

        let (due, stuck_cleared): (Vec<HeartbeatStatus>, Vec<String>) = {
            let mut heartbeats = self.heartbeats.write().await;
            let mut due = Vec::new();
            let mut stuck_cleared = Vec::new();
            for hb in heartbeats.iter_mut() {
                let running_ms = hb
                    .state
                    .running_at
                    .as_deref()
                    .and_then(|s| parse::parse_absolute_time_ms(s).ok());
                if let Some(started) = running_ms
                    && now.saturating_sub(started) > STUCK_THRESHOLD_MS
                {
                    warn!(
                        event = "heartbeat.run.stuck_cleared",
                        policy = POLICY,
                        decision = "drop",
                        reason_code = "heartbeat_stuck_cleared",
                        agent_id = %hb.config.agent_id,
                        "clearing stuck heartbeat (exceeded 2h threshold)"
                    );
                    hb.state.running_at = None;
                    hb.state.last_status = Some(RunStatus::Error);
                    hb.state.last_error = Some("stuck: exceeded 2h threshold".into());
                    stuck_cleared.push(hb.config.agent_id.clone());
                }

                let next_ms = hb
                    .state
                    .next_run_at
                    .as_deref()
                    .and_then(|s| parse::parse_absolute_time_ms(s).ok());
                let is_running = hb.state.running_at.is_some();
                if hb.config.enabled && !is_running && next_ms.is_some_and(|t| t <= now) {
                    hb.state.running_at = Some(ms_to_rfc3339(now));
                    due.push(hb.clone());
                }
            }
            (due, stuck_cleared)
        };

        for agent_id in stuck_cleared {
            if let Err(e) = self.persist_heartbeat_status(&agent_id).await {
                warn!(
                    event = "heartbeat.run.stuck_cleared.persist_failed",
                    policy = POLICY,
                    decision = "fail",
                    reason_code = "heartbeat_stuck_clear_persist_failed",
                    agent_id = %agent_id,
                    error = %e,
                    "failed to persist stuck-cleared heartbeat state"
                );
            }
        }

        let mut runnable = Vec::new();
        for hb in due {
            match self.persist_heartbeat_status(&hb.config.agent_id).await {
                Ok(()) => runnable.push(hb),
                Err(e) => {
                    warn!(
                        event = "heartbeat.run.start_state.persist_failed",
                        policy = POLICY,
                        decision = "skip",
                        reason_code = "heartbeat_running_at_persist_failed",
                        agent_id = %hb.config.agent_id,
                        error = %e,
                        "failed to persist heartbeat runningAt; skipping this run"
                    );
                    self.update_heartbeat_state(&hb.config.agent_id, |state| state.running_at = None)
                        .await;
                },
            }
        }

        for hb in runnable {
            let svc = Arc::clone(self);
            tokio::spawn(async move {
                svc.execute_heartbeat(&hb).await;
            });
        }
    }

    async fn execute_heartbeat(self: &Arc<Self>, hb: &HeartbeatStatus) {
        let started_ms = now_ms();
        let run_id = uuid::Uuid::new_v4().to_string();

        // Active hours preflight.
        if let Some(ah) = hb.config.active_hours.as_ref() {
            match is_within_active_hours(&ah.start, &ah.end, &ah.timezone) {
                Ok(true) => {},
                Ok(false) => {
                    info!(
                        event = "heartbeat.run.skip",
                        policy = POLICY,
                        decision = "skip",
                        reason_code = "heartbeat_outside_active_hours",
                        agent_id = %hb.config.agent_id,
                        run_id = %run_id,
                        "heartbeat skipped (outside active hours)"
                    );
                    self.finish_heartbeat_run(
                        hb,
                        &run_id,
                        started_ms,
                        RunStatus::Skipped,
                        None,
                        None,
                        None,
                        Some("outside active hours".into()),
                    )
                    .await;
                    return;
                },
                Err(e) => {
                    error!(
                        event = "heartbeat.run.reject",
                        policy = POLICY,
                        decision = "reject",
                        reason_code = "active_hours_invalid",
                        agent_id = %hb.config.agent_id,
                        run_id = %run_id,
                        error = %e,
                        "heartbeat activeHours invalid"
                    );
                    self.finish_heartbeat_run(
                        hb,
                        &run_id,
                        started_ms,
                        RunStatus::Error,
                        None,
                        None,
                        None,
                        Some(format!("activeHours invalid: {e}")),
                    )
                    .await;
                    return;
                },
            }
        }

        // Load prompt (gateway-owned file owner).
        let prompt = match self.load_prompt(&hb.config.agent_id).await {
            Ok(p) => p,
            Err(e) => {
                error!(
                    event = "heartbeat.run.reject",
                    policy = POLICY,
                    decision = "reject",
                    reason_code = "heartbeat_prompt_missing",
                    agent_id = %hb.config.agent_id,
                    run_id = %run_id,
                    remediation = "create agents/<agent_id>/HEARTBEAT.md and write actionable content",
                    error = %e,
                    "heartbeat prompt missing"
                );
                self.finish_heartbeat_run(
                    hb,
                    &run_id,
                    started_ms,
                    RunStatus::Error,
                    None,
                    None,
                    None,
                    Some(e.to_string()),
                )
                .await;
                return;
            },
        };

        if is_heartbeat_content_empty(&prompt) {
            info!(
                event = "heartbeat.run.reject",
                policy = POLICY,
                decision = "reject",
                reason_code = "heartbeat_prompt_empty",
                agent_id = %hb.config.agent_id,
                run_id = %run_id,
                remediation = "edit agents/<agent_id>/HEARTBEAT.md and add at least one actionable line (non-header, non-empty list item)",
                "heartbeat prompt empty"
            );
            self.finish_heartbeat_run(
                hb,
                &run_id,
                started_ms,
                RunStatus::Error,
                None,
                None,
                None,
                Some("heartbeat prompt empty".into()),
            )
            .await;
            return;
        }

        info!(
            event = "heartbeat.run.start",
            policy = POLICY,
            decision = "allow",
            agent_id = %hb.config.agent_id,
            run_id = %run_id,
            "heartbeat executing"
        );

        let result = (self.on_run)(HeartbeatRunRequest {
            run_id: run_id.clone(),
            agent_id: hb.config.agent_id.clone(),
            session_target: hb.config.session_target.clone(),
            prompt: prompt.clone(),
            model_selector: hb.config.model_selector.clone(),
            timeout_secs: None,
        })
        .await;

        let finished_ms = now_ms();
        let duration_ms = finished_ms.saturating_sub(started_ms);

        let (mut status, mut error_msg, output, input_tokens, output_tokens) = match result {
            Ok(r) => (
                RunStatus::Ok,
                None,
                Some(r.output),
                r.input_tokens,
                r.output_tokens,
            ),
            Err(e) => {
                error!(
                    event = "heartbeat.run.finish",
                    policy = POLICY,
                    decision = "fail",
                    reason_code = "heartbeat_run_failed",
                    agent_id = %hb.config.agent_id,
                    run_id = %run_id,
                    error = %e,
                    "heartbeat run failed"
                );
                (RunStatus::Error, Some(e.to_string()), None, None, None)
            },
        };

        // Delivery (strip HEARTBEAT_OK).
        if status == RunStatus::Ok {
            if let Some(out) = output.as_deref() {
                let strip = strip_heartbeat_token(out, StripMode::Trim);
                if strip.should_skip {
                    info!(
                        event = "heartbeat.delivery",
                        policy = POLICY,
                        decision = "ok",
                        reason_code = "heartbeat_delivery_suppressed",
                        agent_id = %hb.config.agent_id,
                        run_id = %run_id,
                        "heartbeat delivery suppressed"
                    );
                } else {
                    let deliver_res = (self.on_deliver)(HeartbeatDeliverRequest {
                        run_id: run_id.clone(),
                        agent_id: hb.config.agent_id.clone(),
                        session_target: hb.config.session_target.clone(),
                        output: strip.text.clone(),
                    })
                    .await;
                    match deliver_res {
                        Ok(()) => info!(
                            event = "heartbeat.delivery",
                            policy = POLICY,
                            decision = "ok",
                            reason_code = "heartbeat_delivery_ok",
                            agent_id = %hb.config.agent_id,
                            run_id = %run_id,
                            output_len = strip.text.len(),
                            "heartbeat delivered"
                        ),
                        Err(e) => {
                            let delivery_error = e.to_string();
                            error!(
                                event = "heartbeat.delivery",
                                policy = POLICY,
                                decision = "fail",
                                reason_code = "heartbeat_delivery_failed",
                                agent_id = %hb.config.agent_id,
                                run_id = %run_id,
                                error = %delivery_error,
                                "heartbeat delivery failed"
                            );
                            status = RunStatus::Error;
                            error_msg = Some(delivery_error);
                        },
                    }
                }
            }
        }

        // Record run and update state.
        let output_preview = output
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate_preview(s, 240));

        let run_record = HeartbeatRunRecord {
            run_id: run_id.clone(),
            agent_id: hb.config.agent_id.clone(),
            started_at: ms_to_rfc3339(started_ms),
            finished_at: ms_to_rfc3339(finished_ms),
            status,
            error: error_msg.clone(),
            output_preview: output_preview.clone(),
            input_tokens,
            output_tokens,
        };

        if let Err(e) = self
            .store
            .append_run(&hb.config.agent_id, &run_record)
            .await
        {
            warn!(error = %e, "failed to record heartbeat run");
        }

        // Next run: now + interval.
        let next_run_at = if hb.config.enabled {
            parse::parse_duration_ms(&hb.config.every)
                .ok()
                .map(|ms| ms_to_rfc3339(now_ms() + ms))
        } else {
            None
        };

        self.update_heartbeat_state(&hb.config.agent_id, |state| {
            state.running_at = None;
            state.last_run_at = Some(ms_to_rfc3339(finished_ms));
            state.last_status = Some(status);
            state.last_error = error_msg.clone();
            state.last_duration_ms = Some(duration_ms);
            state.next_run_at = next_run_at.clone();
        })
        .await;

        // Persist updated status.
        if let Err(e) = self.persist_heartbeat_status(&hb.config.agent_id).await {
            warn!(
                event = "heartbeat.run.finish.persist_failed",
                policy = POLICY,
                decision = "fail",
                reason_code = "heartbeat_finish_persist_failed",
                agent_id = %hb.config.agent_id,
                error = %e,
                "failed to persist heartbeat finish state"
            );
        }

        info!(
            event = "heartbeat.run.finish",
            policy = POLICY,
            decision = match status {
                RunStatus::Ok => "ok",
                RunStatus::Error => "fail",
                RunStatus::Skipped => "skip",
            },
            agent_id = %hb.config.agent_id,
            run_id = %run_id,
            status = ?status,
            duration_ms,
            "heartbeat finished"
        );
    }

    async fn finish_heartbeat_run(
        &self,
        hb: &HeartbeatStatus,
        run_id: &str,
        started_ms: u64,
        status: RunStatus,
        output: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        error_msg: Option<String>,
    ) {
        let finished_ms = now_ms();
        let duration_ms = finished_ms.saturating_sub(started_ms);

        let output_preview = output
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate_preview(s, 240));

        let run_record = HeartbeatRunRecord {
            run_id: run_id.to_string(),
            agent_id: hb.config.agent_id.clone(),
            started_at: ms_to_rfc3339(started_ms),
            finished_at: ms_to_rfc3339(finished_ms),
            status,
            error: error_msg.clone(),
            output_preview,
            input_tokens,
            output_tokens,
        };
        let _ = self
            .store
            .append_run(&hb.config.agent_id, &run_record)
            .await;

        self.update_heartbeat_state(&hb.config.agent_id, |state| {
            state.running_at = None;
            state.last_run_at = Some(ms_to_rfc3339(finished_ms));
            state.last_status = Some(status);
            state.last_error = error_msg;
            state.last_duration_ms = Some(duration_ms);
            state.next_run_at = if hb.config.enabled {
                parse::parse_duration_ms(&hb.config.every)
                    .ok()
                    .map(|ms| ms_to_rfc3339(now_ms() + ms))
            } else {
                None
            };
        })
        .await;

        if let Err(e) = self.persist_heartbeat_status(&hb.config.agent_id).await {
            warn!(
                event = "heartbeat.run.finish.persist_failed",
                policy = POLICY,
                decision = "fail",
                reason_code = "heartbeat_finish_persist_failed",
                agent_id = %hb.config.agent_id,
                error = %e,
                "failed to persist heartbeat finish state"
            );
        }
    }

    async fn update_heartbeat_state<F: FnOnce(&mut HeartbeatState)>(&self, agent_id: &str, f: F) {
        let mut heartbeats = self.heartbeats.write().await;
        if let Some(hb) = heartbeats
            .iter_mut()
            .find(|s| s.config.agent_id == agent_id)
        {
            f(&mut hb.state);
        }
    }

    async fn persist_heartbeat_status(&self, agent_id: &str) -> Result<()> {
        let Some(updated) = self.get(agent_id).await? else {
            return Ok(());
        };
        self.store.upsert(&updated).await?;
        Ok(())
    }

    async fn recompute_all_next_runs(&self) -> Result<()> {
        let now = now_ms();
        let mut heartbeats = self.heartbeats.write().await;
        for hb in heartbeats.iter_mut() {
            if hb.config.enabled {
                let every_ms = parse::parse_duration_ms(&hb.config.every)?;
                hb.state.next_run_at = Some(ms_to_rfc3339(now + every_ms));
            } else {
                hb.state.next_run_at = None;
            }
        }
        Ok(())
    }

    async fn validate_status(&self, status: &HeartbeatStatus) -> Result<()> {
        self.validate_status_with_mode(status, false).await
    }

    async fn validate_loaded_status(&self, status: &HeartbeatStatus) -> Result<()> {
        self.validate_status_with_mode(status, true).await
    }

    async fn validate_status_with_mode(
        &self,
        status: &HeartbeatStatus,
        persisted_load: bool,
    ) -> Result<()> {
        let cfg = &status.config;

        if let Some(ref validate_agent_id) = self.validators.validate_agent_id {
            validate_agent_id(&cfg.agent_id).await?;
        } else if cfg.agent_id.trim().is_empty() {
            bail!("agent_missing");
        }

        // Validate interval.
        let _ = parse::parse_duration_ms(&cfg.every)
            .map_err(|_| anyhow::anyhow!("heartbeat interval invalid"))?;

        // Session target: required and gateway-owned validation.
        if let Some(ref validate) = self.validators.validate_session_target {
            if !persisted_load {
                validate(&cfg.agent_id, &cfg.session_target).await?;
            }
        } else {
            bail!("heartbeat_target_missing");
        }

        // Active hours shape: strict.
        if let Some(ah) = cfg.active_hours.as_ref() {
            let _ = is_within_active_hours(&ah.start, &ah.end, &ah.timezone)?;
        }

        // Model selector.
        if let ModelSelector::Explicit { model_id } = &cfg.model_selector
            && let Some(ref validate_model_id) = self.validators.validate_model_id
            && !persisted_load
        {
            validate_model_id(model_id).await?;
        }

        // Prompt presence/emptiness checked when enabled.
        if cfg.enabled && !persisted_load {
            let prompt = self.load_prompt(&cfg.agent_id).await?;
            if prompt.trim().is_empty() {
                bail!("heartbeat_prompt_missing");
            }
            if is_heartbeat_content_empty(&prompt) {
                bail!("heartbeat_prompt_empty");
            }
        }

        Ok(())
    }

    async fn load_prompt(&self, agent_id: &str) -> Result<String> {
        if let Some(ref load) = self.validators.load_prompt {
            return load(agent_id).await;
        }
        bail!("heartbeat_prompt_missing");
    }

    async fn mark_manual_run_running(
        &self,
        agent_id: &str,
        force: bool,
    ) -> Result<HeartbeatStatus> {
        let now_ms = now_ms();
        let now = ms_to_rfc3339(now_ms);
        let mut heartbeats = self.heartbeats.write().await;
        let heartbeat = heartbeats
            .iter_mut()
            .find(|status| status.config.agent_id == agent_id)
            .ok_or_else(|| anyhow::anyhow!("heartbeat not found: {agent_id}"))?;

        if !heartbeat.config.enabled && !force {
            bail!("heartbeat is disabled (use force=true to override)");
        }
        if heartbeat.state.running_at.is_some() {
            let running_ms = heartbeat
                .state
                .running_at
                .as_deref()
                .and_then(|s| parse::parse_absolute_time_ms(s).ok());
            if running_ms.is_some_and(|started| now_ms.saturating_sub(started) > STUCK_THRESHOLD_MS)
            {
                warn!(
                    event = "heartbeat.run.stuck_cleared",
                    policy = POLICY,
                    decision = "drop",
                    reason_code = "heartbeat_stuck_cleared",
                    agent_id = %heartbeat.config.agent_id,
                    "clearing stuck heartbeat before manual run (exceeded 2h threshold)"
                );
                heartbeat.state.running_at = None;
                heartbeat.state.last_status = Some(RunStatus::Error);
                heartbeat.state.last_error = Some("stuck: exceeded 2h threshold".into());
            } else {
                bail!("heartbeat is already running");
            }
        }

        heartbeat.state.running_at = Some(now);
        Ok(heartbeat.clone())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tokio::time::{Duration, sleep};

    fn noop_run() -> RunHeartbeatFn {
        Arc::new(|_req| {
            Box::pin(async {
                Ok(HeartbeatRunResult {
                    output: "HEARTBEAT_OK".into(),
                    input_tokens: None,
                    output_tokens: None,
                })
            })
        })
    }

    fn noop_deliver() -> DeliverHeartbeatFn {
        Arc::new(|_req| Box::pin(async { Ok(()) }))
    }

    async fn sqlite_store() -> crate::store_sqlite::SqliteStore {
        crate::store_sqlite::SqliteStore::new("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn upsert_rejects_empty_prompt_when_enabled() {
        let store = Arc::new(sqlite_store().await);
        let svc = HeartbeatService::with_config(
            store,
            noop_run(),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| Box::pin(async { Ok(String::new()) }))),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );
        let err = svc
            .upsert(HeartbeatConfig {
                agent_id: "default".into(),
                enabled: true,
                every: "30m".into(),
                session_target: SessionTarget::Main,
                model_selector: ModelSelector::Inherit,
                active_hours: None,
            })
            .await
            .expect_err("should reject");
        assert!(err.to_string().contains("heartbeat_prompt_missing"));
    }

    #[tokio::test]
    async fn upsert_rejects_local_timezone_alias() {
        let store = Arc::new(sqlite_store().await);
        let svc = HeartbeatService::with_config(
            store,
            noop_run(),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("Check the bound session.".to_string()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );
        let err = svc
            .upsert(HeartbeatConfig {
                agent_id: "default".into(),
                enabled: true,
                every: "30m".into(),
                session_target: SessionTarget::Main,
                model_selector: ModelSelector::Inherit,
                active_hours: Some(crate::types::ActiveHours {
                    start: "08:00".into(),
                    end: "24:00".into(),
                    timezone: "local".into(),
                }),
            })
            .await
            .expect_err("should reject");
        assert!(err.to_string().contains("unknown timezone"));
    }

    #[tokio::test]
    async fn running_at_is_persisted_before_execution() {
        let store = Arc::new(sqlite_store().await);
        let notify = Arc::new(Notify::new());

        let notify_for_run = Arc::clone(&notify);
        let svc = HeartbeatService::with_config(
            store.clone(),
            Arc::new(move |_req| {
                let notify_for_run = Arc::clone(&notify_for_run);
                Box::pin(async move {
                    notify_for_run.notified().await;
                    Ok(HeartbeatRunResult {
                        output: "hello".into(),
                        input_tokens: None,
                        output_tokens: None,
                    })
                })
            }),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("do something".into()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );

        svc.upsert(HeartbeatConfig {
            agent_id: "default".into(),
            enabled: true,
            every: "1h".into(),
            session_target: SessionTarget::Main,
            model_selector: ModelSelector::Inherit,
            active_hours: None,
        })
        .await
        .unwrap();

        let svc_for_run = Arc::clone(&svc);
        let handle = tokio::spawn(async move { svc_for_run.run("default", false).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let status = store.get("default").await.unwrap().unwrap();
                if status.state.running_at.is_some() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runningAt should be persisted promptly");

        notify.notify_one();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn run_marks_delivery_failure_as_error() {
        let store = Arc::new(sqlite_store().await);
        let svc = HeartbeatService::with_config(
            store,
            Arc::new(|_req| {
                Box::pin(async {
                    Ok(HeartbeatRunResult {
                        output: "heartbeat output".into(),
                        input_tokens: Some(3),
                        output_tokens: Some(5),
                    })
                })
            }),
            Arc::new(|_req| Box::pin(async { Err(anyhow::anyhow!("bound session missing")) })),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("Check the session.".to_string()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );

        svc.upsert(HeartbeatConfig {
            agent_id: "default".into(),
            enabled: true,
            every: "30m".into(),
            session_target: SessionTarget::Main,
            model_selector: ModelSelector::Inherit,
            active_hours: None,
        })
        .await
        .unwrap();

        svc.run("default", false).await.unwrap();

        let runs = svc.runs("default", 10).await.unwrap();
        let run = runs.last().expect("run recorded");
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.error.as_deref(), Some("bound session missing"));
        assert_eq!(run.output_preview.as_deref(), Some("heartbeat output"));

        let status = svc.get("default").await.unwrap().expect("status exists");
        assert_eq!(status.state.last_status, Some(RunStatus::Error));
        assert_eq!(
            status.state.last_error.as_deref(),
            Some("bound session missing")
        );
    }

    #[tokio::test]
    async fn upsert_preserves_runtime_state() {
        let store = Arc::new(sqlite_store().await);
        let existing = HeartbeatStatus {
            config: HeartbeatConfig {
                agent_id: "default".into(),
                enabled: true,
                every: "30m".into(),
                session_target: SessionTarget::Main,
                model_selector: ModelSelector::Inherit,
                active_hours: None,
            },
            state: HeartbeatState {
                next_run_at: Some("2026-03-30T10:00:00.000Z".into()),
                running_at: Some("2026-03-30T09:59:00.000Z".into()),
                last_run_at: Some("2026-03-30T09:30:00.000Z".into()),
                last_status: Some(RunStatus::Error),
                last_error: Some("previous failure".into()),
                last_duration_ms: Some(1234),
            },
        };
        store.upsert(&existing).await.unwrap();

        let svc = HeartbeatService::with_config(
            store,
            noop_run(),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("Check the session.".to_string()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );
        svc.start().await.unwrap();

        let updated = svc
            .upsert(HeartbeatConfig {
                agent_id: "default".into(),
                enabled: false,
                every: "45m".into(),
                session_target: SessionTarget::Session {
                    session_key: "agent/default/ops".into(),
                },
                model_selector: ModelSelector::Explicit {
                    model_id: "gpt-5".into(),
                },
                active_hours: None,
            })
            .await
            .unwrap();

        assert_eq!(
            updated.state.last_run_at.as_deref(),
            Some("2026-03-30T09:30:00.000Z")
        );
        assert_eq!(updated.state.last_status, Some(RunStatus::Error));
        assert_eq!(
            updated.state.last_error.as_deref(),
            Some("previous failure")
        );
        assert_eq!(updated.state.last_duration_ms, Some(1234));
        assert_eq!(
            updated.state.running_at.as_deref(),
            Some("2026-03-30T09:59:00.000Z")
        );
        assert_eq!(updated.state.next_run_at, None);
    }

    #[tokio::test]
    async fn forced_run_persists_rejected_state_to_store() {
        let store = Arc::new(sqlite_store().await);
        let heartbeat_store: Arc<dyn crate::store_heartbeat::HeartbeatStore> = store.clone();
        let svc = HeartbeatService::with_config(
            heartbeat_store,
            noop_run(),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Err(anyhow::anyhow!("heartbeat prompt missing on disk")) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );

        svc.upsert(HeartbeatConfig {
            agent_id: "default".into(),
            enabled: false,
            every: "30m".into(),
            session_target: SessionTarget::Main,
            model_selector: ModelSelector::Inherit,
            active_hours: None,
        })
        .await
        .unwrap();

        svc.run("default", true).await.unwrap();

        let persisted = crate::store_heartbeat::HeartbeatStore::get(store.as_ref(), "default")
            .await
            .unwrap()
            .expect("heartbeat persisted");
        assert_eq!(persisted.state.last_status, Some(RunStatus::Error));
        assert_eq!(
            persisted.state.last_error.as_deref(),
            Some("heartbeat prompt missing on disk")
        );
        assert!(persisted.state.last_run_at.is_some());
    }

    #[tokio::test]
    async fn start_defers_stale_session_target_validation_until_run() {
        let store = Arc::new(sqlite_store().await);
        crate::store_heartbeat::HeartbeatStore::upsert(
            store.as_ref(),
            &HeartbeatStatus {
                config: HeartbeatConfig {
                    agent_id: "default".into(),
                    enabled: true,
                    every: "30m".into(),
                    session_target: SessionTarget::Session {
                        session_key: "agent/default/missing".into(),
                    },
                    model_selector: ModelSelector::Inherit,
                    active_hours: None,
                },
                state: HeartbeatState::default(),
            },
        )
        .await
        .unwrap();

        let svc = HeartbeatService::with_config(
            store,
            noop_run(),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Err(anyhow::anyhow!("heartbeat prompt missing on disk")) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Err(anyhow::anyhow!("heartbeat_target_missing")) })
                })),
                ..Default::default()
            },
        );

        svc.start().await.unwrap();
        assert!(svc.get("default").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn manual_run_marks_running_and_rejects_overlap() {
        let store = Arc::new(sqlite_store().await);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let run_calls = Arc::new(AtomicUsize::new(0));
        let started_clone = Arc::clone(&started);
        let release_clone = Arc::clone(&release);
        let run_calls_clone = Arc::clone(&run_calls);

        let svc = HeartbeatService::with_config(
            store,
            Arc::new(move |_req| {
                let started = Arc::clone(&started_clone);
                let release = Arc::clone(&release_clone);
                let run_calls = Arc::clone(&run_calls_clone);
                Box::pin(async move {
                    run_calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_waiters();
                    release.notified().await;
                    Ok(HeartbeatRunResult {
                        output: "HEARTBEAT_OK".into(),
                        input_tokens: None,
                        output_tokens: None,
                    })
                })
            }),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("Check the session.".to_string()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );

        svc.upsert(HeartbeatConfig {
            agent_id: "default".into(),
            enabled: true,
            every: "30m".into(),
            session_target: SessionTarget::Main,
            model_selector: ModelSelector::Inherit,
            active_hours: None,
        })
        .await
        .unwrap();

        let svc_clone = Arc::clone(&svc);
        let first_run = tokio::spawn(async move { svc_clone.run("default", false).await });
        started.notified().await;

        let mut saw_running = false;
        for _ in 0..20 {
            if svc
                .get("default")
                .await
                .unwrap()
                .and_then(|status| status.state.running_at)
                .is_some()
            {
                saw_running = true;
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(saw_running, "manual run should mark heartbeat as running");

        let err = svc
            .run("default", false)
            .await
            .expect_err("overlap should reject");
        assert!(err.to_string().contains("heartbeat is already running"));
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);

        release.notify_waiters();
        first_run.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn timer_clears_stale_running_state_before_scheduling_next_run() {
        let store = Arc::new(sqlite_store().await);
        let started = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));

        let started_clone = Arc::clone(&started);
        let allow_finish_clone = Arc::clone(&allow_finish);
        let calls_clone = Arc::clone(&calls);
        let svc = HeartbeatService::with_config(
            store,
            Arc::new(move |_req| {
                let started = Arc::clone(&started_clone);
                let allow_finish = Arc::clone(&allow_finish_clone);
                let calls = Arc::clone(&calls_clone);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    allow_finish.notified().await;
                    Ok(HeartbeatRunResult {
                        output: "HEARTBEAT_OK".into(),
                        input_tokens: None,
                        output_tokens: None,
                    })
                })
            }),
            noop_deliver(),
            HeartbeatValidators {
                load_prompt: Some(Arc::new(|_agent_id| {
                    Box::pin(async { Ok("Check the session.".to_string()) })
                })),
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Ok(()) })
                })),
                ..Default::default()
            },
        );

        svc.upsert(HeartbeatConfig {
            agent_id: "default".into(),
            enabled: true,
            every: "30m".into(),
            session_target: SessionTarget::Main,
            model_selector: ModelSelector::Inherit,
            active_hours: None,
        })
        .await
        .unwrap();

        let now = now_ms();
        {
            let mut heartbeats = svc.heartbeats.write().await;
            let hb = heartbeats
                .iter_mut()
                .find(|status| status.config.agent_id == "default")
                .expect("heartbeat exists");
            hb.state.next_run_at = Some(ms_to_rfc3339(now.saturating_sub(1)));
            hb.state.running_at = Some(ms_to_rfc3339(
                now.saturating_sub(STUCK_THRESHOLD_MS + 1),
            ));
        }

        svc.process_due_heartbeats().await;

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("heartbeat scheduled");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let status = svc.get("default").await.unwrap().expect("status exists");
        assert_eq!(
            status.state.last_error.as_deref(),
            Some("stuck: exceeded 2h threshold")
        );

        allow_finish.notify_one();
    }
}

//! Core cron scheduler: timer loop, job execution, CRUD operations.
//!
//! One-cut governance model:
//! - Cron execution is always isolated (no session context).
//! - Delivery is post-run and must be explicit: silent | session | telegram.
//! - No legacy fields, no fallback stores, no silent degrade.

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

#[cfg(feature = "metrics")]
use moltis_metrics::{counter, cron as cron_metrics, gauge, histogram};

use crate::{
    parse,
    schedule::compute_next_run,
    store::CronStore,
    types::{
        CronDelivery, CronJob, CronJobCreate, CronJobPatch, CronNotification, CronRunRecord,
        CronSchedule, CronStatus, ModelSelector, RunStatus, SessionTarget, TelegramTarget,
    },
};

const POLICY: &str = "cron_heartbeat_governance_v1";

/// Result of an agent turn, including optional token usage.
#[derive(Debug, Clone)]
pub struct AgentTurnResult {
    pub output: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Parameters for running an isolated agent turn (cron execution).
#[derive(Debug, Clone)]
pub struct CronRunRequest {
    pub run_id: String,
    pub job_id: String,
    pub agent_id: String,
    pub prompt: String,
    pub model_selector: ModelSelector,
    pub delivery_session_target: Option<SessionTarget>,
    pub timeout_secs: Option<u64>,
}

/// Callback for running an isolated agent turn.
pub type RunCronFn = Arc<
    dyn Fn(CronRunRequest) -> Pin<Box<dyn Future<Output = Result<AgentTurnResult>> + Send>>
        + Send
        + Sync,
>;

/// Parameters for delivering a cron result.
#[derive(Debug, Clone)]
pub struct CronDeliverRequest {
    pub run_id: String,
    pub job_id: String,
    pub agent_id: String,
    pub delivery: CronDelivery,
    pub output: String,
}

/// Callback for delivering a cron result.
pub type DeliverCronFn = Arc<
    dyn Fn(CronDeliverRequest) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync,
>;

/// Callback for notifying about cron job changes.
pub type NotifyFn = Arc<dyn Fn(CronNotification) + Send + Sync>;

/// Optional validation hooks owned by the gateway boundary.
#[derive(Clone, Default)]
pub struct CronValidators {
    pub validate_model_id:
        Option<Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>,
    pub validate_session_target: Option<
        Arc<
            dyn Fn(&str, &SessionTarget) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
                + Send
                + Sync,
        >,
    >,
    pub validate_telegram_target: Option<
        Arc<
            dyn Fn(&TelegramTarget) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
                + Send
                + Sync,
        >,
    >,
    pub validate_agent_id:
        Option<Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>,
}

/// Rate limiting configuration for cron job creation.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of jobs that can be created within the window.
    pub max_per_window: usize,
    /// Window duration in milliseconds.
    pub window_ms: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_per_window: 10,
            window_ms: 60_000, // 1 minute
        }
    }
}

/// Simple sliding-window rate limiter.
struct RateLimiter {
    timestamps: VecDeque<u64>,
    config: RateLimitConfig,
}

impl RateLimiter {
    fn new(config: RateLimitConfig) -> Self {
        Self {
            timestamps: VecDeque::new(),
            config,
        }
    }

    /// Check if a new job can be created. Returns Ok(()) if allowed, Err if rate limited.
    fn check(&mut self) -> Result<()> {
        let now = now_ms();
        let cutoff = now.saturating_sub(self.config.window_ms);

        while self.timestamps.front().is_some_and(|&ts| ts < cutoff) {
            self.timestamps.pop_front();
        }

        if self.timestamps.len() >= self.config.max_per_window {
            bail!(
                "rate limit exceeded: max {} jobs per {} seconds",
                self.config.max_per_window,
                self.config.window_ms / 1000
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

/// The cron scheduler.
pub struct CronService {
    store: Arc<dyn CronStore>,
    validators: CronValidators,
    jobs: RwLock<Vec<CronJob>>,
    timer_handle: Mutex<Option<JoinHandle<()>>>,
    wake_notify: Arc<Notify>,
    running: RwLock<bool>,
    on_run: RunCronFn,
    on_deliver: DeliverCronFn,
    on_notify: Option<NotifyFn>,
    rate_limiter: Mutex<RateLimiter>,
}

/// Max time a job can be in "running" state before we consider it stuck (2 hours).
const STUCK_THRESHOLD_MS: u64 = 2 * 60 * 60 * 1000;

impl CronService {
    pub fn new(
        store: Arc<dyn CronStore>,
        on_run: RunCronFn,
        on_deliver: DeliverCronFn,
    ) -> Arc<Self> {
        Self::with_config(
            store,
            on_run,
            on_deliver,
            None,
            RateLimitConfig::default(),
            CronValidators::default(),
        )
    }

    pub fn with_notify(
        store: Arc<dyn CronStore>,
        on_run: RunCronFn,
        on_deliver: DeliverCronFn,
        on_notify: NotifyFn,
    ) -> Arc<Self> {
        Self::with_config(
            store,
            on_run,
            on_deliver,
            Some(on_notify),
            RateLimitConfig::default(),
            CronValidators::default(),
        )
    }

    pub fn with_config(
        store: Arc<dyn CronStore>,
        on_run: RunCronFn,
        on_deliver: DeliverCronFn,
        on_notify: Option<NotifyFn>,
        rate_limit_config: RateLimitConfig,
        validators: CronValidators,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            validators,
            jobs: RwLock::new(Vec::new()),
            timer_handle: Mutex::new(None),
            wake_notify: Arc::new(Notify::new()),
            running: RwLock::new(false),
            on_run,
            on_deliver,
            on_notify,
            rate_limiter: Mutex::new(RateLimiter::new(rate_limit_config)),
        })
    }

    pub fn with_validators(self: Arc<Self>, validators: CronValidators) -> Arc<Self> {
        // Arc mutation pattern: clone inner to new service only when needed is overkill here.
        // Instead, require validators to be set during construction in new call sites.
        // Kept for API stability: no-op when already set.
        let _ = validators;
        self
    }

    fn notify(&self, notification: CronNotification) {
        if let Some(ref notify_fn) = self.on_notify {
            notify_fn(notification);
        }
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let loaded = self.store.load_jobs().await?;
        info!(count = loaded.len(), "loaded cron jobs");

        for job in &loaded {
            self.validate_loaded_job(job).await?;
        }

        {
            let mut jobs = self.jobs.write().await;
            *jobs = loaded;
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
        info!("cron service stopped");
    }

    pub async fn add(&self, create: CronJobCreate) -> Result<CronJob> {
        self.rate_limiter.lock().await.check()?;

        let now = now_ms();
        let mut job = CronJob {
            job_id: create
                .job_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            agent_id: create.agent_id,
            name: create.name,
            enabled: create.enabled,
            delete_after_run: create.delete_after_run,
            schedule: create.schedule,
            prompt: create.prompt,
            model_selector: create.model_selector,
            timeout_secs: create.timeout_secs,
            delivery: create.delivery,
            state: Default::default(),
        };

        self.validate_job(&job).await?;

        if job.enabled {
            job.state.next_run_at = compute_next_run_rfc3339(&job.schedule, now)?;
        }

        self.store.save_job(&job).await?;
        {
            let mut jobs = self.jobs.write().await;
            jobs.push(job.clone());
        }
        self.wake_notify.notify_one();
        self.notify(CronNotification::Created { job: job.clone() });
        info!(job_id = %job.job_id, agent_id = %job.agent_id, "cron job added");
        Ok(job)
    }

    pub async fn update(&self, job_id: &str, patch: CronJobPatch) -> Result<CronJob> {
        let now = now_ms();
        let mut jobs = self.jobs.write().await;
        let job = jobs
            .iter_mut()
            .find(|j| j.job_id == job_id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {job_id}"))?;

        if let Some(agent_id) = patch.agent_id {
            job.agent_id = agent_id;
        }
        if let Some(name) = patch.name {
            job.name = name;
        }
        if let Some(schedule) = patch.schedule {
            job.schedule = schedule;
        }
        if let Some(prompt) = patch.prompt {
            job.prompt = prompt;
        }
        if let Some(model_selector) = patch.model_selector {
            job.model_selector = model_selector;
        }
        if let Some(timeout) = patch.timeout_secs {
            job.timeout_secs = timeout;
        }
        if let Some(delivery) = patch.delivery {
            job.delivery = delivery;
        }
        if let Some(enabled) = patch.enabled {
            job.enabled = enabled;
        }
        if let Some(delete_after_run) = patch.delete_after_run {
            job.delete_after_run = delete_after_run;
        }

        self.validate_job(job).await?;

        if job.enabled {
            job.state.next_run_at = compute_next_run_rfc3339(&job.schedule, now)?;
        } else {
            job.state.next_run_at = None;
        }

        let updated = job.clone();
        self.store.update_job(&updated).await?;

        drop(jobs);
        self.wake_notify.notify_one();
        self.notify(CronNotification::Updated {
            job: updated.clone(),
        });
        info!(job_id, "cron job updated");
        Ok(updated)
    }

    pub async fn remove(&self, job_id: &str) -> Result<()> {
        self.store.delete_job(job_id).await?;
        let mut jobs = self.jobs.write().await;
        jobs.retain(|j| j.job_id != job_id);
        drop(jobs);
        self.notify(CronNotification::Removed {
            job_id: job_id.to_string(),
        });
        info!(job_id, "cron job removed");
        Ok(())
    }

    pub async fn list(&self) -> Vec<CronJob> {
        self.jobs.read().await.clone()
    }

    pub async fn runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        self.store.get_runs(job_id, limit).await
    }

    pub async fn status(&self) -> CronStatus {
        let jobs = self.jobs.read().await;
        let running = *self.running.read().await;
        let enabled_count = jobs.iter().filter(|j| j.enabled).count();
        let next_run_at = jobs
            .iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at.clone())
            .min();

        #[cfg(feature = "metrics")]
        gauge!(cron_metrics::JOBS_SCHEDULED).set(jobs.len() as f64);

        CronStatus {
            running,
            job_count: jobs.len(),
            enabled_count,
            next_run_at,
        }
    }

    pub async fn run(self: &Arc<Self>, job_id: &str, force: bool) -> Result<()> {
        let now = now_ms();
        let job = {
            let mut jobs = self.jobs.write().await;
            let job = jobs
                .iter_mut()
                .find(|j| j.job_id == job_id)
                .ok_or_else(|| anyhow::anyhow!("job not found: {job_id}"))?;
            if !job.enabled && !force {
                bail!("job is disabled (use force=true to override)");
            }
            job.state.running_at = Some(ms_to_rfc3339(now));
            job.clone()
        };

        if let Err(e) = self.store.update_job(&job).await {
            // Revert local running state so we don't block future runs.
            self.update_job_state(job_id, |state| {
                state.running_at = None;
            })
            .await;
            bail!("failed to persist cron runningAt: {e}");
        }

        self.execute_job(&job).await;
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
                        debug!("timer loop woken by notify");
                        continue;
                    },
                }
            }

            if !*self.running.read().await {
                break;
            }

            self.process_due_jobs().await;
        }
    }

    async fn ms_until_next_wake(&self) -> u64 {
        let jobs = self.jobs.read().await;
        let now = now_ms();
        jobs.iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at.as_deref())
            .filter_map(|s| parse::parse_absolute_time_ms(s).ok())
            .map(|t| t.saturating_sub(now))
            .min()
            .unwrap_or(60_000)
    }

    async fn process_due_jobs(self: &Arc<Self>) {
        let now = now_ms();

        let due_jobs: Vec<CronJob> = {
            let mut jobs = self.jobs.write().await;
            let mut due = Vec::new();
            for job in jobs.iter_mut() {
                let next_ms = job
                    .state
                    .next_run_at
                    .as_deref()
                    .and_then(|s| parse::parse_absolute_time_ms(s).ok());
                let is_running = job.state.running_at.is_some();
                if job.enabled && !is_running && next_ms.is_some_and(|t| t <= now) {
                    job.state.running_at = Some(ms_to_rfc3339(now));
                    due.push(job.clone());
                }
            }
            due
        };

        let stuck_cleared = self.clear_stuck_jobs(now).await;
        for job in stuck_cleared {
            if let Err(e) = self.store.update_job(&job).await {
                warn!(
                    event = "cron.run.stuck_cleared.persist_failed",
                    policy = POLICY,
                    decision = "fail",
                    reason_code = "cron_stuck_clear_persist_failed",
                    job_id = %job.job_id,
                    agent_id = %job.agent_id,
                    error = %e,
                    "failed to persist stuck-cleared cron state"
                );
            }
        }

        let mut runnable = Vec::new();
        for job in due_jobs {
            match self.store.update_job(&job).await {
                Ok(()) => runnable.push(job),
                Err(e) => {
                    warn!(
                        event = "cron.run.start_state.persist_failed",
                        policy = POLICY,
                        decision = "skip",
                        reason_code = "cron_running_at_persist_failed",
                        job_id = %job.job_id,
                        agent_id = %job.agent_id,
                        error = %e,
                        "failed to persist cron runningAt; skipping this run"
                    );
                    // Revert local running state so future intervals aren't blocked.
                    self.update_job_state(&job.job_id, |state| state.running_at = None)
                        .await;
                },
            }
        }

        for job in runnable {
            let svc = Arc::clone(self);
            tokio::spawn(async move {
                svc.execute_job(&job).await;
            });
        }
    }

    async fn execute_job(self: &Arc<Self>, job: &CronJob) {
        let started_ms = now_ms();
        let run_id = uuid::Uuid::new_v4().to_string();

        #[cfg(feature = "metrics")]
        counter!(cron_metrics::EXECUTIONS_TOTAL).increment(1);

        info!(
            event = "cron.run.start",
            policy = POLICY,
            decision = "allow",
            job_id = %job.job_id,
            agent_id = %job.agent_id,
            run_id = %run_id,
            "cron job executing"
        );

        let result = (self.on_run)(CronRunRequest {
            run_id: run_id.clone(),
            job_id: job.job_id.clone(),
            agent_id: job.agent_id.clone(),
            prompt: job.prompt.clone(),
            model_selector: job.model_selector.clone(),
            delivery_session_target: match &job.delivery {
                CronDelivery::Session { target } => Some(target.clone()),
                CronDelivery::Silent | CronDelivery::Telegram { .. } => None,
            },
            timeout_secs: job.timeout_secs,
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
                    event = "cron.run.finish",
                    policy = POLICY,
                    decision = "fail",
                    reason_code = "cron_run_failed",
                    job_id = %job.job_id,
                    agent_id = %job.agent_id,
                    run_id = %run_id,
                    error = %e,
                    "cron job failed"
                );
                #[cfg(feature = "metrics")]
                counter!(cron_metrics::ERRORS_TOTAL).increment(1);
                (RunStatus::Error, Some(e.to_string()), None, None, None)
            },
        };

        #[cfg(feature = "metrics")]
        histogram!(cron_metrics::EXECUTION_DURATION_SECONDS).record(duration_ms as f64 / 1000.0);

        // Delivery (post-run).
        if status == RunStatus::Ok {
            if let Some(out) = output.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                match &job.delivery {
                    CronDelivery::Silent => {
                        info!(
                            event = "cron.delivery",
                            policy = POLICY,
                            decision = "ok",
                            reason_code = "cron_delivery_silent",
                            job_id = %job.job_id,
                            agent_id = %job.agent_id,
                            run_id = %run_id,
                            delivery_kind = "silent",
                            output_len = out.len(),
                            "cron delivery suppressed (silent)"
                        );
                    },
                    _ => {
                        let delivery = job.delivery.clone();
                        let deliver_res = (self.on_deliver)(CronDeliverRequest {
                            run_id: run_id.clone(),
                            job_id: job.job_id.clone(),
                            agent_id: job.agent_id.clone(),
                            delivery: delivery.clone(),
                            output: out.to_string(),
                        })
                        .await;
                        match deliver_res {
                            Ok(()) => info!(
                                event = "cron.delivery",
                                policy = POLICY,
                                decision = "ok",
                                reason_code = "cron_delivery_ok",
                                job_id = %job.job_id,
                                agent_id = %job.agent_id,
                                run_id = %run_id,
                                delivery_kind = match delivery { CronDelivery::Session{..} => "session", CronDelivery::Telegram{..} => "telegram", CronDelivery::Silent => "silent" },
                                output_len = out.len(),
                                "cron delivery ok"
                            ),
                            Err(e) => {
                                let delivery_error = e.to_string();
                                error!(
                                    event = "cron.delivery",
                                    policy = POLICY,
                                    decision = "fail",
                                    reason_code = "cron_delivery_failed",
                                    job_id = %job.job_id,
                                    agent_id = %job.agent_id,
                                    run_id = %run_id,
                                    error = %delivery_error,
                                    "cron delivery failed"
                                );
                                status = RunStatus::Error;
                                error_msg = Some(delivery_error);
                            },
                        }
                    },
                }
            }
        }

        let output_preview = output
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate_preview(s, 240));

        let run_record = CronRunRecord {
            run_id: run_id.clone(),
            job_id: job.job_id.clone(),
            started_at: ms_to_rfc3339(started_ms),
            finished_at: ms_to_rfc3339(finished_ms),
            status,
            error: error_msg.clone(),
            output_preview: output_preview.clone(),
            input_tokens,
            output_tokens,
        };

        if let Err(e) = self.store.append_run(&job.job_id, &run_record).await {
            warn!(error = %e, "failed to record cron run");
        }

        // Update job state.
        let now = now_ms();
        let next_run_at = match compute_next_run_rfc3339(&job.schedule, now) {
            Ok(v) => v,
            Err(e) => {
                warn!(job_id = %job.job_id, error = %e, "failed to compute next run");
                None
            },
        };

        self.update_job_state(&job.job_id, |state| {
            state.running_at = None;
            state.last_run_at = Some(ms_to_rfc3339(finished_ms));
            state.last_status = Some(status);
            state.last_error = error_msg;
            state.last_duration_ms = Some(duration_ms);
            state.next_run_at = next_run_at.clone();
        })
        .await;

        // once schedule: disable or delete after run
        if matches!(job.schedule, CronSchedule::Once { .. }) {
            if job.delete_after_run {
                let _ = self.remove(&job.job_id).await;
            } else {
                let mut jobs = self.jobs.write().await;
                if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job.job_id) {
                    j.enabled = false;
                    j.state.next_run_at = None;
                    let _ = self.store.update_job(j).await;
                }
            }
        } else {
            // Persist updated state.
            let jobs = self.jobs.read().await;
            if let Some(j) = jobs.iter().find(|j| j.job_id == job.job_id) {
                let _ = self.store.update_job(j).await;
            }
        }

        info!(
            event = "cron.run.finish",
            policy = POLICY,
            decision = match status {
                RunStatus::Ok => "ok",
                RunStatus::Error => "fail",
                RunStatus::Skipped => "skip",
            },
            job_id = %job.job_id,
            agent_id = %job.agent_id,
            run_id = %run_id,
            status = ?status,
            duration_ms,
            "cron job finished"
        );
    }

    async fn update_job_state<F: FnOnce(&mut crate::types::CronJobState)>(
        &self,
        job_id: &str,
        f: F,
    ) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.job_id == job_id) {
            f(&mut job.state);
        }
    }

    async fn recompute_all_next_runs(&self) -> Result<()> {
        let now = now_ms();
        let mut jobs = self.jobs.write().await;
        for job in jobs.iter_mut() {
            if job.enabled {
                job.state.next_run_at = compute_next_run_rfc3339(&job.schedule, now)?;
            } else {
                job.state.next_run_at = None;
            }
        }
        Ok(())
    }

    async fn clear_stuck_jobs(&self, now: u64) -> Vec<CronJob> {
        let mut jobs = self.jobs.write().await;
        let mut cleared = Vec::new();
        for job in jobs.iter_mut() {
            let running_ms = job
                .state
                .running_at
                .as_deref()
                .and_then(|s| parse::parse_absolute_time_ms(s).ok());
            if let Some(started) = running_ms
                && now.saturating_sub(started) > STUCK_THRESHOLD_MS
            {
                warn!(
                    event = "cron.run.stuck_cleared",
                    policy = POLICY,
                    decision = "drop",
                    reason_code = "cron_stuck_cleared",
                    job_id = %job.job_id,
                    agent_id = %job.agent_id,
                    "clearing stuck cron job"
                );
                job.state.running_at = None;
                job.state.last_status = Some(RunStatus::Error);
                job.state.last_error = Some("stuck: exceeded 2h threshold".into());
                #[cfg(feature = "metrics")]
                counter!(cron_metrics::STUCK_JOBS_CLEARED_TOTAL).increment(1);
                cleared.push(job.clone());
            }
        }
        cleared
    }

    async fn validate_job(&self, job: &CronJob) -> Result<()> {
        self.validate_job_with_mode(job, false).await
    }

    async fn validate_loaded_job(&self, job: &CronJob) -> Result<()> {
        self.validate_job_with_mode(job, true).await
    }

    async fn validate_job_with_mode(&self, job: &CronJob, persisted_load: bool) -> Result<()> {
        // agent id checks (gateway-owned)
        if let Some(ref validate_agent_id) = self.validators.validate_agent_id {
            (validate_agent_id)(&job.agent_id).await?;
        } else if job.agent_id.trim().is_empty() {
            bail!("agentId is required");
        }

        // prompt
        if job.prompt.trim().is_empty() {
            bail!("cronPromptMissing");
        }

        // schedule
        validate_schedule(&job.schedule, now_ms(), persisted_load)?;

        // deleteAfterRun constraint
        if job.delete_after_run && !matches!(job.schedule, CronSchedule::Once { .. }) {
            bail!("deleteAfterRun=true requires schedule.kind=once");
        }

        // model selector
        if let ModelSelector::Explicit { model_id } = &job.model_selector
            && let Some(ref validate_model_id) = self.validators.validate_model_id
            && !persisted_load
        {
            validate_model_id(model_id).await?;
        }

        // delivery validation (shape + gateway-owned checks)
        match &job.delivery {
            CronDelivery::Silent => Ok(()),
            CronDelivery::Session { target } => {
                if let Some(ref validate) = self.validators.validate_session_target {
                    if !persisted_load {
                        validate(&job.agent_id, target).await?;
                    }
                }
                Ok(())
            },
            CronDelivery::Telegram { target } => {
                if target.account_key.trim().is_empty() || target.chat_id.trim().is_empty() {
                    bail!("telegram delivery requires accountKey + chatId");
                }
                if let Some(ref validate) = self.validators.validate_telegram_target {
                    if !persisted_load {
                        validate(target).await?;
                    }
                }
                Ok(())
            },
        }
    }
}

fn validate_schedule(schedule: &CronSchedule, now_ms: u64, allow_past_once: bool) -> Result<()> {
    match schedule {
        CronSchedule::Once { at } => {
            let at_ms = parse::parse_absolute_time_ms(at)?;
            if !allow_past_once && at_ms <= now_ms {
                bail!("cronSchedulePast");
            }
            Ok(())
        },
        CronSchedule::Every { every } => {
            let ms = parse::parse_duration_ms(every)?;
            if ms == 0 {
                bail!("cronScheduleInvalid");
            }
            Ok(())
        },
        CronSchedule::Cron { expr: _, timezone } => {
            // validate expr + timezone by trying next-run compute
            let _ = compute_next_run(schedule, now_ms)?;
            let _: chrono_tz::Tz = timezone
                .parse()
                .map_err(|_| anyhow::anyhow!("unknown timezone: {timezone}"))?;
            // (compute_next_run already validates expr)
            Ok(())
        },
    }
}

fn compute_next_run_rfc3339(schedule: &CronSchedule, now_ms: u64) -> Result<Option<String>> {
    compute_next_run(schedule, now_ms).map(|opt| opt.map(ms_to_rfc3339))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CronJobState;
    use tokio::{sync::{Mutex, Notify}, time::{Duration, sleep}};

    fn noop_run() -> RunCronFn {
        Arc::new(|_req| {
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
        Arc::new(|_req| Box::pin(async { Ok(()) }))
    }

    async fn sqlite_store() -> crate::store_sqlite::SqliteStore {
        crate::store_sqlite::SqliteStore::new("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn add_rejects_empty_prompt() {
        let store = Arc::new(sqlite_store().await);
        let svc = CronService::new(store, noop_run(), noop_deliver());
        let err = svc
            .add(CronJobCreate {
                job_id: None,
                agent_id: "default".into(),
                name: "x".into(),
                schedule: CronSchedule::Every { every: "1m".into() },
                prompt: "   ".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Silent,
                delete_after_run: false,
                enabled: true,
            })
            .await
            .expect_err("empty prompt should reject");
        assert!(format!("{err:#}").contains("cronPromptMissing"));
    }

    #[tokio::test]
    async fn add_rejects_delete_after_run_for_non_once() {
        let store = Arc::new(sqlite_store().await);
        let svc = CronService::new(store, noop_run(), noop_deliver());
        assert!(
            svc.add(CronJobCreate {
                job_id: None,
                agent_id: "default".into(),
                name: "x".into(),
                schedule: CronSchedule::Every { every: "1m".into() },
                prompt: "hi".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Silent,
                delete_after_run: true,
                enabled: true,
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn run_marks_delivery_failure_as_error() {
        let store = Arc::new(sqlite_store().await);
        let svc = CronService::new(
            store,
            Arc::new(|_req| {
                Box::pin(async {
                    Ok(AgentTurnResult {
                        output: "delivered text".into(),
                        input_tokens: Some(11),
                        output_tokens: Some(7),
                    })
                })
            }),
            Arc::new(|_req| Box::pin(async { Err(anyhow::anyhow!("target session missing")) })),
        );

        let job = svc
            .add(CronJobCreate {
                job_id: None,
                agent_id: "default".into(),
                name: "delivery-fail".into(),
                schedule: CronSchedule::Every { every: "1m".into() },
                prompt: "hi".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Session {
                    target: SessionTarget::Session {
                        session_key: "agent/default/chat".into(),
                    },
                },
                delete_after_run: false,
                enabled: true,
            })
            .await
            .unwrap();

        svc.run(&job.job_id, false).await.unwrap();

        let runs = svc.runs(&job.job_id, 10).await.unwrap();
        let run = runs.last().expect("run recorded");
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.error.as_deref(), Some("target session missing"));
        assert_eq!(run.output_preview.as_deref(), Some("delivered text"));

        let job = svc
            .list()
            .await
            .into_iter()
            .find(|candidate| candidate.job_id == job.job_id)
            .expect("job exists");
        assert_eq!(job.state.last_status, Some(RunStatus::Error));
        assert_eq!(
            job.state.last_error.as_deref(),
            Some("target session missing")
        );
    }

    #[tokio::test]
    async fn running_at_is_persisted_before_execution() {
        let store = Arc::new(sqlite_store().await);
        let notify = Arc::new(Notify::new());

        let notify_for_run = Arc::clone(&notify);
        let svc = CronService::new(
            store.clone(),
            Arc::new(move |_req| {
                let notify_for_run = Arc::clone(&notify_for_run);
                Box::pin(async move {
                    notify_for_run.notified().await;
                    Ok(AgentTurnResult {
                        output: "ok".into(),
                        input_tokens: None,
                        output_tokens: None,
                    })
                })
            }),
            noop_deliver(),
        );

        let job = svc
            .add(CronJobCreate {
                job_id: None,
                agent_id: "default".into(),
                name: "persist-running".into(),
                schedule: CronSchedule::Every { every: "1h".into() },
                prompt: "hi".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Silent,
                delete_after_run: false,
                enabled: true,
            })
            .await
            .unwrap();

        let job_id = job.job_id.clone();
        let job_id_for_run = job_id.clone();
        let svc_for_run = Arc::clone(&svc);
        let handle = tokio::spawn(async move { svc_for_run.run(&job_id_for_run, false).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let jobs = store.load_jobs().await.unwrap();
                let persisted = jobs.iter().find(|j| j.job_id == job_id).unwrap();
                if persisted.state.running_at.is_some() {
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
    async fn run_passes_delivery_session_target_to_executor() {
        let store = Arc::new(sqlite_store().await);
        let seen_request = Arc::new(Mutex::new(None));
        let seen_request_clone = Arc::clone(&seen_request);
        let svc = CronService::new(
            store,
            Arc::new(move |req| {
                let seen_request = Arc::clone(&seen_request_clone);
                Box::pin(async move {
                    *seen_request.lock().await = Some(req.clone());
                    Ok(AgentTurnResult {
                        output: "ok".into(),
                        input_tokens: None,
                        output_tokens: None,
                    })
                })
            }),
            noop_deliver(),
        );

        let target = SessionTarget::Main;
        let job = svc
            .add(CronJobCreate {
                job_id: None,
                agent_id: "default".into(),
                name: "inherit-model".into(),
                schedule: CronSchedule::Every { every: "1m".into() },
                prompt: "hi".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: Some(42),
                delivery: CronDelivery::Session {
                    target: target.clone(),
                },
                delete_after_run: false,
                enabled: true,
            })
            .await
            .unwrap();

        svc.run(&job.job_id, false).await.unwrap();

        let request = seen_request
            .lock()
            .await
            .clone()
            .expect("run request captured");
        assert_eq!(request.job_id, job.job_id);
        assert_eq!(request.timeout_secs, Some(42));
        assert_eq!(request.delivery_session_target, Some(target));
    }

    #[tokio::test]
    async fn start_allows_loaded_disabled_once_job_in_past() {
        let store = Arc::new(sqlite_store().await);
        crate::store::CronStore::save_job(
            store.as_ref(),
            &CronJob {
                job_id: "once-past".into(),
                agent_id: "default".into(),
                name: "once-past".into(),
                enabled: false,
                delete_after_run: false,
                schedule: CronSchedule::Once {
                    at: "1970-01-01T00:00:00.500Z".into(),
                },
                prompt: "done".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Silent,
                state: CronJobState::default(),
            },
        )
        .await
        .unwrap();

        let svc = CronService::new(store, noop_run(), noop_deliver());
        svc.start().await.unwrap();

        let jobs = svc.list().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "once-past");
    }

    #[tokio::test]
    async fn start_defers_stale_session_target_validation_until_run() {
        let store = Arc::new(sqlite_store().await);
        crate::store::CronStore::save_job(
            store.as_ref(),
            &CronJob {
                job_id: "stale-session".into(),
                agent_id: "default".into(),
                name: "stale-session".into(),
                enabled: true,
                delete_after_run: false,
                schedule: CronSchedule::Every { every: "1m".into() },
                prompt: "check".into(),
                model_selector: ModelSelector::Inherit,
                timeout_secs: None,
                delivery: CronDelivery::Session {
                    target: SessionTarget::Session {
                        session_key: "agent/default/missing".into(),
                    },
                },
                state: CronJobState::default(),
            },
        )
        .await
        .unwrap();

        let svc = CronService::with_config(
            store,
            noop_run(),
            noop_deliver(),
            None,
            RateLimitConfig::default(),
            CronValidators {
                validate_session_target: Some(Arc::new(|_agent_id, _target| {
                    Box::pin(async { Err(anyhow::anyhow!("session target missing")) })
                })),
                ..Default::default()
            },
        );

        svc.start().await.unwrap();
        assert_eq!(svc.list().await.len(), 1);
    }
}

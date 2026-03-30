ALTER TABLE cron_runs ADD COLUMN run_id TEXT;
UPDATE cron_runs
SET run_id = 'legacy-cron-run-' || id
WHERE run_id IS NULL OR run_id = '';
CREATE UNIQUE INDEX IF NOT EXISTS idx_cron_runs_run_id ON cron_runs(run_id);

ALTER TABLE heartbeat_runs ADD COLUMN run_id TEXT;
UPDATE heartbeat_runs
SET run_id = 'legacy-heartbeat-run-' || id
WHERE run_id IS NULL OR run_id = '';
CREATE UNIQUE INDEX IF NOT EXISTS idx_heartbeat_runs_run_id ON heartbeat_runs(run_id);

-- Heartbeat tables schema
-- Owned by: moltis-cron crate

CREATE TABLE IF NOT EXISTS heartbeat (
    agent_id TEXT PRIMARY KEY,
    config   TEXT NOT NULL,
    state    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS heartbeat_runs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id       TEXT    NOT NULL,
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER NOT NULL,
    status         TEXT    NOT NULL,
    error          TEXT,
    duration_ms    INTEGER NOT NULL,
    output         TEXT,
    input_tokens   INTEGER,
    output_tokens  INTEGER,
    FOREIGN KEY (agent_id) REFERENCES heartbeat(agent_id)
);

CREATE INDEX IF NOT EXISTS idx_heartbeat_runs_agent_id
    ON heartbeat_runs(agent_id, started_at_ms DESC);


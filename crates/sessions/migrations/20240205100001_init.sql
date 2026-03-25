-- Sessions table schema
-- Owned by: moltis-sessions crate

CREATE TABLE IF NOT EXISTS sessions (
    session_id              TEXT    PRIMARY KEY,
    session_key             TEXT    NOT NULL,
    label                   TEXT,
    model                   TEXT,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    message_count           INTEGER NOT NULL DEFAULT 0,
    project_id              TEXT,
    archived                INTEGER NOT NULL DEFAULT 0,
    worktree_branch         TEXT,
    sandbox_enabled         INTEGER,
    sandbox_image           TEXT,
    channel_binding         TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);
CREATE INDEX IF NOT EXISTS idx_sessions_session_key ON sessions(session_key);

CREATE TABLE IF NOT EXISTS active_sessions (
    session_key TEXT    PRIMARY KEY,
    session_id  TEXT    NOT NULL,
    updated_at  INTEGER NOT NULL
);

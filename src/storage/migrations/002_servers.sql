-- M3: servers table tracks live rover mcp instances for multi-instance
-- coordination (design supplement §2.3). Each running server upserts a row
-- with its OS PID on startup, refreshes last_heartbeat every few seconds,
-- and deletes its row on graceful shutdown. Stale rows are reaped on the
-- next startup.

CREATE TABLE IF NOT EXISTS servers (
    pid             INTEGER PRIMARY KEY,
    version         TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    last_heartbeat  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS servers_heartbeat ON servers(last_heartbeat);

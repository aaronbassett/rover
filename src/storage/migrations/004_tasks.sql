-- M6: tasks + task_events.
--
-- Tasks survive process restarts; owner_pid links to the servers table from
-- M3 (multi-instance design supplement §2.3). task_events is append-only; the
-- (task_id, id) index drives the `rover ... --monitor` poll loop.
--
-- Timestamps are epoch milliseconds. This is a unit divergence from M2's
-- pages.fetched_at (epoch seconds) — see storage::tasks for the rationale
-- (sub-second ordering matters for event streams).

CREATE TABLE IF NOT EXISTS tasks (
    id                      TEXT PRIMARY KEY,
    kind                    TEXT NOT NULL,
    status                  TEXT NOT NULL,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    params_json             TEXT NOT NULL,
    result_json             TEXT,
    error                   TEXT,
    cancellation_requested  INTEGER NOT NULL DEFAULT 0,
    owner_pid               INTEGER
);

CREATE INDEX IF NOT EXISTS tasks_status_kind  ON tasks(status, kind);
CREATE INDEX IF NOT EXISTS tasks_owner_status ON tasks(owner_pid, status);
CREATE INDEX IF NOT EXISTS tasks_created_at   ON tasks(created_at);

CREATE TABLE IF NOT EXISTS task_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       TEXT NOT NULL,
    ts            INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS task_events_by_task ON task_events(task_id, id);

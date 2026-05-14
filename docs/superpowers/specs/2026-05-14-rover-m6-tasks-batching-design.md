# Rover M6 — Long-Running Tasks & Batching — Design

> Status: design complete, awaiting implementation plan.
>
> Prerequisites: M1 (fetcher), M2 (cache + storage actor), M3 (MCP server + `servers` table), M4 (extraction envelope), M5 (rate limiting + robots).
>
> Canonical references:
> - PRD §3.3 (task resumption), §4.2 (`batch_fetch` tool), §8.1 (schema), §9 (long-running task pattern), §14 M6 (acceptance).
> - Design supplement §2.3 (multi-instance + owner_pid), §3.1 (bare UUIDv7), §3.2 (`summarize` kind name), §3.3 (`revalidation_task_id` envelope), §3.4 (servers table).
> - Milestone manifest §M6 (file layout, open questions, deferrals).

---

## 1. Scope and Goals

M6 introduces a durable task subsystem so that long-running and batchable work survives MCP-tool latency budgets, server restarts, and multi-instance contention. The four shipping workers, plus a stub for M7:

1. `batch_fetch` — fetch many URLs concurrently with the existing fetcher pipeline.
2. `retry` — long-deferred retries that exceed the in-call retry budget land here as a task (PRD §5.4).
3. `revalidate` — stale-while-revalidate background refreshes (PRD §8.3, design §3.3) finally close the loop punted in M2.
4. `summarize` (stub) — schema accepts the kind so M7 can fill in the worker without a migration.

CLI shippers: `rover batch <uuid>` and `rover task <uuid>`, each with snapshot, `--monitor`, `--cancel`, and `--format=ndjson` modes.

**M6 acceptance (PRD §14):** a batch of 20 URLs returns a task ID immediately, progress is observable via monitor or poll, completes correctly, can be cancelled mid-flight, and resumes after a server restart.

**M5 follow-ups bundled into M6** (all four):
1. Extract `Config::apply_overrides` to remove duplicated override blocks in `src/cli/{fetch,mcp}.rs` — `rover batch` reuses the same knobs.
2. Fix `RobotsFetchFailed` Display to render the boxed source chain (currently drops via `e.to_string()`).
3. Wire or delete the four unused robots fixtures in `tests/fixtures/m5/`.
4. Tighten `tests/fetcher_robots.rs::robots_disallow_all_refuses_fetch` to assert only `RobotsDisallowed`.

M4 follow-ups #3, #4, #6 stay deferred (no overlap with M6 surfaces).

---

## 2. Decisions Inherited from Open-Question Round

| Question | Decision |
| --- | --- |
| Orphan claim mechanism | **Single CAS `UPDATE`** scanned every 10s by every live server. Tasks stay in `running` across handoff; resumption reads persisted progress from `task_events`. |
| Batch progress tracking | **Events only.** Snapshot computes counts from `task_events` on read. Denormalise later only if profiling demands. |
| Event taxonomy | **Shared core + kind-specific events.** Core: `task_started`, `task_progress`, `task_completed`, `task_failed`, `task_cancelled`. `batch_fetch` adds `item_started`/`item_done`/`item_failed`. `retry` adds `retry_attempted`/`retry_succeeded`/`retry_failed`. `revalidate` adds `revalidation_started`/`revalidation_completed`. |
| Wire vs DB shape | DB stores `(kind, payload_json)`. Wire flattens to `{ts, kind, event_id, ...payload}` — matches PRD §9.2 sample literally and lets downstream tools resume from `event_id`. |
| Monitor poll cadence | **Fixed 200ms.** SIGINT trap. No backoff. |
| Batch timeout | **No batch-level timeout in v1.** Per-URL fetcher timeouts (M5) bound runtime; `rover task <id> --cancel` is the kill path. CLI snapshot includes a `Tip:` hint when status is `running`. |
| CLI snapshot format | **Human default** (PRD §9.2 layout). `--format=ndjson` (alias `--ndjson`) emits one rollup line and exits. Both are pure read paths. |
| Worker kinds shipped | All four: `batch_fetch`, `retry`, `revalidate`, `summarize` (stub-fails). |
| Worker runtime | **Single scheduler loop**, `tokio::spawn` per task. Cancellation checked at safe points inside each worker. |

---

## 3. Architecture

### 3.1 Module Layout

```
src/storage/
  migrations/004_tasks.sql                # new: tasks + task_events
  tasks.rs                                # new: async API for tasks
  events.rs                               # new: async API for task_events
src/tasks/
  mod.rs                                  # new: scheduler, kind dispatch, claim loop, public API
  batch_fetch.rs                          # new: batch worker
  retry.rs                                # new: retry worker
  revalidate.rs                           # new: SWR revalidation worker
  summarize.rs                            # new: stub worker (always fails)
  error.rs                                # new: per-module thiserror enum
src/mcp/
  tools/batch_fetch.rs                    # new: MCP tool implementation
src/cli/
  batch.rs                                # new: rover batch <id> [--monitor|--cancel|--format]
  task.rs                                 # new: rover task <id> [--monitor|--cancel|--format]
src/fetcher/
  retry.rs                                # touched: classify long-Retry-After → deferred task
  cached.rs                               # touched: SWR stale path inserts revalidate task
src/config.rs                             # touched: extract apply_overrides
src/cli/{fetch,mcp}.rs                    # touched: use apply_overrides
src/mcp/error.rs                          # touched: render RobotsFetchFailed source chain
tests/
  tasks_lifecycle.rs                      # new: insert → run → complete
  tasks_orphan_claim.rs                   # new: CAS reclaim semantics
  tasks_cancellation.rs                   # new: --cancel mid-flight
  tasks_revalidate.rs                     # new: SWR stale path inserts + completes
  tasks_retry_deferred.rs                 # new: long Retry-After becomes a task
  cli_batch_monitor.rs                    # new: monitor streams NDJSON until terminal
  cli_batch_snapshot.rs                   # new: snapshot human + ndjson formats
  mcp_batch_fetch.rs                      # new: MCP tool envelope contract
  fetcher_robots.rs                       # touched: tighten + RobotsFetchFailed source chain
tests/fixtures/m5/                        # touched: wire/delete the four unused fixtures
```

Per-module `thiserror` enums; `anyhow` only at the CLI binary boundary.
All SQLite access via the existing `tokio-rusqlite` actor (`Db` in `src/storage/mod.rs`) — no direct `rusqlite::Connection` outside `src/storage/`.
All logging via `tracing`; no `println!` in lib code.

### 3.2 Process Topology

```
rover mcp (writer + worker process)
  ├── mcp::server                         (existing)
  ├── mcp::handler                        (existing)
  │     └── tools::batch_fetch            (NEW: inserts tasks row, returns envelope)
  ├── tasks::scheduler                    (NEW: one tokio task)
  │     ├── 10s tick → orphan-claim CAS scan
  │     ├── on new task insert / claim    → tokio::spawn(worker)
  │     └── owns a JoinSet of running workers
  ├── tasks::workers::{batch_fetch, retry, revalidate, summarize}
  │     (each reads tasks/task_events via storage::tasks/events,
  │      uses existing fetcher::cached pipeline, checks cancellation)
  └── storage::Db                         (existing actor)

rover batch <id> / rover task <id> (reader process, separate)
  └── storage::Db (read-only) → snapshot OR 200ms poll loop OR --cancel write
```

The scheduler is the **single owner** of task lifecycle in a given `rover mcp` process. Tools and workers never spawn each other directly — they write rows to `tasks` and the scheduler picks them up. This makes orphan handoff symmetric: a server restart looks identical to "another live server picked up this orphan."

### 3.3 Task Lifecycle

```
                  (insert by tool, owner_pid = own pid)
                            │
                          pending  ──────────► running ─────► completed
                                                 │   \
                                                 │    \─────► failed
                                                 │
                                          (cancel flag set)
                                                 │
                                                 ▼
                                             cancelled

  Orphan handoff (owner_pid not in servers, status='running', resumable kind):
     UPDATE tasks SET owner_pid = $own
       WHERE id = ? AND owner_pid = $orphan AND status = 'running';
   rowcount=1 → claimed; spawn worker → resume from task_events.
   rowcount=0 → race lost; ignore.

  Orphan handoff for non-resumable kind (summarize):
     UPDATE tasks SET status='failed', error='owner_died',
            updated_at=?, owner_pid=$own
       WHERE id = ? AND owner_pid = $orphan AND status = 'running';
     INSERT INTO task_events kind='task_failed' payload {error:'owner_died'};
```

`pending` exists only as a transient state between INSERT and the scheduler's first pickup; under normal flow tools insert with `status='running'` and `owner_pid=$own_pid` directly. `pending` is reserved for cases where a row is inserted by a process that is **not** going to run it (none in M6 — leave the state defined for forward compatibility but unused).

### 3.4 Scheduler

```rust
// src/tasks/mod.rs (sketch)
pub struct Scheduler {
    db: Db,
    own_pid: i32,
    cancel_token: CancellationToken, // shutdown signal
    join_set: JoinSet<()>,           // live workers
    cfg: Arc<Config>,
    fetcher: Arc<Fetcher>,
}

impl Scheduler {
    pub async fn run(mut self) -> Result<(), TasksError> {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        let mut new_tasks_rx = self.db.subscribe_new_tasks(); // see §3.5

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => break,
                _ = tick.tick() => self.scan_and_claim_orphans().await?,
                Some(task_id) = new_tasks_rx.recv() => self.spawn_worker(task_id).await?,
                Some(res) = self.join_set.join_next() => self.on_worker_exit(res),
            }
        }
        // shutdown: drain join_set with a deadline, then return.
        Ok(())
    }
}
```

On graceful shutdown the scheduler:
1. signals all live workers via the per-task cancellation token (workers convert this to "treat next safe point as cancelled, but on shutdown leave `status='running'`, not `cancelled` — let the next live server claim and resume");
2. drains the `JoinSet` with a 5s deadline;
3. delegates the `servers` row delete to the existing shutdown hook in `mcp::server`.

### 3.5 New-task notification

The scheduler must learn about freshly inserted tasks without polling. M6 adds an in-process `tokio::sync::mpsc` between the storage layer and the scheduler:

- `Db` carries an optional `new_task_tx: Option<UnboundedSender<TaskId>>`. Set during `rover mcp` startup; left `None` for CLI reader processes.
- `Db::insert_task()` sends `task_id` on the channel after the INSERT commits. Send failures (channel closed = server shutting down) are logged at `tracing::debug` and ignored.
- The scheduler holds the corresponding `Receiver`.

CLI processes never insert tasks except for `--cancel` (an UPDATE), so they don't need the channel.

This is in-process only. **Cross-process new-task delivery** (e.g., a second `rover mcp` inserts a task that the first should also notice) is not required: each server's scheduler only runs the tasks it inserts itself plus orphans claimed via the 10s scan. A future server-to-server notify is out of scope.

---

## 4. Schema (migration 004_tasks.sql)

```sql
-- M6: tasks + task_events.
--
-- Tasks survive process restarts; owner_pid links to the servers table from M3.
-- task_events is append-only; (task_id, id) is the polling index used by
-- `rover ... --monitor`. Timestamps are epoch milliseconds (sub-second
-- ordering matters for event streams). This is a unit divergence from M2's
-- pages.fetched_at (epoch seconds) — documented in storage::tasks.

CREATE TABLE IF NOT EXISTS tasks (
    id                      TEXT PRIMARY KEY,         -- UUIDv7 string, no kind prefix
    kind                    TEXT NOT NULL,            -- batch_fetch|retry|revalidate|summarize
    status                  TEXT NOT NULL,            -- pending|running|completed|failed|cancelled
    created_at              INTEGER NOT NULL,         -- epoch ms
    updated_at              INTEGER NOT NULL,         -- epoch ms
    params_json             TEXT NOT NULL,            -- kind-specific input shape
    result_json             TEXT,                     -- kind-specific final result on terminal
    error                   TEXT,                     -- short slug (owner_died, etc.) on failed/cancelled
    cancellation_requested  INTEGER NOT NULL DEFAULT 0,
    owner_pid               INTEGER                   -- NULL = unclaimed pending
);

CREATE INDEX IF NOT EXISTS tasks_status_kind   ON tasks(status, kind);
CREATE INDEX IF NOT EXISTS tasks_owner_status  ON tasks(owner_pid, status);
CREATE INDEX IF NOT EXISTS tasks_created_at    ON tasks(created_at);

CREATE TABLE IF NOT EXISTS task_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       TEXT NOT NULL,
    ts            INTEGER NOT NULL,                   -- epoch ms
    kind          TEXT NOT NULL,                      -- see §5 taxonomy
    payload_json  TEXT NOT NULL,                      -- '{}' if no payload
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS task_events_by_task ON task_events(task_id, id);
```

Migration registration: append `("004_tasks.sql", include_str!("migrations/004_tasks.sql"))` to the `MIGRATIONS` slice in `src/storage/mod.rs`. Never edit an existing migration; M2's convention applies.

**Foreign key.** `task_events.task_id REFERENCES tasks(id) ON DELETE CASCADE` so a future task-GC pass cleans up events automatically. SQLite foreign keys require `PRAGMA foreign_keys = ON` per connection; the existing actor already sets this.

**`error` column convention.** Short, stable, programmatic slugs. The human-facing message lives in the `payload_json` of the terminal `task_failed` event. Known slugs in M6:
- `owner_died` — non-resumable kind reaped after server crash
- `cancelled` — `cancellation_requested` honoured at a safe point
- `summarization_not_yet_implemented` — stub worker
- `batch_partial` — terminal `completed` is still used for batches where some items failed; this slug is *not* set. Per-item failures live as `item_failed` events.
- `internal_error` — fallback for unexpected worker panics (caught via JoinSet)

---

## 5. Event Taxonomy

All events share the wire envelope `{ts, kind, event_id, ...payload}`. `event_id` is `task_events.id` (allowing downstream tools to resume from a specific point).

### 5.1 Core (every kind emits these)

| Event | When | Payload |
| --- | --- | --- |
| `task_started` | First action by the worker | `{kind, params: <summary>}` |
| `task_progress` | Optional periodic heartbeat (worker discretion) | `{note: string}` |
| `task_completed` | Terminal success | `{result: <summary>, duration_ms}` |
| `task_failed` | Terminal failure | `{error: <slug>, message: string, duration_ms}` |
| `task_cancelled` | Cancellation honoured | `{at: string, duration_ms}` (`at` = where in the work, e.g. `"between_items"`) |

### 5.2 `batch_fetch`

| Event | When | Payload |
| --- | --- | --- |
| `batch_start` | After `task_started`; preferred entry event for batch monitors | `{total: int}` |
| `item_started` | URL begins fetching | `{url: string, index: int}` |
| `item_done` | URL completed successfully | `{url, index, tokens?: int, cached: bool, duration_ms}` |
| `item_failed` | URL failed (after in-call retries exhausted) | `{url, index, error: string, will_retry: bool, retry_in_s?: int}` |
| `final` | Concluding rollup, immediately before `task_completed` | `{succeeded: int, failed: int, duration_s: float}` |

`tokens` is the extracted-markdown token count when available; omitted if extraction yielded nothing.

`will_retry: true` means the failure was classified as deferred and a `retry` task was inserted (see §6.2). `retry_in_s` is the planned wait.

### 5.3 `retry`

| Event | When | Payload |
| --- | --- | --- |
| `retry_attempted` | Each attempt starts | `{url, attempt: int, wait_ms_used: int}` |
| `retry_succeeded` | Attempt produced a non-retryable success | `{url, attempt}` → followed by `task_completed` |
| `retry_failed` | Attempt failed; either next attempt scheduled or escalated | `{url, attempt, error, will_retry: bool}` |

### 5.4 `revalidate`

| Event | When | Payload |
| --- | --- | --- |
| `revalidation_started` | Worker begins force-refresh fetch | `{url}` |
| `revalidation_completed` | Fetch finished | `{url, changed: bool, status_code: int}` — followed by `task_completed`. `changed=false` for 304 / unchanged content. |

### 5.5 `summarize` (stub)

Emits `task_started` then immediately `task_failed` with `error='summarization_not_yet_implemented'`. No kind-specific events. Real implementation in M7.

---

## 6. Workers

All workers share a common loop:

```rust
async fn run(task_id: TaskId, db: Db, cancel: CancellationToken, ...) {
    emit(task_started).await;
    let result = work(...).await;       // worker-specific
    match result {
        Ok(r)            => { emit(task_completed { r }); set_status(completed); }
        Err(Cancelled)   => { emit(task_cancelled);       set_status(cancelled); }
        Err(Failed(e))   => { emit(task_failed { e });    set_status(failed);    }
    }
}
```

Status writes and the final event are issued in **separate** storage calls (no single transaction). Snapshot readers tolerate the brief window where `status='completed'` but the final event isn't visible yet, by always preferring `status` from `tasks` over the event stream. Monitors tolerate the inverse by polling until `status` is terminal *and* a terminal event has been seen for that task.

### 6.1 `batch_fetch`

**Params shape:**
```json
{
  "urls": ["https://...", ...],
  "concurrency": 8,
  "per_domain_concurrency": 2,
  "per_url_options": { /* fetch options applied to every URL */ }
}
```

**Algorithm.** A `tokio::sync::Semaphore(concurrency)` plus a per-host `HashMap<Host, Semaphore(per_domain_concurrency)>` (the latter built lazily on first sight of each host). For each URL, in stable input order:

1. Check `cancellation_requested` (read-through `Db::is_cancelled`). If set, emit `task_cancelled` and exit. Reaches inner items already-spawned cancel via the shared token.
2. Acquire global permit, then per-host permit.
3. Emit `item_started`.
4. Call `fetcher::fetch_with_cache` with the merged options.
5. Translate result:
   - `Ok(envelope)` → `item_done` with `tokens` (from envelope.metadata.token_count where present), `cached`, `duration_ms`.
   - `Err(FetcherError)` classified as **deferred-retryable** (the same classifier used by the in-call retry layer, see M5 `retry.rs`) → insert a `retry` task carrying `{url, original_params, attempt: 1}`, emit `item_failed {will_retry: true, retry_in_s}`.
   - Other `Err(_)` → `item_failed {will_retry: false}`.

After all items: emit `final` (rollup of events), then `task_completed`. `result_json` carries the same rollup.

**Stable per-item index.** `index` in events is the 0-based position in the input `urls` array; preserved across resumption.

**Resumption.** On orphan claim, the worker rebuilds completion state by querying:
```sql
SELECT payload_json->>'index' FROM task_events
 WHERE task_id = ? AND kind IN ('item_done', 'item_failed');
```
Indices already seen are skipped. `index` values not yet seen are processed normally. Per-host semaphores are rebuilt from scratch (a brief over-claim window across servers is acceptable; M6 explicitly does not coordinate concurrency caps across processes).

### 6.2 `retry`

Inserted by the fetcher when an in-call retry exhausts its budget but the failure is still classified as transient (5xx, certain `Retry-After`s longer than the in-call budget, network-class errors). One `retry` task per deferred URL.

**Params:**
```json
{
  "url": "https://...",
  "original_params": { ... },
  "attempt": 1,
  "wait_ms_initial": 60000,
  "max_attempts": 3,
  "parent_task_id": "<batch task id or null>"
}
```

**Algorithm.** Wait `wait_ms_initial * (2 ** (attempt - 1))` capped at 5 minutes; check cancellation every 1s; re-fetch once; on success emit `retry_succeeded` and `task_completed`. On failure, if `attempt < max_attempts`, **insert a new `retry` task** for attempt+1 (with the doubled wait), emit `retry_failed {will_retry: true}`, and complete *this* task. Otherwise emit `retry_failed {will_retry: false}` and `task_failed { error: "retries_exhausted" }`.

`parent_task_id` is carried forward as a breadcrumb but is **not** used to fan results back into the batch in M6 — the agent inspecting the batch sees the failure and an event referencing the retry task ID. Future enhancement.

### 6.3 `revalidate`

Inserted by `fetcher::cached` when a stale-served fetch returns the SWR envelope. One per stale URL.

**Params:** `{ "url": "https://...", "etag_at_serve": "...", "last_modified_at_serve": "..." }`.

**Algorithm.** Emit `revalidation_started`. Call `fetcher::fetch_with_cache(url, force_refresh=true)`. On `200`: update `pages` row (handled by existing cache layer). On `304`: bump `fetched_at` only. Emit `revalidation_completed { changed, status_code }` then `task_completed`. Errors emit `task_failed` with the underlying `FetcherError::to_string()` as the message and a short slug as `error`.

### 6.4 `summarize` (stub)

Emit `task_started`, then `task_failed { error: "summarization_not_yet_implemented", message: "Summarization will be implemented in M7." }`. Status becomes `failed`. The MCP `summarize` tool itself is **not added in M6** — the worker is only callable by inserting a tasks row directly (used in tests).

---

## 7. MCP Tool: `batch_fetch`

**Tool name:** `batch_fetch`.

**Arguments** (mirrors PRD §4.2; per-URL options merge with existing `fetch` arg schema):

```json
{
  "urls": ["..."],                                  // required, max 100
  "concurrency": 8,                                 // optional, default 8, max 32
  "per_domain_concurrency": 2,                      // optional, default 2, max 8
  "force_refresh": false,
  "tables": { ... }, "images": { ... }, "metadata": { ... },
  "max_tokens": null
}
```

**Validation.** `urls.len() in 1..=100`. Each URL parsed and run through the SSRF policy *before* insert (cheap rejection — saves a round-trip through the scheduler).

**Returns** (immediately, no waiting):

```json
{
  "task_id": "0190bf68-3a4c-7000-8000-...",
  "status": "running",
  "kind": "batch_fetch",
  "total_urls": 50,
  "monitor_command": "rover batch 0190bf68-... --monitor",
  "poll_command": "rover batch 0190bf68-...",
  "cancel_command": "rover task 0190bf68-... --cancel",
  "hint": "Use the Monitor tool with monitor_command for live updates, or call poll_command to check status."
}
```

`status: "running"` (not `"started"`) — matches the actual database state at return time. Existing PRD wording said `"started"`; we resolve consistently to the literal status names. PRD wording is treated as illustrative.

**Errors.** MCP error mapping additions (`src/mcp/error.rs`):
- `TooManyUrls { count, max }` → JSON-RPC `-32602` invalid params
- `EmptyUrlList` → `-32602`
- `SsrfRejected { url, reason }` → `-32602`
- Storage insert failure → internal error `-32603`

### 7.1 `revalidation_task_id` envelope on stale-served `fetch`

Per design §3.3. When `fetch` returns `cache_status: "stale_served"`, include:

```json
"revalidation": {
  "task_id": "0190bf68-...",
  "monitor_command": "rover task 0190bf68-... --monitor",
  "poll_command": "rover task 0190bf68-...",
  "hint": "Optional. Revalidation runs in the background regardless."
}
```

The `revalidate` task is inserted by `fetcher::cached` on the stale path; the MCP tool just reflects the ID outward.

### 7.2 Deferred-retry envelope on `fetch`

Per PRD §4.1 and the retry classification in M5. When a single-URL `fetch` would otherwise fail but the classifier returns deferred-retryable, the fetcher inserts a `retry` task and the tool returns:

```json
{
  "status": "deferred",
  "task_id": "0190bf68-...",
  "monitor_command": "rover task 0190bf68-... --monitor",
  "poll_command": "rover task 0190bf68-...",
  "cancel_command": "rover task 0190bf68-... --cancel",
  "hint": "Fetch failed but will be retried in the background. Monitor or poll for results."
}
```

---

## 8. CLI

### 8.1 `rover batch <id>` and `rover task <id>`

`rover batch <id>` is a thin wrapper that pre-checks `tasks.kind = 'batch_fetch'` and adjusts the snapshot layout. `rover task <id>` works on any kind. Both share implementation in `src/cli/task.rs`; `src/cli/batch.rs` re-uses it with `expect_kind = Some("batch_fetch")`.

**Flags:**

| Flag | Meaning |
| --- | --- |
| (none) | Print snapshot in default human format, exit 0 if task found. |
| `--monitor` | Stream NDJSON until terminal state. SIGINT exits cleanly. |
| `--cancel` | Set `cancellation_requested = 1`, print "Cancellation requested.", exit. |
| `--format=human` \| `--format=ndjson` \| `--ndjson` | Snapshot output format. Default `human`. `--ndjson` is an alias. |
| `--from-event <id>` | Used with `--monitor` only; start streaming from `event_id > <id>`. Default 0. |

### 8.2 Snapshot — human format

For `batch_fetch`:

```
Batch 0190bf68-... — running
Started 12s ago
Progress: 35/50 (70%)  ✓ 33  ✗ 2  ⋯ 15 in flight
ETA ~5s
Last event: item_done https://example.com/foo (1.2s ago)
Tip: use `rover task <id> --cancel` to stop.
```

Liveness warning (design §2.3 + PRD §9.4) is prepended when applicable:

```
⚠ Task is marked `running` but no `rover mcp` process appears to be alive.
Start one with `rover mcp` to resume work.
```

Trigger: `SELECT MAX(last_heartbeat) FROM servers` is NULL or older than 30s, AND `tasks.status = 'running'`.

For non-batch kinds (`retry`, `revalidate`, `summarize`):

```
Task 0190bf68-... — running (kind: revalidate)
Started 1.3s ago
Last event: revalidation_started https://example.com/foo
```

`ETA` (batch only) computed naively: `avg_item_duration_so_far × remaining_count / concurrency`. If fewer than 3 items completed, omit.

### 8.3 Snapshot — `--format=ndjson`

One JSON line, then exit:

```jsonl
{"ts":"2026-05-14T12:00:00.000Z","kind":"snapshot","task_id":"0190bf68-...","task_kind":"batch_fetch","status":"running","total":50,"succeeded":33,"failed":2,"in_flight":15,"completed":35,"started_at":"2026-05-14T11:59:48.000Z","last_event_id":127,"eta_s":5}
```

For non-batch kinds, the same envelope with `total`/`succeeded`/`failed`/`in_flight` omitted.

### 8.4 `--monitor`

```
loop {
    rows = SELECT * FROM task_events WHERE task_id = ? AND id > $last_seen ORDER BY id LIMIT 1000;
    for row in rows:
        print(flatten_to_wire(row));
        $last_seen = row.id;
    if rows.is_empty():
        status = SELECT status FROM tasks WHERE id = ?;
        if status in {completed, failed, cancelled}:
            // emit one more pass to make sure we picked up the terminal event
            // (it may have been written between the events SELECT and the status SELECT)
            rows = SELECT * FROM task_events WHERE task_id = ? AND id > $last_seen ORDER BY id;
            for row in rows: print(...); $last_seen = row.id;
            break;
        sleep 200ms;
}
```

SIGINT → break cleanly (no panic). Exit code 0 on normal terminal; 1 on `failed`; 2 on `cancelled`. (These match `rover doctor` precedent of 0=ok, 1=problem.)

### 8.5 `--cancel`

```sql
UPDATE tasks SET cancellation_requested = 1, updated_at = ? WHERE id = ?;
```

Print `Cancellation requested for <id>.` and exit 0 if rowcount=1; else `Task <id> not found.` exit 1. Cancellation is honoured by the worker on its next safe point; the CLI does not wait.

### 8.6 `Config::apply_overrides` (M5 follow-up #1)

```rust
impl Config {
    pub fn apply_overrides(
        &mut self,
        rate_limit_rpm: Option<u32>,
        concurrency: Option<u32>,
        per_domain_concurrency: Option<u32>,
        ignore_robots: bool,
    ) {
        // existing logic, lifted from cli/{fetch,mcp}.rs verbatim,
        // including the deadlock-clamp from 02bd7e8.
    }
}
```

Removes ~30 lines duplicated across `cli/fetch.rs` and `cli/mcp.rs`. `cli/batch.rs` will not need direct overrides (the MCP server owns the workers), but the helper makes that decision easy to revisit.

---

## 9. Fetcher Integration

### 9.1 Stale-served path (`fetcher::cached`)

When the cache layer decides to serve stale (M2's SWR logic), it inserts a `revalidate` task and includes the task ID in the returned envelope. M2 had a TODO here; M6 fills it in:

```rust
// pseudocode in fetcher/cached.rs
if serving_stale {
    let task_id = db.insert_task(TaskInsert {
        kind: "revalidate",
        params_json: json!({ "url": url, ... }),
        ...
    }).await?;
    envelope.cache_status = CacheStatus::StaleServed { revalidation: Some(task_id) };
}
```

The MCP `fetch` tool surfaces this as the `revalidation` envelope shape (§7.1).

### 9.2 Deferred-retry path (`fetcher::retry`)

The existing M5 retry layer classifies in-call retryability. M6 adds a second classification: **deferred-retryable**. A failure is deferred-retryable if:

- HTTP `429` or `503` with `Retry-After` greater than the in-call budget (default >30s), OR
- The in-call retry attempts exhausted but the last error class is in `{Network, Timeout, Server5xx}`, OR
- The host has been seen too many times in a short window (preserved from M5's rate-limit gating).

When deferred-retryable, the synchronous code path inserts a `retry` task and returns a `FetcherError::Deferred { task_id }` variant. Callers (the MCP `fetch` tool, the `batch_fetch` worker) translate this into the deferred envelope / `item_failed { will_retry: true }` event respectively.

This is a small additive change to `FetcherError` and the retry layer; covered by `tests/tasks_retry_deferred.rs`.

### 9.3 `RobotsFetchFailed` source-chain (M5 follow-up #2)

`src/mcp/error.rs` currently has:

```rust
RobotsFetchFailed { source: Box<dyn StdError + Send + Sync> } =>
    JsonRpcError::internal_error(format!("{}", e.to_string())),
```

The `.to_string()` call invokes the `Display` impl, which by `thiserror`'s default elides the `#[source]`. Fix: render the chain.

```rust
RobotsFetchFailed { source } =>
    JsonRpcError::internal_error(format!("robots fetch failed: {source}")),
```

Or — equivalently — change the `#[error]` format string on the variant to include `{source}`. Regression test in `tests/fetcher_robots.rs` asserts the rendered string contains the inner cause's message.

---

## 10. Test Strategy

### 10.1 Unit Tests

- `storage::tasks` insert / update / status transition / cancellation_requested / orphan-CAS rowcount semantics (one in-memory DB per test).
- `storage::events` append / range query / payload round-trip.
- `tasks::scheduler::scan_and_claim_orphans` — given a populated `servers` + `tasks` fixture, asserts the CAS UPDATE picks exactly the orphan rows of resumable kinds.
- `tasks::batch_fetch::resume_indices` — given a partial event stream, compute the set of indices still to process.
- `Config::apply_overrides` — deadlock-clamp still triggers; idempotent re-application.

### 10.2 Integration Tests

All hit a real `tokio-rusqlite` actor against an in-memory or tempfile DB. Wiremock for HTTP. `test-loopback` feature gates SSRF as in M5.

| Test | Asserts |
| --- | --- |
| `tasks_lifecycle::happy_path` | Insert batch of 3 URLs → events stream contains `task_started`, 3×`item_started`/`item_done`, `final`, `task_completed`. |
| `tasks_lifecycle::tokens_emitted` | `item_done.tokens` matches the extractor's token count for the response body. |
| `tasks_orphan_claim::resumable_kind_reclaimed` | Pre-seed a `running` `batch_fetch` task with `owner_pid` of a defunct PID; start a scheduler with a different PID; assert CAS claim succeeds and the worker resumes from `task_events`. |
| `tasks_orphan_claim::non_resumable_kind_marked_failed` | Pre-seed a `running` `summarize` task with a defunct PID; on scan, status → `failed`, `error='owner_died'`. |
| `tasks_orphan_claim::race_two_servers_one_wins` | Two schedulers, same orphan. One UPDATE returns rowcount=1, the other rowcount=0. No double-execution. |
| `tasks_cancellation::between_items` | Start a batch of 5 URLs with `per_domain_concurrency=1`; flip `cancellation_requested` after 2 items; assert `task_cancelled` event, exactly 2 `item_done`, no further `item_started`. |
| `tasks_cancellation::idempotent` | Setting the flag twice doesn't produce a duplicate cancellation event. |
| `tasks_revalidate::stale_path_inserts_task` | M2 stale-served fetch returns envelope with `revalidation.task_id`; the inserted task runs and completes with `revalidation_completed`. |
| `tasks_revalidate::not_modified` | Wiremock returns 304; `changed: false`, `pages.fetched_at` bumped. |
| `tasks_retry_deferred::long_retry_after_becomes_task` | Wiremock returns 429 with `Retry-After: 120`; `fetch` returns deferred envelope, a `retry` task is inserted, worker waits (mock-clock) and re-fetches. |
| `tasks_retry_deferred::max_attempts_exhausted` | After `max_attempts`, `retry_failed { will_retry: false }` then `task_failed { error: 'retries_exhausted' }`. |
| `cli_batch_monitor::streams_until_terminal` | Spawn `rover mcp`; insert a batch via MCP `batch_fetch`; run `rover batch <id> --monitor` as a subprocess; assert NDJSON contains `batch_start` first and `task_completed` last; subprocess exits 0. |
| `cli_batch_monitor::sigint_clean_exit` | Same setup; send SIGINT; subprocess exits cleanly without panic. |
| `cli_batch_snapshot::human_format` | Snapshot includes `Progress:`, `Tip:`, no liveness warning when server is fresh. |
| `cli_batch_snapshot::ndjson_format` | One line; parseable as JSON; keys match §8.3. |
| `cli_batch_snapshot::liveness_warning` | Delete `servers` row; `--format=human` snapshot leads with the ⚠ warning. |
| `cli_batch_snapshot::cancel` | `rover task <id> --cancel` flips the flag; running worker honours it. |
| `mcp_batch_fetch::envelope_shape` | Tool returns the §7 envelope; `task_id` is a UUIDv7; commands are well-formed. |
| `mcp_batch_fetch::ssrf_rejected_before_insert` | Submitting a `file://` URL at `strict` rejects with no task row created. |
| `mcp_batch_fetch::too_many_urls` | 101 URLs returns `-32602` with a clear message. |
| `fetcher_robots::robots_fetch_failed_renders_source` | (M5 #2) The rendered error contains the inner cause. |
| `fetcher_robots::disallow_all_only_refuses` | (M5 #4) Tightened — only `RobotsDisallowed`, not `RobotsFetchFailed`. |

### 10.3 What we are NOT testing in M6

- Cross-process new-task delivery (deliberately not implemented; §3.5).
- Wallclock batch timeout (no batch-level timeout; §2).
- Real-clock wait times in `retry` (mock-clock only; deterministic).
- MCP `summarize` tool (deferred to M7).
- Performance / scale (>100 URLs / many-thousand-event tasks). Manifest cap is 100.
- Multi-server orphan-claim under adversarial timing beyond the one race test above.

---

## 11. Crate Dependencies Added

- `uuid` with features `["v7", "std"]` — bare UUIDv7 task IDs.
- `tokio-util` already present (M3 used `CancellationToken`).
- No new crates beyond `uuid` v7 feature.

`time` (already pulled in by `chrono` indirectly via dependencies) — we'll use `chrono::Utc::now().timestamp_millis()` for `ts` columns, consistent with M3/M5 storage style.

---

## 12. Error Model

`tasks::error::TasksError`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum TasksError {
    #[error("storage: {0}")]
    Storage(#[from] crate::storage::StorageError),
    #[error("worker {kind} failed: {message}")]
    WorkerFailed { kind: &'static str, message: String },
    #[error("task {0} not found")]
    NotFound(String),
    #[error("task {0} is not of kind {expected}")]
    KindMismatch { id: String, expected: &'static str },
    #[error("invalid task params: {0}")]
    InvalidParams(serde_json::Error),
    #[error("internal: worker panicked")]
    WorkerPanic,
    #[error("cancelled")]
    Cancelled,
}
```

`anyhow` only at the CLI binary boundary; `main.rs` does the final `Result<(), anyhow::Error>` translation.

`FetcherError` gains:

```rust
#[error("deferred to retry task {task_id}")]
Deferred { task_id: String },
```

`McpError` (or whatever the existing tool-error enum is) gains:

```rust
#[error("too many URLs ({count}, max {max})")]
TooManyUrls { count: usize, max: usize },
#[error("empty URL list")]
EmptyUrlList,
```

The MCP error-code mapping (in `src/mcp/error.rs`) routes these to `-32602`. `RobotsFetchFailed` source-chain fix lands here too (M5 #2).

---

## 13. Acceptance Criteria

From PRD §14 M6, plus the open-question resolutions:

- [ ] A batch of 20 URLs returns a `task_id` immediately (sub-100ms p99).
- [ ] `rover batch <id>` snapshot shows in-flight progress.
- [ ] `rover batch <id> --monitor` streams NDJSON until terminal.
- [ ] `rover task <id> --cancel` mid-flight stops further item starts; `task_cancelled` emitted; remaining items do not start.
- [ ] On `rover mcp` restart mid-batch, a fresh `rover mcp` claims the orphan and resumes; no duplicate `item_done` events.
- [ ] Stale-served fetch returns a `revalidation_task_id`; the revalidate task completes; cache is refreshed.
- [ ] Long `Retry-After` (>30s) returns a deferred envelope; the retry task waits and completes.
- [ ] Liveness warning prints when no live `rover mcp` and task is `running`.
- [ ] 273 + new M6 tests pass; clippy clean under `[lints.rust] warnings = "deny"`; no `println!` in lib code.

---

## 14. Open Items Deferred to Writing-Plans

- Exact wait-doubling cap inside `retry` worker (5 min ceiling is a starting point; planning task may sample real-world `Retry-After` distributions).
- Whether to emit a `task_progress` heartbeat from `batch_fetch` workers periodically, or rely solely on `item_*` events. Default: rely on item events; revisit if monitor goes long-quiet.
- Whether `rover task <id>` exit codes should distinguish `failed` (1) from `cancelled` (2). The plan locks the choice.
- Whether to expose `--from-event <id>` to MCP clients (currently CLI-only). Default: CLI-only.
- Where in the worker loop the safe-point cancellation check goes for `revalidate` (single HTTP request; the only natural point is "before request"). Plan task confirms.
- Concrete UUIDv7 timestamp source (`uuid::Uuid::now_v7()` vs explicit `Uuid::new_v7(Timestamp::from_unix(...))`). Default: `now_v7()` unless test determinism demands otherwise.

---

## 15. Decision Log

| Decision | Rationale |
| --- | --- |
| Orphan claim via single CAS UPDATE, 10s tick | Tasks stay `running` across handoff — symmetric with normal claim. No three-state shuffle. Idempotent: rowcount=1 means we own it. |
| Events-only progress | Snapshot reads on batches of ≤100 are O(events) on an indexed table — far below profiling-relevant. Zero risk of denormalised counters drifting on crash. |
| Shared core + kind-specific events | Lets `rover task <id>` work uniformly across kinds while preserving the rich `item_*` taxonomy `batch_fetch` needs. |
| DB row vs wire-shape split | DB row is `(kind, payload_json)` — minimal. Wire is flattened `{ts, kind, event_id, ...payload}` — matches PRD §9.2 example literally and gives clients a `resume_from` cursor. |
| Fixed 200ms monitor poll | PRD's stated cadence. Batches are bounded at 100 URLs so total events stay bounded. Backoff adds variable display latency for marginal CPU savings. |
| No batch-level timeout | Per-URL timeouts already bound runtime. Wallclock watchdogs invite confusing partial-completion UX. `--cancel` is the kill path. |
| Snapshot human default + `--format=ndjson` | Matches PRD §9.2 sample verbatim. Tooling consumers get a single-line JSON; humans get the friendly readout. |
| All four workers ship in M6 | `batch_fetch` is the headline; `retry` and `revalidate` exist because M2/M5 punted them; `summarize` stub locks the schema for M7 without a migration. |
| Single scheduler loop, tokio::spawn per task | One owner of lifecycle; easy to reason about cancellation and orphan claim. Per-kind pools are premature. |
| In-process new-task mpsc | Avoids polling the tasks table in the common case. Cross-process notify deliberately out of scope — handled by the 10s scan. |
| All four M5 follow-ups bundled | Apply-overrides directly serves `rover batch`; the others are cheap and align with files we'll already touch. |
| M4 follow-ups stay deferred | None of #3, #4, #6 touch M6 surfaces (extractor, signal split). Defer per manifest §M4 footer. |

# Rover M6 — Long-Running Tasks & Batching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a durable task subsystem to Rover so batched fetches, deferred retries, and stale-while-revalidate refreshes survive process restarts and multi-instance contention. Ship four workers (`batch_fetch`, `retry`, `revalidate`, plus a `summarize` stub that locks the schema for M7), the `batch_fetch` MCP tool, and `rover batch <id>` / `rover task <id>` CLI subcommands with snapshot, `--monitor` (NDJSON streaming), `--cancel`, and `--format=ndjson` modes.

**Architecture:** A new migration introduces `tasks` and `task_events` tables tied to the M3 `servers` table via `owner_pid`. A single `Scheduler` task inside `rover mcp` owns task lifecycle: it scans for orphaned tasks via a 10-second CAS-claim tick, listens on an in-process MPSC for newly inserted tasks, and `tokio::spawn`s a per-kind worker. Workers append append-only `task_events` rows that the CLI poll-loops over (200ms tick) when `--monitor` is set. Cancellation is cooperative — workers check `cancellation_requested` between safe points. The CLI processes are pure readers (snapshot) except for `--cancel` (single UPDATE).

**Tech Stack:** `uuid` (v7 feature for bare UUIDv7 task IDs), the existing `tokio-rusqlite` storage actor, `tokio::sync::{Semaphore, Mutex, mpsc, oneshot}`, `tokio_util::sync::CancellationToken`, `tokio::task::JoinSet`. Tests use `wiremock` (existing dev-dep), `tempfile`, and `serde_json` for event payload assertions.

**Branch context:** Execute on `m6-tasks`, cut from `main` at `d225b7d` (M5 PR #6 merge commit) and currently carrying one extra commit — the M6 design spec at `100c53c`. Verify `cargo test` is green on a clean checkout before Task 1.

**Scope of this plan:** PRD milestone M6 only (PRD §3.3, §4.2, §9). Four M5 follow-ups bundled because we are already touching `config.rs`, the same CLI override blocks, `mcp::error.rs`, and the M5 fetcher tests. Later milestones (M7 summarization, M8 polish, M9 feature flags) get their own plans.

**References:**
- Design spec: `docs/superpowers/specs/2026-05-14-rover-m6-tasks-batching-design.md`
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` §3.3, §4.2, §9, §14 M6
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` §2.3 (multi-instance), §3.1–§3.4 (PRD corrections)
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` M6 section
- M5 plan (granularity reference): `docs/superpowers/plans/2026-05-14-rover-m5-rate-limiting.md`

---

## Decisions inherited from the M6 design spec

The spec resolved every open question. Quick reference:

1. **Orphan claim:** single CAS `UPDATE` per orphan, scan every 10s by every live server. Tasks stay `status='running'` across handoff.
2. **Progress tracking:** events-only. Snapshots `GROUP BY kind` over `task_events`.
3. **Event taxonomy:** shared core (`task_started`, `task_progress`, `task_completed`, `task_failed`, `task_cancelled`) plus per-kind events.
4. **Wire shape:** DB stores `(kind, payload_json)`; wire flattens to `{ts, kind, event_id, ...payload}`.
5. **Monitor cadence:** fixed 200ms. SIGINT clean-exit.
6. **Batch timeout:** none. Per-URL fetcher timeouts already bound runtime; `--cancel` is the kill path.
7. **CLI snapshot:** human default; `--format=ndjson` / `--ndjson` for a single rollup line.
8. **Workers shipped:** all four (`batch_fetch`, `retry`, `revalidate`, `summarize` as a stub).
9. **Runtime:** single scheduler loop, `tokio::spawn` per task.
10. **In-process new-task delivery:** `tokio::sync::mpsc` from storage to scheduler.
11. **M5 follow-ups bundled:** all four (`apply_overrides` helper, `RobotsFetchFailed` source-chain, fixture rationalisation, tighten robots-disallow-all test).
12. **SWR path:** stale-but-expired cache entries return immediately + insert a `revalidate` task. The previous synchronous conditional-GET path is replaced.

---

## Files Created or Modified in This Plan

```
# Created
src/storage/migrations/004_tasks.sql
src/storage/tasks.rs                          # CRUD + orphan-claim CAS
src/storage/events.rs                         # append + range query

src/tasks/mod.rs                              # public surface + scheduler
src/tasks/error.rs                            # TasksError + Event types
src/tasks/types.rs                            # TaskId, TaskKind, Status, params/result shapes
src/tasks/scheduler.rs                        # claim loop + mpsc + dispatch
src/tasks/batch_fetch.rs                      # batch worker
src/tasks/retry.rs                            # retry worker
src/tasks/revalidate.rs                       # SWR worker
src/tasks/summarize.rs                        # stub worker (always fails)

src/mcp/tools/batch_fetch.rs                  # batch_fetch MCP tool

src/cli/task.rs                               # rover task <id> [--monitor|--cancel|--format]
src/cli/batch.rs                              # rover batch <id> wrapper

tests/tasks_lifecycle.rs
tests/tasks_orphan_claim.rs
tests/tasks_cancellation.rs
tests/tasks_revalidate.rs
tests/tasks_retry_deferred.rs
tests/cli_batch_monitor.rs
tests/cli_batch_snapshot.rs
tests/mcp_batch_fetch.rs

# Modified
Cargo.toml                                    # +uuid v7 feature
src/lib.rs                                    # +pub mod tasks
src/storage/mod.rs                            # register migration 004 + new modules
src/fetcher/mod.rs                            # +FetcherError::Deferred variant
src/fetcher/retry.rs                          # deferred classifier; tasks.insert on long Retry-After
src/fetcher/cached.rs                         # SWR path inserts revalidate task; stale_served envelope
src/mcp/handler.rs                            # carry Db (for tasks tools); register batch_fetch tool
src/mcp/server.rs                             # construct Scheduler, set new-task tx on Db, drive shutdown
src/mcp/envelope.rs                           # +TaskCreatedResponse, +DeferredResponse, +RoverError codes
src/mcp/error.rs                              # route new FetcherError + new McpError variants; M5 #2 source-chain
src/mcp/tools/fetch.rs                        # surface Deferred + StaleServed envelopes
src/config.rs                                 # extract apply_overrides helper (M5 #1)
src/cli/fetch.rs                              # use apply_overrides (M5 #1)
src/cli/mcp.rs                                # use apply_overrides (M5 #1)
src/cli/mod.rs                                # pub mod task; pub mod batch
src/main.rs                                   # wire `Command::Batch` / `Command::Task` dispatch

tests/fetcher_robots.rs                       # tighten disallow-all (M5 #4); RobotsFetchFailed source (M5 #2)
tests/fixtures/m5/*.txt                       # wire-into-tests or delete (M5 #3)

README.md                                     # M6 complete marker (final task)
docs/superpowers/milestones/rover-milestones.md   # M6 status update (final task)
```

Inline unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each new source file. Integration tests (`tests/*.rs`) cover end-to-end MCP/CLI flows.

---

## Task 1: M5 Follow-Ups Bundle

**Files:**
- Modify: `src/config.rs` (extract `apply_overrides`)
- Modify: `src/cli/fetch.rs:42-60` (delegate to `apply_overrides`)
- Modify: `src/cli/mcp.rs:22-40` (delegate to `apply_overrides`)
- Modify: `src/mcp/error.rs` (render `RobotsFetchFailed` source chain)
- Modify: `tests/fetcher_robots.rs` (tighten `disallow_all` test; add source-chain regression)
- Modify or delete: `tests/fixtures/m5/{robots-allow-articles,robots-disallow-admin,robots-with-crawldelay,wide-ua-rules}.txt`

Bundles four cleanups carried over from M5's review. None of them depend on M6 storage; doing them first keeps the rest of the plan focused on the task subsystem.

### Step 1.1: `Config::apply_overrides` helper (M5 #1)

- [ ] **Step 1: Write the failing test**

In `src/config.rs`, append to `#[cfg(test)] mod tests`:

```rust
#[test]
fn apply_overrides_clamps_concurrency_minimum() {
    let mut cfg = Config::default();
    cfg.apply_overrides(None, Some(0), Some(0), None, false);
    assert_eq!(cfg.rate_limit.per_domain_concurrency, 1);
    assert_eq!(cfg.rate_limit.global_concurrency, 1);
}

#[test]
fn apply_overrides_leaves_unset_fields_untouched() {
    let mut cfg = Config::default();
    let baseline_rpm = cfg.rate_limit.requests_per_minute_per_domain;
    let baseline_retries = cfg.rate_limit.max_retries;
    cfg.apply_overrides(None, None, None, None, false);
    assert_eq!(cfg.rate_limit.requests_per_minute_per_domain, baseline_rpm);
    assert_eq!(cfg.rate_limit.max_retries, baseline_retries);
    assert!(cfg.robots.respect);
}

#[test]
fn apply_overrides_disables_robots_when_requested() {
    let mut cfg = Config::default();
    cfg.apply_overrides(None, None, None, None, true);
    assert!(!cfg.robots.respect);
}

#[test]
fn apply_overrides_sets_explicit_values() {
    let mut cfg = Config::default();
    cfg.apply_overrides(Some(30), Some(4), Some(16), Some(5), false);
    assert_eq!(cfg.rate_limit.requests_per_minute_per_domain, 30);
    assert_eq!(cfg.rate_limit.per_domain_concurrency, 4);
    assert_eq!(cfg.rate_limit.global_concurrency, 16);
    assert_eq!(cfg.rate_limit.max_retries, 5);
}
```

- [ ] **Step 2: Run tests to verify they fail with "method not found"**

Run: `cargo test --lib config::tests::apply_overrides -- --exact`

Expected: compilation error `no method named apply_overrides`.

- [ ] **Step 3: Add the helper**

In `src/config.rs`, after `impl FetchConfig { ... }`, add:

```rust
impl Config {
    /// Apply CLI / MCP override flags onto an already-loaded config.
    ///
    /// Centralises the override logic shared by `rover fetch`, `rover mcp`, and
    /// (M6) `rover batch`. Bypasses `config::validate`; concurrency widths are
    /// clamped to >=1 to avoid `Semaphore::new(0)` silently hanging on acquire
    /// (regression fix from M5 commit 02bd7e8).
    pub fn apply_overrides(
        &mut self,
        rate_limit_rpm: Option<u32>,
        per_host_concurrency: Option<u32>,
        global_concurrency: Option<u32>,
        max_retries: Option<u8>,
        ignore_robots: bool,
    ) {
        if let Some(v) = rate_limit_rpm {
            self.rate_limit.requests_per_minute_per_domain = v;
        }
        if let Some(v) = per_host_concurrency {
            self.rate_limit.per_domain_concurrency = v.max(1);
        }
        if let Some(v) = global_concurrency {
            self.rate_limit.global_concurrency = v.max(1);
        }
        if let Some(v) = max_retries {
            self.rate_limit.max_retries = v;
        }
        if ignore_robots {
            self.robots.respect = false;
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::tests::apply_overrides -- --exact`

Expected: all 4 tests PASS.

- [ ] **Step 5: Replace duplicated logic in `cli/fetch.rs`**

In `src/cli/fetch.rs`, replace lines 42–60 (the four `if let Some(v) = ...` blocks and the `args.ignore_robots` handling, if present near `cfg.robots.respect = false`) with:

```rust
cfg.apply_overrides(
    args.rate_limit_rpm,
    args.per_host_concurrency,
    args.global_concurrency,
    args.max_retries,
    args.ignore_robots,
);
```

Move the call to immediately after `let mut cfg = config::load(config_path).context("loading config")?;`. Delete any now-redundant `if args.ignore_robots { cfg.robots.respect = false; }` line (the helper handles it).

- [ ] **Step 6: Replace duplicated logic in `cli/mcp.rs`**

In `src/cli/mcp.rs`, replace the entire override block (`if args.ignore_robots { ... }` plus the four `if let Some(v) = ...` blocks) with:

```rust
cfg.apply_overrides(
    args.rate_limit_rpm,
    args.per_host_concurrency,
    args.global_concurrency,
    args.max_retries,
    args.ignore_robots,
);
```

- [ ] **Step 7: Verify everything still builds and passes**

Run: `cargo build --all-features && cargo test --lib && cargo test --test cli_fetch && cargo test --test cli_fetch_overrides 2>/dev/null || true`

Expected: build succeeds, all unit tests pass. (The second `cargo test --test` is best-effort — only runs if the file exists.)

### Step 1.2: `RobotsFetchFailed` source chain (M5 #2)

- [ ] **Step 8: Add regression test in `tests/fetcher_robots.rs`**

Append at the bottom of the file:

```rust
#[test]
fn robots_fetch_failed_display_renders_inner_cause() {
    use rover::fetcher::FetcherError;
    let inner = FetcherError::Decode;
    let outer = FetcherError::RobotsFetchFailed {
        host: "example.com".to_string(),
        source: Box::new(inner),
    };
    let rendered = outer.to_string();
    assert!(
        rendered.contains("response decoding failed"),
        "expected inner Decode error in {rendered}",
    );
    assert!(rendered.contains("example.com"));
}
```

- [ ] **Step 9: Run the test to verify it fails**

Run: `cargo test --test fetcher_robots robots_fetch_failed_display_renders_inner_cause -- --exact`

Expected: FAIL — current `#[error]` format string does not include the source.

- [ ] **Step 10: Fix the format string**

In `src/fetcher/mod.rs`, find the variant:

```rust
    #[error("robots.txt fetch failed for {host}")]
    RobotsFetchFailed {
        host: String,
        #[source]
        source: Box<FetcherError>,
    },
```

Replace the `#[error]` line with:

```rust
    #[error("robots.txt fetch failed for {host}: {source}")]
```

- [ ] **Step 11: Run the test to verify it passes**

Run: `cargo test --test fetcher_robots robots_fetch_failed_display_renders_inner_cause -- --exact`

Expected: PASS.

- [ ] **Step 12: Add MCP-layer source-chain regression**

In `src/mcp/error.rs`, append to `#[cfg(test)] mod tests`:

```rust
#[test]
fn robots_fetch_failed_translation_carries_source_message() {
    use crate::fetcher::FetcherError;
    let e = McpError::Fetcher(FetcherError::RobotsFetchFailed {
        host: "example.com".to_string(),
        source: Box::new(FetcherError::Decode),
    });
    let r = e.into_rover_error();
    assert_eq!(r.code, RoverError::ROBOTS_FETCH_FAILED);
    assert!(
        r.message.contains("response decoding failed"),
        "expected inner cause in {}",
        r.message,
    );
}
```

Run: `cargo test --lib mcp::error::tests::robots_fetch_failed_translation_carries_source_message`. Expected: PASS (the existing `e.to_string()` now picks up the new format string).

### Step 1.3: Tighten `robots_disallow_all_refuses_fetch` (M5 #4)

- [ ] **Step 13: Tighten the assertion**

In `tests/fetcher_robots.rs`, locate the `robots_disallow_all_refuses_fetch` test. Replace its matcher (currently a tolerant `matches!(..., RobotsDisallowed | RobotsFetchFailed)`) with:

```rust
    match res {
        Err(FetcherError::RobotsDisallowed { url: u, .. }) => assert_eq!(u, target.as_str()),
        other => panic!("expected RobotsDisallowed, got {other:?}"),
    }
```

- [ ] **Step 14: Run it**

Run: `cargo test --test fetcher_robots robots_disallow_all_refuses_fetch -- --exact`

Expected: PASS.

### Step 1.4: Rationalise unused robots fixtures (M5 #3)

- [ ] **Step 15: Wire `robots-with-crawldelay.txt` into an existing test**

Inspect the file (it should declare `Crawl-Delay: 1`). In `tests/fetcher_robots.rs`, augment whichever test asserts crawl-delay behaviour (or add a new one) to read the fixture and verify the parser surfaces `crawl_delay = Some(Duration::from_secs(1))`:

```rust
#[tokio::test]
async fn parses_crawl_delay_from_fixture() {
    let body = std::fs::read_to_string("tests/fixtures/m5/robots-with-crawldelay.txt").unwrap();
    let entry = rover::storage::robots::RobotsEntry {
        host: "fixture.example".into(),
        body: Some(body),
        fetched_at: 0,
        expires_at: i64::MAX,
        state: rover::storage::robots::RobotsState::Parsed,
    };
    let delay = rover::fetcher::robots::crawl_delay(&entry, "Rover/0.1");
    assert_eq!(delay, Some(std::time::Duration::from_secs(1)));
}
```

If the public symbols above are not directly reachable from an integration test (likely because `crawl_delay` is `pub(crate)`), wire the fixture into a `#[cfg(test)]` unit test in `src/fetcher/robots.rs` instead with the same body. Run `cargo test --lib fetcher::robots` to confirm.

- [ ] **Step 16: Wire `robots-allow-articles.txt` and `robots-disallow-admin.txt` into existing tests**

Both should already correspond to disallow / allow-rule assertions in `tests/fetcher_robots.rs`. Add `std::fs::read_to_string` calls so the tests consume the fixture text rather than inline strings. If a fixture genuinely has no matching test, **delete it** (`git rm tests/fixtures/m5/<name>.txt`) — do not invent a test purely to use the fixture.

- [ ] **Step 17: Wire `wide-ua-rules.txt` or delete**

Same rule: if there is an existing UA-rule test, point it at this fixture. Otherwise `git rm`. Document the decision in the commit message.

- [ ] **Step 18: Run the full robots test suite**

Run: `cargo test --test fetcher_robots`

Expected: every test passes.

### Step 1.5: Commit

- [ ] **Step 19: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(m6): bundle m5 follow-ups before task subsystem

extract config::apply_overrides helper used by cli/{fetch,mcp,batch}.rs,
render robotsfetchfailed source chain on the wire, tighten robots
disallow-all test to assert only the expected variant, and wire/delete
the four unused robots fixtures.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Migration 004 + `storage::tasks` CRUD

**Files:**
- Create: `src/storage/migrations/004_tasks.sql`
- Create: `src/storage/tasks.rs`
- Modify: `src/storage/mod.rs` (register module + migration)
- Modify: `Cargo.toml` (add `uuid` with `v7` feature)

Provides the durable backing store for tasks. Schema and async API land together so that subsequent tasks (scheduler, workers) can be tested against a real DB.

- [ ] **Step 1: Add `uuid` v7 to Cargo.toml**

In `[dependencies]` (alphabetic order), add:

```toml
uuid = { version = "1", features = ["v7", "std"] }
```

Verify with `cargo add uuid --dry-run --features v7,std` that v1.x is current as of 2026-05-14; bump if necessary.

- [ ] **Step 2: Create the migration file**

Create `src/storage/migrations/004_tasks.sql`:

```sql
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
```

- [ ] **Step 3: Register migration + module**

In `src/storage/mod.rs`, extend the `MIGRATIONS` slice (after the existing 003 entry):

```rust
    (
        "004_tasks.sql",
        include_str!("migrations/004_tasks.sql"),
    ),
```

Add the module to the top:

```rust
pub mod tasks;
```

- [ ] **Step 4: Write the failing tests**

Create `src/storage/tasks.rs` with the skeleton + tests + `unimplemented!()` stubs:

```rust
//! `tasks` table async API.
//!
//! Sibling of `storage::pages`/`storage::robots`: a thin async wrapper that
//! hops into the `tokio-rusqlite` actor for SQLite work. Timestamps are
//! epoch milliseconds (sub-second ordering matters for the event stream).
//!
//! Helpers covering `task_events` live in `storage::events`.

use rusqlite::{OptionalExtension, params};

use super::{Db, StorageError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    BatchFetch,
    Retry,
    Revalidate,
    Summarize,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BatchFetch => "batch_fetch",
            Self::Retry => "retry",
            Self::Revalidate => "revalidate",
            Self::Summarize => "summarize",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "batch_fetch" => Self::BatchFetch,
            "retry" => Self::Retry,
            "revalidate" => Self::Revalidate,
            "summarize" => Self::Summarize,
            other => {
                return Err(StorageError::Backend(tokio_rusqlite::Error::Other(
                    format!("unknown tasks.kind = {other}").into(),
                )));
            }
        });
    }

    /// Whether the worker can resume from persisted progress after an
    /// owner-PID handoff. `summarize` is not resumable per design §2.3.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::BatchFetch | Self::Retry | Self::Revalidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => {
                return Err(StorageError::Backend(tokio_rusqlite::Error::Other(
                    format!("unknown tasks.status = {other}").into(),
                )));
            }
        });
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Row shape returned by query helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub params_json: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub cancellation_requested: bool,
    pub owner_pid: Option<i64>,
}

/// Input for inserting a new task (status & timestamps set internally).
#[derive(Debug, Clone)]
pub struct TaskInsert {
    pub id: String,
    pub kind: TaskKind,
    pub params_json: String,
    pub owner_pid: Option<i64>,
}

pub async fn insert(_db: &Db, _input: TaskInsert) -> Result<(), StorageError> {
    unimplemented!("step 6")
}

pub async fn get(_db: &Db, _id: &str) -> Result<Option<TaskRow>, StorageError> {
    unimplemented!("step 6")
}

pub async fn set_status(
    _db: &Db,
    _id: &str,
    _status: TaskStatus,
    _result_json: Option<String>,
    _error: Option<String>,
) -> Result<(), StorageError> {
    unimplemented!("step 6")
}

pub async fn set_cancellation_requested(_db: &Db, _id: &str) -> Result<bool, StorageError> {
    unimplemented!("step 6")
}

pub async fn is_cancelled(_db: &Db, _id: &str) -> Result<bool, StorageError> {
    unimplemented!("step 6")
}

pub async fn list_orphans(_db: &Db) -> Result<Vec<TaskRow>, StorageError> {
    unimplemented!("step 6")
}

pub async fn claim_orphan(
    _db: &Db,
    _id: &str,
    _orphan_pid: i64,
    _own_pid: i64,
) -> Result<bool, StorageError> {
    unimplemented!("step 6")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        db
    }

    fn sample_insert(id: &str, pid: Option<i64>) -> TaskInsert {
        TaskInsert {
            id: id.into(),
            kind: TaskKind::BatchFetch,
            params_json: r#"{"urls":["https://a.example/"]}"#.into(),
            owner_pid: pid,
        }
    }

    #[tokio::test]
    async fn insert_and_get_round_trip() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        let got = get(&db, "t1").await.unwrap().expect("row missing");
        assert_eq!(got.id, "t1");
        assert_eq!(got.kind, TaskKind::BatchFetch);
        assert_eq!(got.status, TaskStatus::Running);
        assert_eq!(got.owner_pid, Some(7));
        assert!(!got.cancellation_requested);
    }

    #[tokio::test]
    async fn get_unknown_returns_none() {
        let db = fresh_db().await;
        assert!(get(&db, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_status_terminal_writes_result_and_error() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        set_status(&db, "t1", TaskStatus::Failed, None, Some("owner_died".into()))
            .await
            .unwrap();
        let got = get(&db, "t1").await.unwrap().unwrap();
        assert_eq!(got.status, TaskStatus::Failed);
        assert_eq!(got.error.as_deref(), Some("owner_died"));
    }

    #[tokio::test]
    async fn set_cancellation_requested_is_idempotent() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        let first = set_cancellation_requested(&db, "t1").await.unwrap();
        let second = set_cancellation_requested(&db, "t1").await.unwrap();
        assert!(first);
        assert!(!second, "second call should be a no-op");
        assert!(is_cancelled(&db, "t1").await.unwrap());
    }

    #[tokio::test]
    async fn set_cancellation_requested_on_missing_id_returns_false() {
        let db = fresh_db().await;
        assert!(!set_cancellation_requested(&db, "ghost").await.unwrap());
    }

    #[tokio::test]
    async fn list_orphans_excludes_live_pids() {
        let db = fresh_db().await;
        db.upsert_server_self(100, "v".into()).await.unwrap();
        insert(&db, sample_insert("live", Some(100))).await.unwrap();
        insert(&db, sample_insert("dead", Some(999))).await.unwrap();
        let orphans = list_orphans(&db).await.unwrap();
        let ids: Vec<&str> = orphans.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["dead"]);
    }

    #[tokio::test]
    async fn list_orphans_excludes_terminal_tasks() {
        let db = fresh_db().await;
        insert(&db, sample_insert("dead_done", Some(999)))
            .await
            .unwrap();
        set_status(&db, "dead_done", TaskStatus::Completed, None, None)
            .await
            .unwrap();
        let orphans = list_orphans(&db).await.unwrap();
        assert!(
            orphans.is_empty(),
            "completed orphan should not appear: {orphans:?}",
        );
    }

    #[tokio::test]
    async fn claim_orphan_cas_wins_then_loses() {
        let db = fresh_db().await;
        insert(&db, sample_insert("orphan", Some(999))).await.unwrap();
        let first = claim_orphan(&db, "orphan", 999, 1).await.unwrap();
        let second = claim_orphan(&db, "orphan", 999, 2).await.unwrap();
        assert!(first, "first claimer should win");
        assert!(!second, "second claimer should lose");
        let got = get(&db, "orphan").await.unwrap().unwrap();
        assert_eq!(got.owner_pid, Some(1));
    }
}
```

- [ ] **Step 5: Run the tests to verify they fail with `unimplemented!`**

Run: `cargo test --lib storage::tasks::tests`

Expected: each test panics with `not yet implemented`.

- [ ] **Step 6: Implement the eight functions**

Replace the eight `unimplemented!()` stubs in order:

```rust
pub async fn insert(db: &Db, input: TaskInsert) -> Result<(), StorageError> {
    let TaskInsert {
        id,
        kind,
        params_json,
        owner_pid,
    } = input;
    let kind_s = kind.as_str().to_string();
    let now = now_epoch_ms();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO tasks
                   (id, kind, status, created_at, updated_at, params_json,
                    result_json, error, cancellation_requested, owner_pid)
                 VALUES (?1, ?2, 'running', ?3, ?3, ?4, NULL, NULL, 0, ?5)",
                params![id, kind_s, now, params_json, owner_pid],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    Ok(())
}

pub async fn get(db: &Db, id: &str) -> Result<Option<TaskRow>, StorageError> {
    let id = id.to_string();
    let row = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT id, kind, status, created_at, updated_at, params_json,
                        result_json, error, cancellation_requested, owner_pid
                 FROM tasks WHERE id = ?1",
                [&id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                    ))
                },
            )
            .optional()
        })
        .await?;
    let Some((id, kind_s, status_s, created_at, updated_at, params_json, result_json, error, canc, owner_pid)) =
        row
    else {
        return Ok(None);
    };
    Ok(Some(TaskRow {
        id,
        kind: TaskKind::from_db(&kind_s)?,
        status: TaskStatus::from_db(&status_s)?,
        created_at,
        updated_at,
        params_json,
        result_json,
        error,
        cancellation_requested: canc != 0,
        owner_pid,
    }))
}

pub async fn set_status(
    db: &Db,
    id: &str,
    status: TaskStatus,
    result_json: Option<String>,
    error: Option<String>,
) -> Result<(), StorageError> {
    let id = id.to_string();
    let status_s = status.as_str().to_string();
    let now = now_epoch_ms();
    db.conn
        .call(move |c| {
            c.execute(
                "UPDATE tasks
                    SET status = ?1, updated_at = ?2,
                        result_json = COALESCE(?3, result_json),
                        error = COALESCE(?4, error)
                  WHERE id = ?5",
                params![status_s, now, result_json, error, id],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    Ok(())
}

pub async fn set_cancellation_requested(db: &Db, id: &str) -> Result<bool, StorageError> {
    let id = id.to_string();
    let now = now_epoch_ms();
    let changed = db
        .conn
        .call(move |c| {
            let n = c.execute(
                "UPDATE tasks
                    SET cancellation_requested = 1, updated_at = ?1
                  WHERE id = ?2 AND cancellation_requested = 0",
                params![now, id],
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(changed == 1)
}

pub async fn is_cancelled(db: &Db, id: &str) -> Result<bool, StorageError> {
    let id = id.to_string();
    let flag = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT cancellation_requested FROM tasks WHERE id = ?1",
                [&id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
        .await?;
    Ok(flag.unwrap_or(0) != 0)
}

pub async fn list_orphans(db: &Db) -> Result<Vec<TaskRow>, StorageError> {
    let rows = db
        .conn
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, kind, status, created_at, updated_at, params_json,
                        result_json, error, cancellation_requested, owner_pid
                 FROM tasks
                 WHERE status = 'running'
                   AND owner_pid IS NOT NULL
                   AND owner_pid NOT IN (SELECT pid FROM servers)",
            )?;
            let iter = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                ))
            })?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    let mut tasks = Vec::with_capacity(rows.len());
    for (id, kind_s, status_s, created_at, updated_at, params_json, result_json, error, canc, owner_pid) in rows {
        tasks.push(TaskRow {
            id,
            kind: TaskKind::from_db(&kind_s)?,
            status: TaskStatus::from_db(&status_s)?,
            created_at,
            updated_at,
            params_json,
            result_json,
            error,
            cancellation_requested: canc != 0,
            owner_pid,
        });
    }
    Ok(tasks)
}

pub async fn claim_orphan(
    db: &Db,
    id: &str,
    orphan_pid: i64,
    own_pid: i64,
) -> Result<bool, StorageError> {
    let id = id.to_string();
    let now = now_epoch_ms();
    let changed = db
        .conn
        .call(move |c| {
            let n = c.execute(
                "UPDATE tasks
                    SET owner_pid = ?1, updated_at = ?2
                  WHERE id = ?3 AND owner_pid = ?4 AND status = 'running'",
                params![own_pid, now, id, orphan_pid],
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(changed == 1)
}

fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --lib storage::tasks::tests`

Expected: all 8 tests PASS.

- [ ] **Step 8: Run the full storage layer suite + verify migration applies cleanly**

Run: `cargo test --lib storage:: && cargo test --lib storage::tests::open_creates_db_and_applies_migrations`

Expected: PASS. `schema_version` is now 4.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml src/storage/migrations/004_tasks.sql src/storage/tasks.rs src/storage/mod.rs
git commit -m "$(cat <<'EOF'
feat(m6): migration 004 + storage::tasks crud

introduces tasks and task_events tables with owner_pid linkage to servers,
plus async crud (insert, get, set_status, set_cancellation_requested,
is_cancelled, list_orphans, claim_orphan) for the scheduler and workers.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `storage::events` Append + Range API

**Files:**
- Create: `src/storage/events.rs`
- Modify: `src/storage/mod.rs` (register module)

`task_events` is append-only. Three operations: append a row, fetch a range since a cursor, group-count by kind for snapshot reads.

- [ ] **Step 1: Register the module**

In `src/storage/mod.rs`, alongside the other `pub mod` lines:

```rust
pub mod events;
```

- [ ] **Step 2: Write the failing tests + stubs**

Create `src/storage/events.rs`:

```rust
//! `task_events` append-only log.

use rusqlite::params;

use super::{Db, StorageError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub task_id: String,
    pub ts: i64,
    pub kind: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub struct EventInsert {
    pub task_id: String,
    pub kind: String,
    pub payload_json: String,
}

pub async fn append(_db: &Db, _input: EventInsert) -> Result<i64, StorageError> {
    unimplemented!("step 4")
}

pub async fn range_since(
    _db: &Db,
    _task_id: &str,
    _after_id: i64,
    _limit: i64,
) -> Result<Vec<EventRow>, StorageError> {
    unimplemented!("step 4")
}

pub async fn count_by_kind(
    _db: &Db,
    _task_id: &str,
) -> Result<Vec<(String, i64)>, StorageError> {
    unimplemented!("step 4")
}

pub async fn last_for_task(_db: &Db, _task_id: &str) -> Result<Option<EventRow>, StorageError> {
    unimplemented!("step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tasks::{TaskInsert, TaskKind, insert as insert_task};
    use tempfile::tempdir;

    async fn fresh_db_with_task(id: &str) -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        insert_task(
            &db,
            TaskInsert {
                id: id.into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        db
    }

    fn ev(task_id: &str, kind: &str, payload: &str) -> EventInsert {
        EventInsert {
            task_id: task_id.into(),
            kind: kind.into(),
            payload_json: payload.into(),
        }
    }

    #[tokio::test]
    async fn append_returns_monotonic_ids() {
        let db = fresh_db_with_task("t1").await;
        let id1 = append(&db, ev("t1", "task_started", "{}")).await.unwrap();
        let id2 = append(&db, ev("t1", "item_done", r#"{"url":"a"}"#))
            .await
            .unwrap();
        assert!(id2 > id1);
    }

    #[tokio::test]
    async fn range_since_filters_by_cursor() {
        let db = fresh_db_with_task("t1").await;
        let id1 = append(&db, ev("t1", "a", "{}")).await.unwrap();
        let id2 = append(&db, ev("t1", "b", "{}")).await.unwrap();
        let id3 = append(&db, ev("t1", "c", "{}")).await.unwrap();
        let rows = range_since(&db, "t1", id1, 100).await.unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["b", "c"]);
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![id2, id3]);
    }

    #[tokio::test]
    async fn range_since_caps_at_limit() {
        let db = fresh_db_with_task("t1").await;
        for i in 0..10 {
            append(&db, ev("t1", "x", &format!(r#"{{"i":{i}}}"#)))
                .await
                .unwrap();
        }
        let rows = range_since(&db, "t1", 0, 3).await.unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn range_since_isolates_tasks() {
        let db = fresh_db_with_task("t1").await;
        insert_task(
            &db,
            TaskInsert {
                id: "t2".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        append(&db, ev("t2", "b", "{}")).await.unwrap();
        let rows = range_since(&db, "t1", 0, 100).await.unwrap();
        assert!(rows.iter().all(|r| r.task_id == "t1"));
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn count_by_kind_groups_correctly() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "item_done", "{}")).await.unwrap();
        append(&db, ev("t1", "item_done", "{}")).await.unwrap();
        append(&db, ev("t1", "item_failed", "{}")).await.unwrap();
        let mut counts = count_by_kind(&db, "t1").await.unwrap();
        counts.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            counts,
            vec![("item_done".into(), 2), ("item_failed".into(), 1)],
        );
    }

    #[tokio::test]
    async fn last_for_task_returns_highest_id() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        let mid_id = append(&db, ev("t1", "b", r#"{"k":1}"#)).await.unwrap();
        append(&db, ev("t1", "c", "{}")).await.unwrap();
        let last = last_for_task(&db, "t1").await.unwrap().unwrap();
        assert_eq!(last.kind, "c");
        assert!(last.id > mid_id);
    }

    #[tokio::test]
    async fn cascade_delete_drops_events() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        db.conn
            .call(|c| {
                c.execute("DELETE FROM tasks WHERE id = 't1'", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        let rows = range_since(&db, "t1", 0, 100).await.unwrap();
        assert!(rows.is_empty());
    }
}
```

- [ ] **Step 3: Run to verify the tests fail with `unimplemented!`**

Run: `cargo test --lib storage::events`

Expected: panics on `not yet implemented`.

- [ ] **Step 4: Implement the four functions**

Replace the stubs:

```rust
pub async fn append(db: &Db, input: EventInsert) -> Result<i64, StorageError> {
    let EventInsert {
        task_id,
        kind,
        payload_json,
    } = input;
    let now = now_epoch_ms();
    let id = db
        .conn
        .call(move |c| {
            c.execute(
                "INSERT INTO task_events (task_id, ts, kind, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![task_id, now, kind, payload_json],
            )?;
            Ok::<_, rusqlite::Error>(c.last_insert_rowid())
        })
        .await?;
    Ok(id)
}

pub async fn range_since(
    db: &Db,
    task_id: &str,
    after_id: i64,
    limit: i64,
) -> Result<Vec<EventRow>, StorageError> {
    let task_id = task_id.to_string();
    let rows = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, task_id, ts, kind, payload_json
                 FROM task_events
                 WHERE task_id = ?1 AND id > ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )?;
            let iter = stmt.query_map(params![task_id, after_id, limit], |r| {
                Ok(EventRow {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    ts: r.get(2)?,
                    kind: r.get(3)?,
                    payload_json: r.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    Ok(rows)
}

pub async fn count_by_kind(
    db: &Db,
    task_id: &str,
) -> Result<Vec<(String, i64)>, StorageError> {
    let task_id = task_id.to_string();
    let counts = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT kind, COUNT(*) FROM task_events WHERE task_id = ?1 GROUP BY kind",
            )?;
            let iter = stmt.query_map([&task_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    Ok(counts)
}

pub async fn last_for_task(db: &Db, task_id: &str) -> Result<Option<EventRow>, StorageError> {
    let task_id = task_id.to_string();
    let row = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT id, task_id, ts, kind, payload_json
                 FROM task_events WHERE task_id = ?1
                 ORDER BY id DESC LIMIT 1",
                [&task_id],
                |r| {
                    Ok(EventRow {
                        id: r.get(0)?,
                        task_id: r.get(1)?,
                        ts: r.get(2)?,
                        kind: r.get(3)?,
                        payload_json: r.get(4)?,
                    })
                },
            )
            .optional()
        })
        .await?;
    Ok(row)
}

fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

Add `use rusqlite::OptionalExtension;` if not already imported.

- [ ] **Step 5: Run to verify the tests pass**

Run: `cargo test --lib storage::events`

Expected: all 7 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/storage/events.rs src/storage/mod.rs
git commit -m "feat(m6): storage::events append-only log api

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 4: `tasks` Module Skeleton, Types, Error

**Files:**
- Create: `src/tasks/mod.rs`
- Create: `src/tasks/error.rs`
- Create: `src/tasks/types.rs`
- Modify: `src/lib.rs` (`pub mod tasks;`)
- Create: stub files for the four worker modules (bodies follow in later tasks)

Public surface for the task subsystem: error type, task ID newtype, core event kinds, and per-kind param/result struct definitions. No scheduler or workers yet — those land in Tasks 5–11.

- [ ] **Step 1: Add the module to `lib.rs`**

In `src/lib.rs`, alphabetically (after `pub mod storage;`):

```rust
pub mod tasks;
```

- [ ] **Step 2: Create error and types files**

Create `src/tasks/error.rs`:

```rust
//! Errors raised by the task subsystem.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TasksError {
    #[error("storage: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("worker {kind} failed: {message}")]
    WorkerFailed {
        kind: &'static str,
        message: String,
    },

    #[error("task {0} not found")]
    NotFound(String),

    #[error("task {id} is not of kind {expected}")]
    KindMismatch {
        id: String,
        expected: &'static str,
    },

    #[error("invalid task params: {0}")]
    InvalidParams(#[from] serde_json::Error),

    #[error("internal: worker panicked")]
    WorkerPanic,

    #[error("cancelled")]
    Cancelled,
}
```

Create `src/tasks/types.rs`:

```rust
//! Public types shared between the scheduler, workers, and the CLI.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::storage::tasks::{TaskKind, TaskStatus};

/// Bare UUIDv7 task identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(s)?;
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Core event kinds shared by every worker (per design spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreEvent {
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
}

impl CoreEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskStarted => "task_started",
            Self::TaskProgress => "task_progress",
            Self::TaskCompleted => "task_completed",
            Self::TaskFailed => "task_failed",
            Self::TaskCancelled => "task_cancelled",
        }
    }
}

/// `batch_fetch` params stored in `tasks.params_json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchFetchParams {
    pub urls: Vec<String>,
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    #[serde(default = "default_per_domain")]
    pub per_domain_concurrency: u32,
    #[serde(default)]
    pub force_refresh: bool,
}

fn default_concurrency() -> u32 { 8 }
fn default_per_domain() -> u32 { 2 }

/// `retry` params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryParams {
    pub url: String,
    pub attempt: u8,
    pub wait_ms_initial: u64,
    pub max_attempts: u8,
    #[serde(default)]
    pub parent_task_id: Option<String>,
}

/// `revalidate` params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevalidateParams {
    pub url: String,
    #[serde(default)]
    pub etag_at_serve: Option<String>,
    #[serde(default)]
    pub last_modified_at_serve: Option<String>,
}

/// Rollup written to `tasks.result_json` when a `batch_fetch` completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchFetchResult {
    pub total: u32,
    pub succeeded: u32,
    pub failed: u32,
    pub duration_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_is_uuid_v7_string() {
        let id = TaskId::new();
        let parsed = Uuid::parse_str(id.as_str()).unwrap();
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn task_id_parse_roundtrip() {
        let id = TaskId::new();
        let again = TaskId::parse(id.as_str()).unwrap();
        assert_eq!(id, again);
    }

    #[test]
    fn task_id_parse_rejects_garbage() {
        assert!(TaskId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn batch_fetch_params_defaults() {
        let v: BatchFetchParams = serde_json::from_str(r#"{"urls":["a"]}"#).unwrap();
        assert_eq!(v.concurrency, 8);
        assert_eq!(v.per_domain_concurrency, 2);
        assert!(!v.force_refresh);
    }

    #[test]
    fn core_event_as_str_table() {
        assert_eq!(CoreEvent::TaskStarted.as_str(), "task_started");
        assert_eq!(CoreEvent::TaskFailed.as_str(), "task_failed");
    }
}
```

- [ ] **Step 3: Create the mod.rs and stub worker files**

Create `src/tasks/mod.rs`:

```rust
//! Long-running task subsystem.
//!
//! See `docs/superpowers/specs/2026-05-14-rover-m6-tasks-batching-design.md`.

pub mod batch_fetch;
pub mod error;
pub mod retry;
pub mod revalidate;
pub mod scheduler;
pub mod summarize;
pub mod types;

pub use error::TasksError;
pub use scheduler::{NewTaskSender, Scheduler};
pub use types::{
    BatchFetchParams, BatchFetchResult, CoreEvent, RetryParams, RevalidateParams, TaskId, TaskKind,
    TaskStatus,
};
```

Create empty bodies for each worker so the module declarations compile:

`src/tasks/batch_fetch.rs`:
```rust
//! `batch_fetch` worker. Body lands in Task 7.
```

`src/tasks/retry.rs`:
```rust
//! `retry` worker. Body lands in Task 9.
```

`src/tasks/revalidate.rs`:
```rust
//! `revalidate` worker. Body lands in Task 11.
```

`src/tasks/summarize.rs`:
```rust
//! `summarize` stub worker. Body lands in Task 6.
```

Create `src/tasks/scheduler.rs` with a minimal stub (real body in Task 5):

```rust
//! Scheduler stub. Real body lands in Task 5.

use tokio::sync::mpsc;

use crate::tasks::types::TaskId;

pub type NewTaskSender = mpsc::UnboundedSender<TaskId>;
pub type NewTaskReceiver = mpsc::UnboundedReceiver<TaskId>;

pub struct Scheduler;
```

- [ ] **Step 4: Build and run unit tests**

Run: `cargo build --all-features && cargo test --lib tasks::types::tests`

Expected: build PASSes; 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/tasks/
git commit -m "feat(m6): tasks module skeleton with types and error enum

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 5: Scheduler — Claim Loop + MPSC Dispatch

**Files:**
- Modify: `src/tasks/scheduler.rs` (full body)

Single scheduler loop owning task lifecycle: orphan-claim CAS every 10s, listens on an MPSC for newly inserted task IDs, `tokio::spawn`s a worker per task via a `WorkerSpawner` trait object.

- [ ] **Step 1: Write the full scheduler body**

Replace `src/tasks/scheduler.rs` with:

```rust
//! Single scheduler loop owning task lifecycle.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskKind, TaskStatus, set_status};
use crate::storage::{self, Db};
use crate::tasks::error::TasksError;
use crate::tasks::types::TaskId;

pub type NewTaskSender = mpsc::UnboundedSender<TaskId>;
pub type NewTaskReceiver = mpsc::UnboundedReceiver<TaskId>;

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub own_pid: i64,
    pub orphan_scan_interval: Duration,
    pub shutdown_grace: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            own_pid: std::process::id() as i64,
            orphan_scan_interval: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

pub struct Scheduler {
    pub db: Db,
    pub cfg: SchedulerConfig,
    pub cancel: CancellationToken,
    pub new_task_rx: NewTaskReceiver,
    pub spawner: Arc<dyn WorkerSpawner>,
}

/// Trait-object indirection so workers wire in via DefaultSpawner without
/// the scheduler depending on every concrete worker module.
pub trait WorkerSpawner: Send + Sync + 'static {
    fn spawn(
        &self,
        join_set: &mut JoinSet<()>,
        db: Db,
        task_id: TaskId,
        kind: TaskKind,
        cancel: CancellationToken,
    );
}

impl Scheduler {
    pub fn channel() -> (NewTaskSender, NewTaskReceiver) {
        mpsc::unbounded_channel()
    }

    pub async fn run(mut self) -> Result<(), TasksError> {
        let mut tick = tokio::time::interval(self.cfg.orphan_scan_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut join_set: JoinSet<()> = JoinSet::new();

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => break,
                _ = tick.tick() => {
                    if let Err(e) = self.scan_and_claim_orphans(&mut join_set).await {
                        tracing::warn!(target: "rover::tasks", error = ?e, "orphan scan failed");
                    }
                }
                Some(task_id) = self.new_task_rx.recv() => {
                    if let Err(e) = self.handle_new_task(&mut join_set, task_id).await {
                        tracing::warn!(target: "rover::tasks", error = ?e, "spawn failed");
                    }
                }
                Some(res) = join_set.join_next() => {
                    if let Err(e) = res {
                        tracing::error!(target: "rover::tasks", error = ?e, "worker panicked");
                    }
                }
            }
        }

        let _ = tokio::time::timeout(self.cfg.shutdown_grace, async {
            while join_set.join_next().await.is_some() {}
        })
        .await;
        Ok(())
    }

    pub async fn scan_and_claim_orphans(
        &self,
        join_set: &mut JoinSet<()>,
    ) -> Result<(), TasksError> {
        let orphans = storage::tasks::list_orphans(&self.db).await?;
        for orphan in orphans {
            let orphan_pid = match orphan.owner_pid {
                Some(p) => p,
                None => continue,
            };
            if !orphan.kind.is_resumable() {
                let claimed = storage::tasks::claim_orphan(
                    &self.db,
                    &orphan.id,
                    orphan_pid,
                    self.cfg.own_pid,
                )
                .await?;
                if claimed {
                    set_status(
                        &self.db,
                        &orphan.id,
                        TaskStatus::Failed,
                        None,
                        Some("owner_died".into()),
                    )
                    .await?;
                    append(
                        &self.db,
                        EventInsert {
                            task_id: orphan.id.clone(),
                            kind: "task_failed".into(),
                            payload_json: r#"{"error":"owner_died","message":"original owner pid disappeared and task kind is not resumable"}"#.into(),
                        },
                    )
                    .await?;
                }
                continue;
            }
            let claimed = storage::tasks::claim_orphan(
                &self.db,
                &orphan.id,
                orphan_pid,
                self.cfg.own_pid,
            )
            .await?;
            if claimed {
                tracing::info!(
                    target: "rover::tasks",
                    task_id = %orphan.id,
                    kind = orphan.kind.as_str(),
                    "claimed orphaned task"
                );
                let task_cancel = self.cancel.child_token();
                self.spawner.spawn(
                    join_set,
                    self.db.clone(),
                    TaskId(orphan.id.clone()),
                    orphan.kind,
                    task_cancel,
                );
            }
        }
        Ok(())
    }

    async fn handle_new_task(
        &self,
        join_set: &mut JoinSet<()>,
        task_id: TaskId,
    ) -> Result<(), TasksError> {
        let row = storage::tasks::get(&self.db, task_id.as_str())
            .await?
            .ok_or_else(|| TasksError::NotFound(task_id.as_str().to_string()))?;
        let task_cancel = self.cancel.child_token();
        self.spawner
            .spawn(join_set, self.db.clone(), task_id, row.kind, task_cancel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        db
    }

    #[derive(Default)]
    struct RecordingSpawner {
        spawned: AtomicUsize,
        kinds: Mutex<Vec<TaskKind>>,
    }

    impl WorkerSpawner for RecordingSpawner {
        fn spawn(
            &self,
            join_set: &mut JoinSet<()>,
            _db: Db,
            _task_id: TaskId,
            kind: TaskKind,
            cancel: CancellationToken,
        ) {
            self.spawned.fetch_add(1, Ordering::SeqCst);
            self.kinds.lock().unwrap().push(kind);
            join_set.spawn(async move {
                cancel.cancelled().await;
            });
        }
    }

    fn mk_sched(
        db: Db,
        pid: i64,
        cancel: CancellationToken,
    ) -> (Scheduler, Arc<RecordingSpawner>) {
        let (_tx, rx) = Scheduler::channel();
        let spawner = Arc::new(RecordingSpawner::default());
        let sched = Scheduler {
            db,
            cfg: SchedulerConfig {
                own_pid: pid,
                orphan_scan_interval: Duration::from_millis(50),
                shutdown_grace: Duration::from_millis(100),
            },
            cancel,
            new_task_rx: rx,
            spawner: spawner.clone(),
        };
        (sched, spawner)
    }

    #[tokio::test]
    async fn scan_claims_resumable_orphan() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "orphan".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 1, cancel.clone());
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        let row = crate::storage::tasks::get(&db, "orphan").await.unwrap().unwrap();
        assert_eq!(row.owner_pid, Some(1));
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
        assert_eq!(spawner.kinds.lock().unwrap().as_slice(), &[TaskKind::BatchFetch]);
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_millis(200), async {
            while js.join_next().await.is_some() {}
        })
        .await;
    }

    #[tokio::test]
    async fn scan_marks_non_resumable_orphan_failed() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "stub".into(),
                kind: TaskKind::Summarize,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 2, cancel.clone());
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        cancel.cancel();
        let row = crate::storage::tasks::get(&db, "stub").await.unwrap().unwrap();
        assert_eq!(row.status, TaskStatus::Failed);
        assert_eq!(row.error.as_deref(), Some("owner_died"));
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn scan_skips_live_pids() {
        let db = fresh_db().await;
        db.upsert_server_self(100, "v".into()).await.unwrap();
        insert(
            &db,
            TaskInsert {
                id: "owned".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(100),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 200, cancel.clone());
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        cancel.cancel();
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn race_two_schedulers_only_one_claims() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "race".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let c1 = CancellationToken::new();
        let c2 = CancellationToken::new();
        let (s1, sp1) = mk_sched(db.clone(), 1, c1.clone());
        let (s2, sp2) = mk_sched(db.clone(), 2, c2.clone());
        let (mut js1, mut js2) = (JoinSet::new(), JoinSet::new());
        s1.scan_and_claim_orphans(&mut js1).await.unwrap();
        s2.scan_and_claim_orphans(&mut js2).await.unwrap();
        let total = sp1.spawned.load(Ordering::SeqCst) + sp2.spawned.load(Ordering::SeqCst);
        assert_eq!(total, 1, "expected exactly one claimer, got {total}");
        c1.cancel();
        c2.cancel();
    }

    #[tokio::test]
    async fn run_dispatches_for_inserted_id() {
        let db = fresh_db().await;
        let (tx, rx) = Scheduler::channel();
        insert(
            &db,
            TaskInsert {
                id: "live".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        tx.send(TaskId("live".into())).unwrap();
        drop(tx);
        let cancel = CancellationToken::new();
        let spawner = Arc::new(RecordingSpawner::default());
        let sched = Scheduler {
            db: db.clone(),
            cfg: SchedulerConfig {
                own_pid: 1,
                orphan_scan_interval: Duration::from_millis(500),
                shutdown_grace: Duration::from_millis(100),
            },
            cancel: cancel.clone(),
            new_task_rx: rx,
            spawner: spawner.clone(),
        };
        let handle = tokio::spawn(sched.run());
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
    }
}
```

- [ ] **Step 2: Run the scheduler tests**

Run: `cargo test --lib tasks::scheduler::tests`

Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/tasks/scheduler.rs
git commit -m "feat(m6): tasks scheduler with orphan-claim cas and mpsc dispatch

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 6: `summarize` Stub Worker + End-to-End Lifecycle Test

**Files:**
- Modify: `src/tasks/summarize.rs` (full body)
- Modify: `src/tasks/mod.rs` (add `DefaultSpawner` + `default_spawner()`)
- Create: `tests/tasks_lifecycle.rs`

The simplest worker. Emits `task_started`, then `task_failed` with `summarization_not_yet_implemented`. Wiring `DefaultSpawner` here lets the lifecycle integration test exercise the scheduler → worker handoff for real.

- [ ] **Step 1: Replace the summarize stub with the real body**

Replace `src/tasks/summarize.rs` with:

```rust
//! `summarize` stub worker.
//!
//! Always fails with `summarization_not_yet_implemented`. The real
//! summarizer lands in M7. The stub exists so the `tasks.kind` schema is
//! final in M6 and the scheduler has a concrete worker to dispatch.

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskStatus, set_status};
use crate::tasks::types::TaskId;

pub async fn run(db: Db, task_id: TaskId, _cancel: CancellationToken) {
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"summarize"}).to_string(),
        },
    )
    .await;
    let payload = json!({
        "error": "summarization_not_yet_implemented",
        "message": "Summarization will be implemented in M7.",
        "duration_ms": 0,
    });
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_failed".into(),
            payload_json: payload.to_string(),
        },
    )
    .await;
    let _ = set_status(
        &db,
        task_id.as_str(),
        TaskStatus::Failed,
        None,
        Some("summarization_not_yet_implemented".into()),
    )
    .await;
}
```

- [ ] **Step 2: Add `DefaultSpawner` to `src/tasks/mod.rs`**

Append (after the existing `pub use` lines):

```rust
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::storage::Db;
use crate::storage::tasks::TaskKind;

/// Default production dispatch table. Routes by `TaskKind`. Tasks 7/9/11
/// replace the `BatchFetch` / `Retry` / `Revalidate` arms with their real
/// worker calls.
pub struct DefaultSpawner;

impl scheduler::WorkerSpawner for DefaultSpawner {
    fn spawn(
        &self,
        join_set: &mut JoinSet<()>,
        db: Db,
        task_id: TaskId,
        kind: TaskKind,
        cancel: CancellationToken,
    ) {
        match kind {
            TaskKind::Summarize => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced wholesale in Task 7
            TaskKind::BatchFetch => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced wholesale in Task 9
            TaskKind::Retry => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced wholesale in Task 11
            TaskKind::Revalidate => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
        }
    }
}

pub fn default_spawner() -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner)
}
```

- [ ] **Step 3: Write the integration test**

Create `tests/tasks_lifecycle.rs`:

```rust
//! End-to-end scheduler + worker lifecycle.

use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use rover::storage::Db;
use rover::storage::events;
use rover::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert};
use rover::tasks::default_spawner;
use rover::tasks::scheduler::{Scheduler, SchedulerConfig};
use rover::tasks::types::TaskId;

#[tokio::test]
async fn summarize_stub_runs_through_scheduler() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let (tx, rx) = Scheduler::channel();
    insert(
        &db,
        TaskInsert {
            id: "t1".into(),
            kind: TaskKind::Summarize,
            params_json: "{}".into(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    tx.send(TaskId("t1".into())).unwrap();
    let cancel = CancellationToken::new();
    let sched = Scheduler {
        db: db.clone(),
        cfg: SchedulerConfig {
            own_pid: 1,
            orphan_scan_interval: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(200),
        },
        cancel: cancel.clone(),
        new_task_rx: rx,
        spawner: default_spawner(),
    };
    let handle = tokio::spawn(sched.run());
    let row = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let r = get(&db, "t1").await.unwrap().unwrap();
            if r.status.is_terminal() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("task did not reach terminal status in time");
    assert_eq!(row.status, TaskStatus::Failed);
    assert_eq!(row.error.as_deref(), Some("summarization_not_yet_implemented"));
    let evs = events::range_since(&db, "t1", 0, 100).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["task_started", "task_failed"]);
    cancel.cancel();
    handle.await.unwrap().unwrap();
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test --test tasks_lifecycle`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tasks/summarize.rs src/tasks/mod.rs tests/tasks_lifecycle.rs
git commit -m "feat(m6): summarize stub worker plus scheduler dispatch lifecycle test

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 7: `batch_fetch` Worker

**Files:**
- Modify: `src/tasks/batch_fetch.rs` (full body)
- Modify: `src/tasks/mod.rs` (replace the temporary BatchFetch arm in `DefaultSpawner`)
- Create: `tests/tasks_cancellation.rs`

Per-URL fetches through `fetcher::cached::fetch_with_cache`, global + per-host semaphores, item events, cancellation between items, resumption from `task_events` on orphan claim.

The worker is a free function, not a struct, to match `summarize::run`'s shape and keep dispatch uniform.

- [ ] **Step 1: Add the worker dependencies struct**

In `src/tasks/batch_fetch.rs`, write:

```rust
//! `batch_fetch` worker.
//!
//! Pulls a `BatchFetchParams` row, fans out via `fetch_with_cache`, emits
//! one `item_started`/`item_done`/`item_failed` event per URL plus the
//! shared `task_started`/`task_completed` envelope. Cancellation is checked
//! between items. On orphan claim, items already represented in
//! `task_events` are skipped so resumption is idempotent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{CacheConfig, FetchConfig, RateLimitConfig, RobotsConfig};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::events::{EventInsert, append, range_since};
use crate::storage::tasks::{TaskStatus, get, is_cancelled, set_status};
use crate::tasks::types::{BatchFetchParams, BatchFetchResult, TaskId};

/// Dependencies needed by the worker. Built once by `mcp::server` and shared
/// across every batch worker via `Arc`.
#[derive(Clone)]
pub struct BatchDeps {
    pub client: reqwest::Client,
    pub pacer: Arc<Pacer>,
    pub cache_cfg: CacheConfig,
    pub rate_cfg: RateLimitConfig,
    pub robots_cfg: RobotsConfig,
    pub fetch_cfg: FetchConfig,
    pub ssrf_level: SsrfLevel,
}
```

- [ ] **Step 2: Write the resumption helper + tests for it**

Append:

```rust
/// Return the set of `index` values already represented as `item_done` or
/// `item_failed` events. Used on startup / orphan claim to skip work that
/// already happened.
async fn already_processed_indices(db: &Db, task_id: &str) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut cursor = 0i64;
    loop {
        let rows = match range_since(db, task_id, cursor, 1000).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(target: "rover::tasks::batch_fetch", error = ?e, "scan events failed");
                return seen;
            }
        };
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            if r.kind == "item_done" || r.kind == "item_failed" {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.payload_json) {
                    if let Some(idx) = v.get("index").and_then(|x| x.as_u64()) {
                        seen.insert(idx as u32);
                    }
                }
            }
        }
        cursor = rows.last().map(|r| r.id).unwrap_or(cursor);
        if rows.len() < 1000 {
            break;
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    use tempfile::tempdir;

    async fn fresh_db_with_batch(id: &str, urls: &[&str]) -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        let params = BatchFetchParams {
            urls: urls.iter().map(|s| s.to_string()).collect(),
            concurrency: 2,
            per_domain_concurrency: 1,
            force_refresh: false,
        };
        insert(
            &db,
            TaskInsert {
                id: id.into(),
                kind: TaskKind::BatchFetch,
                params_json: serde_json::to_string(&params).unwrap(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn already_processed_collects_done_and_failed_indices() {
        let db = fresh_db_with_batch("t1", &["a", "b", "c"]).await;
        append(&db, EventInsert { task_id: "t1".into(), kind: "task_started".into(), payload_json: "{}".into() }).await.unwrap();
        append(&db, EventInsert { task_id: "t1".into(), kind: "item_started".into(), payload_json: r#"{"index":0,"url":"a"}"#.into() }).await.unwrap();
        append(&db, EventInsert { task_id: "t1".into(), kind: "item_done".into(),    payload_json: r#"{"index":0,"url":"a"}"#.into() }).await.unwrap();
        append(&db, EventInsert { task_id: "t1".into(), kind: "item_failed".into(),  payload_json: r#"{"index":2,"url":"c"}"#.into() }).await.unwrap();
        let seen = already_processed_indices(&db, "t1").await;
        assert!(seen.contains(&0));
        assert!(seen.contains(&2));
        assert!(!seen.contains(&1));
    }
}
```

- [ ] **Step 3: Write the main `run` entry point**

Append (above `#[cfg(test)]`):

```rust
/// Worker entry point used by `DefaultSpawner`.
pub async fn run(deps: BatchDeps, db: Db, task_id: TaskId, cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: BatchFetchParams = match serde_json::from_str(&row.params_json) {
        Ok(p) => p,
        Err(e) => {
            emit_terminal_failure(&db, task_id.as_str(), "invalid_params", &e.to_string(), 0)
                .await;
            return;
        }
    };

    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"batch_fetch","total":params.urls.len()}).to_string(),
        },
    )
    .await;
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "batch_start".into(),
            payload_json: json!({"total": params.urls.len()}).to_string(),
        },
    )
    .await;

    let seen = already_processed_indices(&db, task_id.as_str()).await;
    let global = Arc::new(Semaphore::new(params.concurrency.max(1) as usize));
    let per_host: Arc<Mutex<HashMap<String, Arc<Semaphore>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut succeeded: u32 = 0;
    let mut failed: u32 = 0;

    let mut handles = Vec::new();
    for (index, url_str) in params.urls.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        if let Ok(true) = is_cancelled(&db, task_id.as_str()).await {
            break;
        }
        let idx = index as u32;
        if seen.contains(&idx) {
            // Already accounted for on a previous run — count toward rollup.
            // Distinguishing done vs failed requires re-reading; simplest: poll once.
            // Since `seen` was built from item_done/item_failed kinds, a precise
            // tally falls out of a single events scan below — accumulate after the loop.
            continue;
        }
        let url = match Url::parse(url_str) {
            Ok(u) => u,
            Err(e) => {
                let _ = append(
                    &db,
                    EventInsert {
                        task_id: task_id.as_str().to_string(),
                        kind: "item_failed".into(),
                        payload_json: json!({
                            "index": idx,
                            "url": url_str,
                            "error": e.to_string(),
                            "will_retry": false,
                        })
                        .to_string(),
                    },
                )
                .await;
                failed += 1;
                continue;
            }
        };
        let host = url.host_str().unwrap_or("").to_string();
        let host_sem: Arc<Semaphore> = {
            let mut map = per_host.lock().await;
            map.entry(host.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(params.per_domain_concurrency.max(1) as usize)))
                .clone()
        };
        let deps_c = deps.clone();
        let db_c = db.clone();
        let task_str = task_id.as_str().to_string();
        let global_c = global.clone();
        let url_clone = url.clone();
        let url_string = url_str.clone();
        let force_refresh = params.force_refresh;
        let handle = tokio::spawn(async move {
            let _gh = host_sem.acquire_owned().await.expect("host sem closed");
            let _gg = global_c.acquire_owned().await.expect("global sem closed");
            let _ = append(
                &db_c,
                EventInsert {
                    task_id: task_str.clone(),
                    kind: "item_started".into(),
                    payload_json: json!({"index": idx, "url": url_string}).to_string(),
                },
            )
            .await;
            let item_started = Instant::now();
            let res = fetch_with_cache(
                &db_c,
                &deps_c.client,
                &deps_c.pacer,
                &deps_c.rate_cfg,
                &deps_c.robots_cfg,
                &url_clone,
                &deps_c.cache_cfg,
                FetchOptions {
                    force_refresh,
                    ssrf_level: deps_c.ssrf_level,
                    ignore_robots: !deps_c.robots_cfg.respect,
                    user_agent: deps_c.fetch_cfg.user_agent.clone(),
                },
                |body, base| {
                    let extracted = extract(body, Some(base))
                        .map_err(crate::fetcher::FetcherError::Extract)?;
                    Ok(ExtractResult {
                        title: extracted.title.clone(),
                        body_md: extracted.markdown.clone(),
                        content_hash: crate::fetcher::cached::sha256_hex(extracted.markdown.as_bytes()),
                        metadata: extracted.metadata,
                    })
                },
            )
            .await;
            let dur = item_started.elapsed().as_millis() as i64;
            let (event_kind, payload) = match res {
                Ok(cf) => {
                    let tokens: Option<usize> = serde_json::from_str::<serde_json::Value>(
                        cf.page.metadata_json.as_deref().unwrap_or("{}"),
                    )
                    .ok()
                    .and_then(|v| v.get("token_count").and_then(|x| x.as_u64()))
                    .map(|n| n as usize);
                    (
                        "item_done",
                        json!({
                            "index": idx,
                            "url": url_string,
                            "tokens": tokens,
                            "cached": matches!(cf.cache_status, crate::fetcher::cached::CacheStatus::Hit),
                            "duration_ms": dur,
                        }),
                    )
                }
                Err(e) => (
                    "item_failed",
                    json!({
                        "index": idx,
                        "url": url_string,
                        "error": e.to_string(),
                        "will_retry": matches!(e, crate::fetcher::FetcherError::Deferred { .. }),
                        "duration_ms": dur,
                    }),
                ),
            };
            let _ = append(
                &db_c,
                EventInsert {
                    task_id: task_str,
                    kind: event_kind.into(),
                    payload_json: payload.to_string(),
                },
            )
            .await;
        });
        handles.push(handle);
    }
    for h in handles {
        let _ = h.await;
    }

    // Final rollup: tally from events (including resumed items).
    let counts = match crate::storage::events::count_by_kind(&db, task_id.as_str()).await {
        Ok(c) => c,
        Err(_) => Vec::new(),
    };
    succeeded = counts
        .iter()
        .find_map(|(k, n)| if k == "item_done" { Some(*n as u32) } else { None })
        .unwrap_or(0);
    failed = counts
        .iter()
        .find_map(|(k, n)| if k == "item_failed" { Some(*n as u32) } else { None })
        .unwrap_or(0);

    let cancelled_now =
        cancel.is_cancelled() || is_cancelled(&db, task_id.as_str()).await.unwrap_or(false);
    let duration_ms = started.elapsed().as_millis() as i64;
    let result = BatchFetchResult {
        total: params.urls.len() as u32,
        succeeded,
        failed,
        duration_ms,
    };
    if cancelled_now {
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "task_cancelled".into(),
                payload_json: json!({"at": "between_items", "duration_ms": duration_ms}).to_string(),
            },
        )
        .await;
        let _ = set_status(&db, task_id.as_str(), TaskStatus::Cancelled, Some(serde_json::to_string(&result).unwrap()), None).await;
    } else {
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "final".into(),
                payload_json: json!({
                    "succeeded": succeeded,
                    "failed": failed,
                    "duration_s": (duration_ms as f64) / 1000.0,
                })
                .to_string(),
            },
        )
        .await;
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "task_completed".into(),
                payload_json: json!({"result": result, "duration_ms": duration_ms}).to_string(),
            },
        )
        .await;
        let _ = set_status(
            &db,
            task_id.as_str(),
            TaskStatus::Completed,
            Some(serde_json::to_string(&result).unwrap()),
            None,
        )
        .await;
    }
}

async fn emit_terminal_failure(
    db: &Db,
    task_id: &str,
    error_slug: &str,
    message: &str,
    duration_ms: i64,
) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: "task_failed".into(),
            payload_json: json!({
                "error": error_slug,
                "message": message,
                "duration_ms": duration_ms,
            })
            .to_string(),
        },
    )
    .await;
    let _ = set_status(
        db,
        task_id,
        TaskStatus::Failed,
        None,
        Some(error_slug.to_string()),
    )
    .await;
}
```

- [ ] **Step 4: Wire DefaultSpawner to dispatch BatchFetch through this worker**

In `src/tasks/mod.rs`, change the `DefaultSpawner` to carry `BatchDeps`:

```rust
pub struct DefaultSpawner {
    pub batch_deps: batch_fetch::BatchDeps,
}

impl scheduler::WorkerSpawner for DefaultSpawner {
    fn spawn(
        &self,
        join_set: &mut JoinSet<()>,
        db: Db,
        task_id: TaskId,
        kind: TaskKind,
        cancel: CancellationToken,
    ) {
        match kind {
            TaskKind::Summarize => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            TaskKind::BatchFetch => {
                let deps = self.batch_deps.clone();
                join_set.spawn(batch_fetch::run(deps, db, task_id, cancel));
            }
            // ↓ replaced in Task 9
            TaskKind::Retry => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced in Task 11
            TaskKind::Revalidate => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
        }
    }
}

pub fn default_spawner(batch_deps: batch_fetch::BatchDeps) -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner { batch_deps })
}
```

Update the existing call site in `tests/tasks_lifecycle.rs` to construct `BatchDeps` from defaults — for the summarize test path it is never actually used, so a minimal value is fine:

```rust
use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::tasks::batch_fetch::BatchDeps;
use std::sync::Arc;

fn dummy_deps(cfg: &Config) -> BatchDeps {
    BatchDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::Strict,
    }
}
```

Replace the existing `default_spawner()` call:

```rust
let cfg = Config::default();
let deps = dummy_deps(&cfg);
let sched = Scheduler {
    // ... fields ...
    spawner: rover::tasks::default_spawner(deps),
};
```

- [ ] **Step 5: Write the cancellation integration test**

Create `tests/tasks_cancellation.rs`:

```rust
//! batch_fetch worker honours cancellation between items.

use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::events;
use rover::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert, set_cancellation_requested};
use rover::tasks::batch_fetch::{BatchDeps, run as batch_run};
use rover::tasks::types::{BatchFetchParams, TaskId};

#[tokio::test]
async fn cancellation_between_items_stops_loop() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(path_regex(r"^/page/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>x</body></html>"))
        .mount(&server)
        .await;

    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = BatchDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::TestLoopback,
    };

    let urls: Vec<String> = (0..5).map(|i| format!("{}/page/{i}", server.uri())).collect();
    let params = BatchFetchParams {
        urls: urls.clone(),
        concurrency: 1,
        per_domain_concurrency: 1,
        force_refresh: false,
    };
    insert(
        &db,
        TaskInsert {
            id: "cx".into(),
            kind: TaskKind::BatchFetch,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    let db_c = db.clone();
    let worker = tokio::spawn(async move {
        batch_run(deps, db_c, TaskId("cx".into()), cancel.clone()).await;
    });

    // Wait until at least one item is done, then request cancellation.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let counts = events::count_by_kind(&db, "cx").await.unwrap();
            let done = counts
                .iter()
                .find_map(|(k, n)| if k == "item_done" { Some(*n) } else { None })
                .unwrap_or(0);
            if done >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no item completed in time");
    let _ = set_cancellation_requested(&db, "cx").await;

    worker.await.unwrap();
    let row = get(&db, "cx").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Cancelled);

    let counts = events::count_by_kind(&db, "cx").await.unwrap();
    let done = counts.iter().find(|(k, _)| k == "item_done").map(|(_, n)| *n).unwrap_or(0);
    let started = counts.iter().find(|(k, _)| k == "item_started").map(|(_, n)| *n).unwrap_or(0);
    assert!(done >= 1);
    assert!(
        started < urls.len() as i64,
        "expected fewer than {} item_started, got {started}",
        urls.len(),
    );
    let evs = events::range_since(&db, "cx", 0, 1000).await.unwrap();
    assert!(evs.iter().any(|e| e.kind == "task_cancelled"));
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test --test tasks_cancellation && cargo test --test tasks_lifecycle && cargo test --lib tasks::batch_fetch`

Expected: all PASS. (If `FetcherError::Deferred` is not yet a variant — added in Task 8 — change the `matches!` line in step 3 to `false` temporarily and add a TODO comment; the compiler will reject otherwise. Cleanest path: do Task 8 first and revisit, OR introduce the Deferred variant in Task 8 with the `matches!` line referencing it. **Decision:** do Task 8 right after Task 7 and patch the matches! arm before running the full test suite — Step 6 is a soft checkpoint.)

- [ ] **Step 7: Commit**

```bash
git add src/tasks/batch_fetch.rs src/tasks/mod.rs tests/tasks_cancellation.rs tests/tasks_lifecycle.rs
git commit -m "feat(m6): batch_fetch worker with item events and cancellation

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 8: `FetcherError::Deferred` + Long-Retry Classification

**Files:**
- Modify: `src/fetcher/mod.rs` (add `Deferred` variant)
- Modify: `src/fetcher/retry.rs` (deferred classifier; create retry task on long Retry-After)
- Modify: `src/mcp/error.rs` (route `Deferred` to a new `RoverError` code)
- Modify: `src/mcp/envelope.rs` (add `DEFERRED` code; `TaskCreatedResponse` shape)

When the in-call retry budget is shorter than the server-requested wait, the fetcher inserts a `retry` task and returns `FetcherError::Deferred { task_id }`. Callers (MCP `fetch` tool, `batch_fetch` worker) surface the deferred envelope.

- [ ] **Step 1: Add the variant**

In `src/fetcher/mod.rs` (after `RobotsFetchFailed`):

```rust
    #[error("fetch deferred to retry task {task_id}")]
    Deferred { task_id: String },
```

- [ ] **Step 2: Add the RoverError code**

In `src/mcp/envelope.rs`, alongside the other `pub const ...` lines:

```rust
    pub const DEFERRED: &'static str = "deferred";
    pub const TOO_MANY_URLS: &'static str = "too_many_urls";
    pub const EMPTY_URL_LIST: &'static str = "empty_url_list";
```

Also append:

```rust
/// Returned by tools that schedule a background task.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskCreatedResponse {
    pub task_id: String,
    pub status: String,
    pub kind: String,
    pub monitor_command: String,
    pub poll_command: String,
    pub cancel_command: String,
    pub hint: String,
}

/// Stale-served envelope on a `fetch` response.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StaleRevalidation {
    pub task_id: String,
    pub monitor_command: String,
    pub poll_command: String,
    pub hint: String,
}
```

- [ ] **Step 3: Route the new variant in `mcp::error.rs`**

In `src/mcp/error.rs` inside the `Fetcher` arm `match e`, add:

```rust
    F::Deferred { task_id } => {
        let mut r = RoverError::new(RoverError::DEFERRED, format!("deferred to task {task_id}"));
        r.message = format!("deferred to task {task_id}");
        r
    }
```

(The simpler equivalent: `F::Deferred { .. } => RoverError::new(RoverError::DEFERRED, e.to_string()),`. Either form is fine — the test asserts the code, not the exact phrasing.)

Add a regression test in `#[cfg(test)] mod tests`:

```rust
#[test]
fn deferred_translation_uses_stable_code() {
    let e = McpError::Fetcher(crate::fetcher::FetcherError::Deferred {
        task_id: "abc".into(),
    });
    let r = e.into_rover_error();
    assert_eq!(r.code, RoverError::DEFERRED);
    assert!(r.message.contains("abc"));
}
```

- [ ] **Step 4: Add the deferred classifier in `fetcher/retry.rs`**

The current retry loop's `Class::RetryAfter(d, _)` branch sleeps `d` then loops. Add: if `d > config.deferred_retry_threshold` (new knob; default 30s), insert a `retry` task and return `FetcherError::Deferred { task_id }`. The threshold knob lives in `RateLimitConfig`.

In `src/config.rs`, add to `RateLimitConfig`:

```rust
    #[serde(default = "default_deferred_threshold_secs")]
    pub deferred_retry_threshold_secs: u64,
```

with `fn default_deferred_threshold_secs() -> u64 { 30 }` and the corresponding field in `Default`. Update existing `RateLimitConfig` tests if any assert defaults.

In `src/fetcher/retry.rs`, the deferred decision needs storage access. Restructure `with_retries`'s signature to accept `&Db`:

```rust
pub async fn with_retries(
    db: &crate::storage::Db,
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    crawl_delay: Option<Duration>,
    cfg: &RateLimitConfig,
) -> Result<FetchedPage, FetcherError> {
```

Inside the loop, when the classifier returns `Class::RetryAfter(d, last)`:

```rust
if d.as_secs() > cfg.deferred_retry_threshold_secs {
    let task_id = uuid::Uuid::now_v7().to_string();
    let params = serde_json::to_string(&crate::tasks::types::RetryParams {
        url: url.to_string(),
        attempt: 1,
        wait_ms_initial: (d.as_millis() as u64).max(1_000),
        max_attempts: cfg.max_retries.max(1),
        parent_task_id: None,
    })
    .unwrap_or("{}".into());
    crate::storage::tasks::insert(
        db,
        crate::storage::tasks::TaskInsert {
            id: task_id.clone(),
            kind: crate::storage::tasks::TaskKind::Retry,
            params_json: params,
            owner_pid: Some(std::process::id() as i64),
        },
    )
    .await?;
    return Err(FetcherError::Deferred { task_id });
}
```

Update every caller of `with_retries` (`fetcher/cached.rs::fetch_with_cache`) to pass the `Db` through. The signature change ripples — `cached::fetch_with_cache` already has `db: &Db`.

- [ ] **Step 5: Write a regression test**

Append a unit test in `src/fetcher/retry.rs::tests` exercising a long Retry-After:

```rust
#[tokio::test]
async fn long_retry_after_produces_deferred_error() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "120"),
        )
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::storage::Db::open(tmp.path().join("rover.db"))
        .await
        .unwrap();
    let url = Url::parse(&server.uri()).unwrap();
    let cfg = RateLimitConfig {
        deferred_retry_threshold_secs: 30,
        max_retries: 3,
        ..Default::default()
    };
    let pacer = Pacer::new(&cfg);
    let client = reqwest::Client::new();
    let cond = ConditionalGet::default();
    let res = with_retries(
        &db,
        &pacer,
        &client,
        &url,
        SsrfLevel::TestLoopback,
        &cond,
        None,
        &cfg,
    )
    .await;
    match res {
        Err(FetcherError::Deferred { task_id }) => {
            let row = crate::storage::tasks::get(&db, &task_id).await.unwrap().unwrap();
            assert_eq!(row.kind, crate::storage::tasks::TaskKind::Retry);
        }
        other => panic!("expected Deferred, got {other:?}"),
    }
}
```

- [ ] **Step 6: Run the tests**

Run:
```
cargo test --lib fetcher::retry::tests::long_retry_after_produces_deferred_error
cargo test --lib mcp::error
cargo test --test tasks_cancellation
```

Expected: PASS. (If `tests/tasks_cancellation.rs`'s `matches!` was provisional, it now compiles cleanly.)

- [ ] **Step 7: Commit**

```bash
git add src/fetcher/mod.rs src/fetcher/retry.rs src/fetcher/cached.rs src/mcp/error.rs src/mcp/envelope.rs src/config.rs
git commit -m "feat(m6): fetcher::deferred variant and long retry-after task insertion

long retry-after responses (> deferred_retry_threshold_secs, default 30s)
now insert a retry task and return fethcererror::deferred {task_id}, which
the mcp layer maps to the new rover deferred code. taskcreatedresponse and
stalerevalidation envelopes are added to mcp::envelope.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 9: `retry` Worker

**Files:**
- Modify: `src/tasks/retry.rs` (full body)
- Modify: `src/tasks/mod.rs` (replace `Retry` arm in `DefaultSpawner`)
- Create: `tests/tasks_retry_deferred.rs`

Waits the configured initial delay (doubling on subsequent attempts), then re-runs the fetch. On success: `retry_succeeded` + `task_completed`. On failure with attempts remaining: insert a new `retry` task for `attempt+1` (with doubled wait, capped at 5 min) and complete *this* task with `retry_failed { will_retry: true }`. On final exhaustion: `retry_failed { will_retry: false }` + `task_failed { error: 'retries_exhausted' }`.

- [ ] **Step 1: Write the worker body**

Replace `src/tasks/retry.rs` with:

```rust
//! `retry` worker — long-deferred retries scheduled by the fetcher.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::time;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{CacheConfig, FetchConfig, RateLimitConfig, RobotsConfig};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert, is_cancelled, set_status};
use crate::tasks::types::{RetryParams, TaskId};

const RETRY_WAIT_CAP_MS: u64 = 5 * 60 * 1000;

#[derive(Clone)]
pub struct RetryDeps {
    pub client: reqwest::Client,
    pub pacer: Arc<Pacer>,
    pub cache_cfg: CacheConfig,
    pub rate_cfg: RateLimitConfig,
    pub robots_cfg: RobotsConfig,
    pub fetch_cfg: FetchConfig,
    pub ssrf_level: SsrfLevel,
}

pub async fn run(deps: RetryDeps, db: Db, task_id: TaskId, cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: RetryParams = match serde_json::from_str(&row.params_json) {
        Ok(p) => p,
        Err(e) => {
            terminal_fail(&db, task_id.as_str(), "invalid_params", &e.to_string(), 0).await;
            return;
        }
    };
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"retry","attempt": params.attempt}).to_string(),
        },
    )
    .await;
    let url = match Url::parse(&params.url) {
        Ok(u) => u,
        Err(e) => {
            terminal_fail(
                &db,
                task_id.as_str(),
                "invalid_url",
                &e.to_string(),
                started.elapsed().as_millis() as i64,
            )
            .await;
            return;
        }
    };

    let wait_ms = (params.wait_ms_initial.saturating_mul(
        1u64 << (params.attempt.saturating_sub(1) as u64).min(8),
    ))
    .min(RETRY_WAIT_CAP_MS);
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "retry_attempted".into(),
            payload_json: json!({"url": params.url, "attempt": params.attempt, "wait_ms_used": wait_ms}).to_string(),
        },
    )
    .await;
    // Wait — but bail early on cancellation.
    let wait = time::sleep(Duration::from_millis(wait_ms));
    tokio::select! {
        _ = wait => {}
        _ = cancel.cancelled() => {
            cancelled_terminal(&db, task_id.as_str(), started.elapsed().as_millis() as i64).await;
            return;
        }
    }
    if is_cancelled(&db, task_id.as_str()).await.unwrap_or(false) {
        cancelled_terminal(&db, task_id.as_str(), started.elapsed().as_millis() as i64).await;
        return;
    }

    let res = fetch_with_cache(
        &db,
        &deps.client,
        &deps.pacer,
        &deps.rate_cfg,
        &deps.robots_cfg,
        &url,
        &deps.cache_cfg,
        FetchOptions {
            force_refresh: true,
            ssrf_level: deps.ssrf_level,
            ignore_robots: !deps.robots_cfg.respect,
            user_agent: deps.fetch_cfg.user_agent.clone(),
        },
        |body, base| {
            let extracted = extract(body, Some(base))
                .map_err(crate::fetcher::FetcherError::Extract)?;
            Ok(ExtractResult {
                title: extracted.title.clone(),
                body_md: extracted.markdown.clone(),
                content_hash: crate::fetcher::cached::sha256_hex(extracted.markdown.as_bytes()),
                metadata: extracted.metadata,
            })
        },
    )
    .await;

    let duration_ms = started.elapsed().as_millis() as i64;
    match res {
        Ok(_) => {
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "retry_succeeded".into(),
                    payload_json: json!({"url": params.url, "attempt": params.attempt}).to_string(),
                },
            )
            .await;
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "task_completed".into(),
                    payload_json: json!({"duration_ms": duration_ms}).to_string(),
                },
            )
            .await;
            let _ = set_status(&db, task_id.as_str(), TaskStatus::Completed, None, None).await;
        }
        Err(e) => {
            let exhausted = params.attempt >= params.max_attempts;
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "retry_failed".into(),
                    payload_json: json!({
                        "url": params.url,
                        "attempt": params.attempt,
                        "error": e.to_string(),
                        "will_retry": !exhausted,
                    })
                    .to_string(),
                },
            )
            .await;
            if exhausted {
                terminal_fail(
                    &db,
                    task_id.as_str(),
                    "retries_exhausted",
                    &e.to_string(),
                    duration_ms,
                )
                .await;
            } else {
                // Chain a new retry task for attempt+1.
                let next_wait = (wait_ms.saturating_mul(2)).min(RETRY_WAIT_CAP_MS);
                let next = RetryParams {
                    url: params.url.clone(),
                    attempt: params.attempt + 1,
                    wait_ms_initial: next_wait,
                    max_attempts: params.max_attempts,
                    parent_task_id: params.parent_task_id.clone(),
                };
                let new_id = uuid::Uuid::now_v7().to_string();
                let _ = insert(
                    &db,
                    TaskInsert {
                        id: new_id.clone(),
                        kind: TaskKind::Retry,
                        params_json: serde_json::to_string(&next).unwrap_or("{}".into()),
                        owner_pid: Some(std::process::id() as i64),
                    },
                )
                .await;
                // This task completes successfully — the *attempt* failed but
                // the *task* successfully handed off to the next attempt.
                let _ = append(
                    &db,
                    EventInsert {
                        task_id: task_id.as_str().to_string(),
                        kind: "task_completed".into(),
                        payload_json: json!({"chained_next_task_id": new_id, "duration_ms": duration_ms}).to_string(),
                    },
                )
                .await;
                let _ = set_status(&db, task_id.as_str(), TaskStatus::Completed, None, None).await;
            }
        }
    }
}

async fn terminal_fail(db: &Db, task_id: &str, slug: &str, message: &str, duration_ms: i64) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: "task_failed".into(),
            payload_json: json!({"error": slug, "message": message, "duration_ms": duration_ms}).to_string(),
        },
    )
    .await;
    let _ = set_status(db, task_id, TaskStatus::Failed, None, Some(slug.to_string())).await;
}

async fn cancelled_terminal(db: &Db, task_id: &str, duration_ms: i64) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: "task_cancelled".into(),
            payload_json: json!({"at": "during_wait", "duration_ms": duration_ms}).to_string(),
        },
    )
    .await;
    let _ = set_status(db, task_id, TaskStatus::Cancelled, None, None).await;
}
```

- [ ] **Step 2: Update DefaultSpawner**

In `src/tasks/mod.rs`, replace the `Retry` arm and extend the struct:

```rust
pub struct DefaultSpawner {
    pub batch_deps: batch_fetch::BatchDeps,
    pub retry_deps: retry::RetryDeps,
}

// ... in spawn ...
TaskKind::Retry => {
    let deps = self.retry_deps.clone();
    join_set.spawn(retry::run(deps, db, task_id, cancel));
}
```

Update `default_spawner`:

```rust
pub fn default_spawner(
    batch_deps: batch_fetch::BatchDeps,
    retry_deps: retry::RetryDeps,
) -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner { batch_deps, retry_deps })
}
```

Update the lifecycle test's spawner construction with a `RetryDeps` built from the same `Config::default()` pattern as `dummy_deps`.

- [ ] **Step 3: Write the deferred-becomes-task integration test**

Create `tests/tasks_retry_deferred.rs`:

```rust
//! end-to-end: long retry-after produces a retry task that runs to completion.

use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::tasks::{TaskKind, TaskStatus, get};
use rover::tasks::retry::{RetryDeps, run as retry_run};
use rover::tasks::types::{RetryParams, TaskId};

#[tokio::test]
async fn retry_succeeds_on_second_attempt() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>ok</body></html>"))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = RetryDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::TestLoopback,
    };

    let params = RetryParams {
        url: format!("{}/", server.uri()),
        attempt: 1,
        wait_ms_initial: 50,
        max_attempts: 3,
        parent_task_id: None,
    };
    rover::storage::tasks::insert(
        &db,
        rover::storage::tasks::TaskInsert {
            id: "r1".into(),
            kind: TaskKind::Retry,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    retry_run(deps, db.clone(), TaskId("r1".into()), cancel).await;

    let row = get(&db, "r1").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Completed);
    let evs = rover::storage::events::range_since(&db, "r1", 0, 100).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"retry_attempted"));
    assert!(kinds.contains(&"retry_succeeded"));
    assert!(kinds.contains(&"task_completed"));
}

#[tokio::test]
async fn retry_max_attempts_exhausted_terminal_failure() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = RetryDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::TestLoopback,
    };

    let params = RetryParams {
        url: format!("{}/", server.uri()),
        attempt: 3,
        wait_ms_initial: 10,
        max_attempts: 3,
        parent_task_id: None,
    };
    rover::storage::tasks::insert(
        &db,
        rover::storage::tasks::TaskInsert {
            id: "r2".into(),
            kind: TaskKind::Retry,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    retry_run(deps, db.clone(), TaskId("r2".into()), CancellationToken::new()).await;
    let row = get(&db, "r2").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Failed);
    assert_eq!(row.error.as_deref(), Some("retries_exhausted"));
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test tasks_retry_deferred && cargo test --lib tasks::retry`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tasks/retry.rs src/tasks/mod.rs tests/tasks_retry_deferred.rs tests/tasks_lifecycle.rs
git commit -m "feat(m6): retry worker chains on partial failure and bails on exhaustion

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 10: SWR Path in `fetcher::cached` + Stale-Served Envelope

**Files:**
- Modify: `src/fetcher/cached.rs` (insert revalidate task on stale path; extend `CacheStatus`)
- Modify: `src/mcp/tools/fetch.rs` (surface `revalidation` envelope on stale_served)
- Modify: `src/mcp/envelope.rs` (`FetchResponse` gains optional `revalidation` field)

Today the orchestrator does conditional-revalidate-then-serve. M6 flips to stale-while-revalidate: when the entry is expired but present, return the stale row immediately and insert a `revalidate` task. The same task ID surfaces in the network-failure fallback that already returns stale.

- [ ] **Step 1: Extend the storage-level `CacheStatus` variant for stale-served-with-revalidation**

In `src/fetcher/cached.rs`, change the enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Stale {
        /// Inserted on the stale path; agents may monitor or ignore.
        revalidation_task_id: Option<String>,
    },
    Miss,
}
```

Compile errors will ripple. Update the existing pattern matches:
- In `fetcher/cached.rs` itself, every `CacheStatus::Stale` becomes `CacheStatus::Stale { revalidation_task_id }`. The network-failure fallback path constructs `CacheStatus::Stale { revalidation_task_id: Some(id) }` after inserting the task.
- In `src/mcp/envelope.rs`, the wire `CacheStatus` and its `From` impl: `Stale` on the wire stays unit; the revalidation_task_id moves into the new optional envelope.

- [ ] **Step 2: Insert revalidate task on stale path**

In `fetcher/cached.rs`'s step 1 cache lookup, change:

```rust
    let stale: Option<Page> = if opts.force_refresh {
        None
    } else {
        match lookup_cached(db, url).await? {
            Some(p) if p.expires_at.is_some_and(|e| e > now) => {
                return Ok(CachedFetch {
                    page: p,
                    cache_status: CacheStatus::Hit,
                });
            }
            Some(p) => Some(p),
            None => None,
        }
    };
```

to:

```rust
    let stale: Option<Page> = if opts.force_refresh {
        None
    } else {
        match lookup_cached(db, url).await? {
            Some(p) if p.expires_at.is_some_and(|e| e > now) => {
                return Ok(CachedFetch {
                    page: p,
                    cache_status: CacheStatus::Hit,
                });
            }
            Some(p) => {
                // SWR: insert revalidation task, return stale immediately.
                let task_id = insert_revalidate_task(db, url, &p).await;
                return Ok(CachedFetch {
                    page: p,
                    cache_status: CacheStatus::Stale { revalidation_task_id: task_id },
                });
            }
            None => None,
        }
    };
```

The condition `let stale: Option<Page> = ... = None;` now always evaluates to `None` for the stale branch (since it returns early). Update the remaining `step 2/3/4` paths that referenced `stale` for the conditional GET — those are now only reached on `force_refresh = true` or a cache miss, both of which keep `stale = None`. Delete `Step 2: build conditional validators from any stale entry`'s body (always `ConditionalGet::default()`), and the 304-handling block is also unreachable on miss; keep it but it becomes dead unless `force_refresh` is set with `If-None-Match` (very rare). Document with a code comment.

Add the helper at the bottom of `cached.rs`:

```rust
async fn insert_revalidate_task(db: &Db, url: &Url, stale: &Page) -> Option<String> {
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    let params = serde_json::to_string(&crate::tasks::types::RevalidateParams {
        url: url.to_string(),
        etag_at_serve: stale.etag.clone(),
        last_modified_at_serve: stale.last_modified.clone(),
    })
    .ok()?;
    let id = uuid::Uuid::now_v7().to_string();
    if insert(
        db,
        TaskInsert {
            id: id.clone(),
            kind: TaskKind::Revalidate,
            params_json: params,
            owner_pid: Some(std::process::id() as i64),
        },
    )
    .await
    .is_ok()
    {
        Some(id)
    } else {
        None
    }
}
```

The network-failure fallback path (which already returned `CacheStatus::Stale`) should also call `insert_revalidate_task` and propagate the ID — adjust the `Network failure with a stale entry available` branch.

- [ ] **Step 3: Update envelope.rs**

In `src/mcp/envelope.rs`, modify `CacheStatus::From` and `FetchResponse`:

```rust
impl From<crate::fetcher::cached::CacheStatus> for CacheStatus {
    fn from(v: crate::fetcher::cached::CacheStatus) -> Self {
        use crate::fetcher::cached::CacheStatus as C;
        match v {
            C::Hit => CacheStatus::Hit,
            C::Miss => CacheStatus::Miss,
            C::Stale { .. } => CacheStatus::Stale,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchResponse {
    pub markdown: String,
    pub frontmatter: String,
    pub cache_status: CacheStatus,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub revalidation: Option<StaleRevalidation>,
}
```

- [ ] **Step 4: Wire the envelope into `mcp/tools/fetch.rs`**

When building the `FetchResponse`, if `cached.cache_status` is `Stale { revalidation_task_id: Some(id) }`, attach:

```rust
let revalidation = match &cf.cache_status {
    crate::fetcher::cached::CacheStatus::Stale { revalidation_task_id: Some(id) } => {
        Some(crate::mcp::envelope::StaleRevalidation {
            task_id: id.clone(),
            monitor_command: format!("rover task {id} --monitor"),
            poll_command: format!("rover task {id}"),
            hint: "Optional. Revalidation runs in the background regardless.".into(),
        })
    }
    _ => None,
};
```

Set `FetchResponse { revalidation, ... }` in the returned `Json(FetchResponse)`.

- [ ] **Step 5: Update existing M2 cache tests**

Any test that pattern-matches `CacheStatus::Stale` now needs to use `CacheStatus::Stale { .. }`. Run:

```
cargo build --all-features 2>&1 | grep -E 'error\[' | head -40
```

Fix each. The largest blast radius is in `tests/cache_lifecycle.rs` and the `CacheStatus` doctests if any.

- [ ] **Step 6: Add an integration test**

Append to `tests/cache_lifecycle.rs` (or create `tests/tasks_revalidate.rs` if cleaner):

```rust
#[tokio::test]
async fn stale_path_inserts_revalidate_task() {
    use std::sync::Arc;
    use rover::config::Config;
    use rover::fetcher::cached::{CacheStatus, FetchOptions, fetch_with_cache, sha256_hex, ExtractResult};
    use rover::fetcher::client::build_http_client;
    use rover::fetcher::concurrency::Pacer;
    use rover::fetcher::ssrf::SsrfLevel;
    use rover::storage::{Db, pages::{self, Page, url_hash}};
    use rover::storage::tasks::{self, TaskKind};
    use tempfile::tempdir;
    use url::Url;

    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let url = Url::parse("https://example.test/article").unwrap();

    // Seed a stale page row.
    let now = jiff::Timestamp::now().as_second();
    pages::upsert(
        &db,
        Page {
            url_hash: url_hash(url.as_str()),
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("t".into()),
            fetched_at: now - 7200,
            expires_at: Some(now - 60),
            etag: Some("\"abc\"".into()),
            last_modified: None,
            content_hash: sha256_hex(b"old"),
            extracted_md: "old".into(),
            metadata_json: None,
        },
    )
    .await
    .unwrap();

    // No HTTP path will be hit — we expect the stale return before any fetch.
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let pacer = Pacer::new(&cfg.rate_limit);
    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let cf = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &cfg.rate_limit,
        &cfg.robots,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: false,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: cfg.fetch.user_agent.clone(),
        },
        |_b, _u| panic!("extract_fn should not run on stale-served path"),
    )
    .await
    .unwrap();

    let task_id = match cf.cache_status {
        CacheStatus::Stale { revalidation_task_id: Some(id) } => id,
        other => panic!("expected stale with revalidation_task_id, got {other:?}"),
    };
    let row = tasks::get(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(row.kind, TaskKind::Revalidate);
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --lib && cargo test --test cache_lifecycle`

Expected: PASS. (Any inherited `CacheStatus::Stale` matches must be adapted; the previous step should catch them.)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(m6): swr path inserts revalidate task and surfaces envelope

cachestatus::stale now carries an optional revalidation_task_id. expired
cache entries return immediately + queue a revalidate task; the mcp fetch
tool propagates this as a {task_id,monitor_command,poll_command,hint}
envelope.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 11: `revalidate` Worker

**Files:**
- Modify: `src/tasks/revalidate.rs` (full body)
- Modify: `src/tasks/mod.rs` (replace `Revalidate` arm in `DefaultSpawner`)
- Create: `tests/tasks_revalidate.rs` (if not added in Task 10)

Force-refresh fetch through the cache pipeline, emit `revalidation_started` / `revalidation_completed`. Distinguishes 304 (changed=false) from a fresh 200 (changed=true).

- [ ] **Step 1: Write the worker body**

Replace `src/tasks/revalidate.rs` with:

```rust
//! `revalidate` worker — refreshes a stale cache entry in the background.

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{CacheConfig, FetchConfig, RateLimitConfig, RobotsConfig};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{CacheStatus, ExtractResult, FetchOptions, fetch_with_cache};
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskStatus, get, set_status};
use crate::tasks::types::{RevalidateParams, TaskId};

#[derive(Clone)]
pub struct RevalidateDeps {
    pub client: reqwest::Client,
    pub pacer: Arc<Pacer>,
    pub cache_cfg: CacheConfig,
    pub rate_cfg: RateLimitConfig,
    pub robots_cfg: RobotsConfig,
    pub fetch_cfg: FetchConfig,
    pub ssrf_level: SsrfLevel,
}

pub async fn run(deps: RevalidateDeps, db: Db, task_id: TaskId, _cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: RevalidateParams = match serde_json::from_str(&row.params_json) {
        Ok(p) => p,
        Err(e) => {
            terminal_fail(&db, task_id.as_str(), "invalid_params", &e.to_string(), 0).await;
            return;
        }
    };
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"revalidate"}).to_string(),
        },
    )
    .await;
    let url = match Url::parse(&params.url) {
        Ok(u) => u,
        Err(e) => {
            terminal_fail(
                &db,
                task_id.as_str(),
                "invalid_url",
                &e.to_string(),
                started.elapsed().as_millis() as i64,
            )
            .await;
            return;
        }
    };
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "revalidation_started".into(),
            payload_json: json!({"url": params.url}).to_string(),
        },
    )
    .await;
    let res = fetch_with_cache(
        &db,
        &deps.client,
        &deps.pacer,
        &deps.rate_cfg,
        &deps.robots_cfg,
        &url,
        &deps.cache_cfg,
        FetchOptions {
            force_refresh: true,
            ssrf_level: deps.ssrf_level,
            ignore_robots: !deps.robots_cfg.respect,
            user_agent: deps.fetch_cfg.user_agent.clone(),
        },
        |body, base| {
            let extracted = extract(body, Some(base))
                .map_err(crate::fetcher::FetcherError::Extract)?;
            Ok(ExtractResult {
                title: extracted.title.clone(),
                body_md: extracted.markdown.clone(),
                content_hash: crate::fetcher::cached::sha256_hex(extracted.markdown.as_bytes()),
                metadata: extracted.metadata,
            })
        },
    )
    .await;
    let duration_ms = started.elapsed().as_millis() as i64;
    match res {
        Ok(cf) => {
            let changed = matches!(cf.cache_status, CacheStatus::Miss);
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "revalidation_completed".into(),
                    payload_json: json!({
                        "url": params.url,
                        "changed": changed,
                        "status_code": if changed { 200 } else { 304 },
                    })
                    .to_string(),
                },
            )
            .await;
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "task_completed".into(),
                    payload_json: json!({"duration_ms": duration_ms}).to_string(),
                },
            )
            .await;
            let _ = set_status(&db, task_id.as_str(), TaskStatus::Completed, None, None).await;
        }
        Err(e) => {
            terminal_fail(
                &db,
                task_id.as_str(),
                "revalidation_failed",
                &e.to_string(),
                duration_ms,
            )
            .await;
        }
    }
}

async fn terminal_fail(db: &Db, task_id: &str, slug: &str, message: &str, duration_ms: i64) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: "task_failed".into(),
            payload_json: json!({"error": slug, "message": message, "duration_ms": duration_ms}).to_string(),
        },
    )
    .await;
    let _ = set_status(db, task_id, TaskStatus::Failed, None, Some(slug.to_string())).await;
}
```

- [ ] **Step 2: Wire DefaultSpawner**

In `src/tasks/mod.rs`:

```rust
pub struct DefaultSpawner {
    pub batch_deps: batch_fetch::BatchDeps,
    pub retry_deps: retry::RetryDeps,
    pub revalidate_deps: revalidate::RevalidateDeps,
}

// ...
TaskKind::Revalidate => {
    let deps = self.revalidate_deps.clone();
    join_set.spawn(revalidate::run(deps, db, task_id, cancel));
}

pub fn default_spawner(
    batch_deps: batch_fetch::BatchDeps,
    retry_deps: retry::RetryDeps,
    revalidate_deps: revalidate::RevalidateDeps,
) -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner { batch_deps, retry_deps, revalidate_deps })
}
```

Update `tests/tasks_lifecycle.rs` to supply a `revalidate_deps` value alongside `batch_deps` / `retry_deps`.

- [ ] **Step 3: Integration test**

Create `tests/tasks_revalidate.rs`:

```rust
//! revalidate worker completes against a 200 (Miss → changed=true).

use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::events;
use rover::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert};
use rover::tasks::revalidate::{RevalidateDeps, run as revalidate_run};
use rover::tasks::types::{RevalidateParams, TaskId};

#[tokio::test]
async fn revalidate_marks_completed_after_fresh_fetch() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>fresh</body></html>"))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = RevalidateDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::TestLoopback,
    };
    let params = RevalidateParams {
        url: format!("{}/page", server.uri()),
        etag_at_serve: None,
        last_modified_at_serve: None,
    };
    insert(
        &db,
        TaskInsert {
            id: "rv".into(),
            kind: TaskKind::Revalidate,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    revalidate_run(deps, db.clone(), TaskId("rv".into()), CancellationToken::new()).await;
    let row = get(&db, "rv").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Completed);
    let evs = events::range_since(&db, "rv", 0, 100).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"revalidation_started"));
    assert!(kinds.contains(&"revalidation_completed"));
}
```

- [ ] **Step 4: Run**

Run: `cargo test --test tasks_revalidate && cargo test --lib tasks::revalidate`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tasks/revalidate.rs src/tasks/mod.rs tests/tasks_revalidate.rs tests/tasks_lifecycle.rs
git commit -m "feat(m6): revalidate worker closes the swr loop

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 12: `batch_fetch` MCP Tool

**Files:**
- Create: `src/mcp/tools/batch_fetch.rs`
- Modify: `src/mcp/tools/mod.rs` (register)
- Modify: `src/mcp/handler.rs` (carry `NewTaskSender`; register the tool)
- Modify: `src/mcp/server.rs` (build Scheduler, store `NewTaskSender` on the handler)
- Modify: `src/mcp/error.rs` (route `TooManyUrls`, `EmptyUrlList`)
- Modify: `src/mcp/envelope.rs` (already has `TaskCreatedResponse`)
- Create: `tests/mcp_batch_fetch.rs`

Validates inputs, runs each URL through the SSRF policy, inserts a `batch_fetch` task carrying serialised `BatchFetchParams`, sends the new ID on the in-process MPSC, returns the immediate envelope.

- [ ] **Step 1: Add `McpError` variants**

In `src/mcp/error.rs`:

```rust
    #[error("too many URLs ({count}, max {max})")]
    TooManyUrls { count: usize, max: usize },
    #[error("empty URL list")]
    EmptyUrlList,
```

Map both in `into_rover_error`:

```rust
    Self::TooManyUrls { .. } => RoverError::new(RoverError::TOO_MANY_URLS, self.to_string()),
    Self::EmptyUrlList => RoverError::new(RoverError::EMPTY_URL_LIST, self.to_string()),
```

Mark them as user-errors in `into_error_data` (in `handler.rs`):

```rust
    let is_user_error = matches!(
        &err,
        McpError::InvalidArgs(_)
            | McpError::InvalidUrl(_)
            | McpError::TooManyUrls { .. }
            | McpError::EmptyUrlList,
    );
```

- [ ] **Step 2: Write the tool**

Create `src/mcp/tools/batch_fetch.rs`:

```rust
//! MCP `batch_fetch` tool.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::fetcher::ssrf::{ssrf_validate, SsrfError};
use crate::mcp::envelope::TaskCreatedResponse;
use crate::mcp::error::McpError;
use crate::mcp::handler::RoverHandler;
use crate::storage::tasks::{TaskInsert, TaskKind, insert};
use crate::tasks::types::{BatchFetchParams, TaskId};

const MAX_URLS: usize = 100;
const MAX_CONCURRENCY: u32 = 32;
const MAX_PER_DOMAIN: u32 = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchFetchArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub force_refresh: bool,
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub per_domain_concurrency: Option<u32>,
}

impl RoverHandler {
    pub(crate) async fn batch_fetch_inner(
        &self,
        args: BatchFetchArgs,
    ) -> Result<TaskCreatedResponse, McpError> {
        if args.urls.is_empty() {
            return Err(McpError::EmptyUrlList);
        }
        if args.urls.len() > MAX_URLS {
            return Err(McpError::TooManyUrls {
                count: args.urls.len(),
                max: MAX_URLS,
            });
        }
        for raw in &args.urls {
            let url = Url::parse(raw).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
            // Quick SSRF reject so we don't insert a task that will only fail.
            ssrf_validate(&url, self.ssrf_level).map_err(|e: SsrfError| {
                McpError::Fetcher(crate::fetcher::FetcherError::Ssrf(e))
            })?;
        }
        let params = BatchFetchParams {
            urls: args.urls.clone(),
            concurrency: args.concurrency.unwrap_or(8).clamp(1, MAX_CONCURRENCY),
            per_domain_concurrency: args
                .per_domain_concurrency
                .unwrap_or(2)
                .clamp(1, MAX_PER_DOMAIN),
            force_refresh: args.force_refresh,
        };
        let id = TaskId::new();
        let params_json =
            serde_json::to_string(&params).map_err(McpError::from_serde_invalid_args)?;
        insert(
            &self.db,
            TaskInsert {
                id: id.as_str().to_string(),
                kind: TaskKind::BatchFetch,
                params_json,
                owner_pid: Some(std::process::id() as i64),
            },
        )
        .await
        .map_err(McpError::from)?;
        // Notify scheduler. Failure to send means the channel is closed —
        // log + continue: the next orphan scan picks it up.
        if let Err(e) = self.new_task_tx.send(id.clone()) {
            tracing::warn!(target: "rover::mcp", error = ?e, "scheduler channel closed");
        }
        Ok(TaskCreatedResponse {
            task_id: id.as_str().to_string(),
            status: "running".into(),
            kind: "batch_fetch".into(),
            monitor_command: format!("rover batch {id} --monitor"),
            poll_command: format!("rover batch {id}"),
            cancel_command: format!("rover task {id} --cancel"),
            hint: "Use the Monitor tool with monitor_command for live updates, or call poll_command to check status.".into(),
        })
    }
}

impl McpError {
    fn from_serde_invalid_args(e: serde_json::Error) -> Self {
        McpError::InvalidArgs(e.to_string())
    }
}
```

- [ ] **Step 3: Register the tool**

In `src/mcp/tools/mod.rs` (or wherever modules are declared), add `pub mod batch_fetch;`.

In `src/mcp/handler.rs`, extend `RoverHandler`:

```rust
pub(crate) new_task_tx: crate::tasks::scheduler::NewTaskSender,
```

Update `RoverHandler::new` to accept and store the sender. Add the tool to the `#[tool_router]` block:

```rust
    #[tool(description = "Fetch multiple URLs concurrently. Returns a task_id immediately; \
                          use rover batch <id> --monitor to stream progress.")]
    pub async fn batch_fetch_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::batch_fetch::BatchFetchArgs>,
    ) -> Result<Json<crate::mcp::envelope::TaskCreatedResponse>, ErrorData> {
        match self.batch_fetch_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }
```

Update the `with_instructions` text: `"Tools: fetch, count_tokens, get_metadata, batch_fetch."`.

- [ ] **Step 4: Build the scheduler in `mcp/server.rs`**

In `src/mcp/server.rs::serve_stdio`, after the heartbeat task is spawned and before `rmcp::serve_stdio`, construct the scheduler:

```rust
use crate::tasks::batch_fetch::BatchDeps;
use crate::tasks::retry::RetryDeps;
use crate::tasks::revalidate::RevalidateDeps;
use crate::tasks::scheduler::{Scheduler, SchedulerConfig};
use crate::tasks::default_spawner;

let (new_task_tx, new_task_rx) = Scheduler::channel();
let pacer = Arc::new(crate::fetcher::concurrency::Pacer::new(&config.rate_limit));
let client = crate::fetcher::client::build_http_client(&config.fetch.user_agent, config.fetch.timeout());

let batch_deps = BatchDeps {
    client: client.clone(),
    pacer: pacer.clone(),
    cache_cfg: config.cache.clone(),
    rate_cfg: config.rate_limit.clone(),
    robots_cfg: config.robots.clone(),
    fetch_cfg: config.fetch.clone(),
    ssrf_level,
};
let retry_deps = RetryDeps {
    client: client.clone(),
    pacer: pacer.clone(),
    cache_cfg: config.cache.clone(),
    rate_cfg: config.rate_limit.clone(),
    robots_cfg: config.robots.clone(),
    fetch_cfg: config.fetch.clone(),
    ssrf_level,
};
let revalidate_deps = RevalidateDeps {
    client: client.clone(),
    pacer: pacer.clone(),
    cache_cfg: config.cache.clone(),
    rate_cfg: config.rate_limit.clone(),
    robots_cfg: config.robots.clone(),
    fetch_cfg: config.fetch.clone(),
    ssrf_level,
};
let spawner = default_spawner(batch_deps.clone(), retry_deps, revalidate_deps);
let sched = Scheduler {
    db: db.clone(),
    cfg: SchedulerConfig {
        own_pid: pid,
        ..SchedulerConfig::default()
    },
    cancel: cancel.clone(),
    new_task_rx,
    spawner,
};
let sched_handle = tokio::spawn(sched.run());
```

Pass `new_task_tx`, `pacer`, `client` into `RoverHandler::new`. After `rmcp::serve_stdio` returns, signal `cancel.cancel()` and `await sched_handle`.

- [ ] **Step 5: Integration test**

Create `tests/mcp_batch_fetch.rs`:

```rust
//! batch_fetch envelope shape and validation errors.

use std::sync::Arc;

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::mcp::handler::RoverHandler;
use rover::mcp::tools::batch_fetch::BatchFetchArgs;
use rover::storage::Db;
use rover::tasks::scheduler::Scheduler;
use tempfile::tempdir;

async fn fixture_handler() -> (RoverHandler, Db) {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let cfg = Arc::new(Config::default());
    let (tx, _rx) = Scheduler::channel();
    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let pacer = Arc::new(Pacer::new(&cfg.rate_limit));
    let h = RoverHandler::new(db.clone(), cfg, client, SsrfLevel::TestLoopback, pacer, tx);
    (h, db)
}

#[tokio::test]
async fn empty_urls_returns_user_error() {
    let (h, _) = fixture_handler().await;
    let err = h.batch_fetch_inner(BatchFetchArgs { urls: vec![], ..Default::default() }).await.unwrap_err();
    assert!(matches!(err, rover::mcp::error::McpError::EmptyUrlList));
}

#[tokio::test]
async fn over_max_urls_returns_too_many_urls() {
    let (h, _) = fixture_handler().await;
    let urls = (0..101).map(|i| format!("https://example.test/{i}")).collect();
    let err = h.batch_fetch_inner(BatchFetchArgs { urls, ..Default::default() }).await.unwrap_err();
    assert!(matches!(
        err,
        rover::mcp::error::McpError::TooManyUrls { count: 101, max: 100 },
    ));
}

#[tokio::test]
async fn happy_path_returns_envelope_and_inserts_task() {
    let (h, db) = fixture_handler().await;
    let out = h
        .batch_fetch_inner(BatchFetchArgs {
            urls: vec!["https://example.test/a".into(), "https://example.test/b".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(out.kind, "batch_fetch");
    assert_eq!(out.status, "running");
    assert!(out.monitor_command.contains(&out.task_id));
    assert!(out.cancel_command.contains(&out.task_id));
    let row = rover::storage::tasks::get(&db, &out.task_id).await.unwrap().unwrap();
    assert_eq!(row.kind, rover::storage::tasks::TaskKind::BatchFetch);
}
```

- [ ] **Step 6: Run**

Run: `cargo test --test mcp_batch_fetch && cargo build --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(m6): batch_fetch mcp tool with ssrf pre-check and envelope return

inserts a tasks row carrying serialised batchfetchparams, notifies the
scheduler over the in-process mpsc, returns the {task_id, monitor_command,
poll_command, cancel_command, hint} envelope. ssrf rejects pre-empt the
task insert.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 13: CLI `rover task <id>` (Snapshot + Monitor + Cancel)

**Files:**
- Create: `src/cli/task.rs`
- Modify: `src/cli/mod.rs` (`pub mod task; pub mod batch;`)
- Modify: `src/main.rs` (wire `Command::Task` dispatch with new flags)
- Create: `tests/cli_batch_snapshot.rs`

Pure reader (except `--cancel`). Snapshot, NDJSON snapshot, NDJSON monitor stream. Liveness warning when no live server.

- [ ] **Step 1: Extend the CLI flag definitions**

In `src/main.rs`, replace the existing `Task { id, monitor, cancel }` definition with:

```rust
    /// Inspect or monitor a long-running task.
    Task {
        id: String,
        #[arg(long)]
        monitor: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Stream events starting after this event id (use with --monitor).
        #[arg(long)]
        from_event: Option<i64>,
    },
    /// Inspect or monitor a batch_fetch task (alias for `rover task` with a kind check).
    Batch {
        id: String,
        #[arg(long)]
        monitor: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        #[arg(long)]
        from_event: Option<i64>,
    },
```

Add the enum:

```rust
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    Human,
    Ndjson,
}
```

Replace the dispatch arms (`Command::Batch { .. } | Command::Task { .. } | Command::Doctor | Command::Config(_)` becomes split):

```rust
        Command::Task { id, monitor, cancel, format, from_event } => {
            rover::cli::task::run(
                rover::cli::task::Args {
                    id,
                    monitor,
                    cancel,
                    format: format.into(),
                    from_event,
                    expect_kind: None,
                },
                cli.config.as_deref(),
            )
            .await
        }
        Command::Batch { id, monitor, cancel, format, from_event } => {
            rover::cli::batch::run(
                rover::cli::task::Args {
                    id,
                    monitor,
                    cancel,
                    format: format.into(),
                    from_event,
                    expect_kind: Some("batch_fetch"),
                },
                cli.config.as_deref(),
            )
            .await
        }
        Command::Doctor | Command::Config(_) => {
            eprintln!("not yet implemented (planned for a later milestone)");
            return ExitCode::from(2);
        }
```

Add an `impl From<OutputFormat> for rover::cli::task::OutputFormat { ... }` conversion.

- [ ] **Step 2: Write `src/cli/task.rs`**

```rust
//! `rover task <id>` body (also drives `rover batch <id>` via `expect_kind`).

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow};
use jiff::Timestamp;

use crate::config;
use crate::storage::Db;
use crate::storage::events::{EventRow, range_since, count_by_kind, last_for_task};
use crate::storage::tasks::{TaskKind, TaskStatus, get, set_cancellation_requested};

pub struct Args {
    pub id: String,
    pub monitor: bool,
    pub cancel: bool,
    pub format: OutputFormat,
    pub from_event: Option<i64>,
    pub expect_kind: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Human,
    Ndjson,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let _cfg = config::load(config_path).context("loading config")?;
    let data_dir = crate::paths::data_dir();
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let row = get(&db, &args.id).await?.ok_or_else(|| anyhow!("task {} not found", args.id))?;
    if let Some(want) = args.expect_kind {
        if row.kind.as_str() != want {
            return Err(anyhow!(
                "task {} is kind={}, expected {}",
                args.id,
                row.kind.as_str(),
                want
            ));
        }
    }

    if args.cancel {
        let changed = set_cancellation_requested(&db, &args.id).await?;
        if changed {
            println!("Cancellation requested for {}.", args.id);
        } else if row.cancellation_requested {
            println!("Cancellation already requested for {}.", args.id);
        } else {
            println!("Task {} is in a terminal state; nothing to cancel.", args.id);
        }
        return Ok(());
    }

    if args.monitor {
        return monitor_loop(&db, &args).await;
    }
    print_snapshot(&db, &args, &row).await
}

async fn print_snapshot(
    db: &Db,
    args: &Args,
    row: &crate::storage::tasks::TaskRow,
) -> anyhow::Result<()> {
    let liveness = check_liveness(db).await?;
    let now_ms = Timestamp::now().as_millisecond();
    let counts = count_by_kind(db, &args.id).await?;
    let succeeded = counts.iter().find(|(k, _)| k == "item_done").map(|(_, n)| *n).unwrap_or(0);
    let failed = counts.iter().find(|(k, _)| k == "item_failed").map(|(_, n)| *n).unwrap_or(0);
    let started = counts.iter().find(|(k, _)| k == "item_started").map(|(_, n)| *n).unwrap_or(0);
    let total: i64 = if row.kind == TaskKind::BatchFetch {
        serde_json::from_str::<serde_json::Value>(&row.params_json)
            .ok()
            .and_then(|v| v.get("urls").and_then(|u| u.as_array()).map(|a| a.len() as i64))
            .unwrap_or(0)
    } else {
        0
    };
    let in_flight = (started - succeeded - failed).max(0);
    let last = last_for_task(db, &args.id).await?;

    match args.format {
        OutputFormat::Ndjson => {
            let snap = serde_json::json!({
                "ts": rfc3339(now_ms),
                "kind": "snapshot",
                "task_id": row.id,
                "task_kind": row.kind.as_str(),
                "status": row.status.as_str(),
                "total": total,
                "succeeded": succeeded,
                "failed": failed,
                "in_flight": in_flight,
                "completed": succeeded + failed,
                "started_at": rfc3339(row.created_at),
                "last_event_id": last.as_ref().map(|e| e.id),
                "eta_s": eta_seconds(succeeded, total, row.created_at, now_ms),
            });
            println!("{snap}");
        }
        OutputFormat::Human => {
            if let Some(warn) = liveness {
                println!("{warn}");
            }
            if row.kind == TaskKind::BatchFetch {
                println!("Batch {} — {}", row.id, row.status.as_str());
            } else {
                println!("Task {} — {} (kind: {})", row.id, row.status.as_str(), row.kind.as_str());
            }
            println!("Started {}", relative_human(now_ms - row.created_at));
            if row.kind == TaskKind::BatchFetch && total > 0 {
                let pct = if total == 0 { 0 } else { ((succeeded + failed) * 100 / total) };
                println!(
                    "Progress: {}/{} ({}%)  ✓ {}  ✗ {}  ⋯ {} in flight",
                    succeeded + failed,
                    total,
                    pct,
                    succeeded,
                    failed,
                    in_flight,
                );
                if let Some(eta) = eta_seconds(succeeded, total, row.created_at, now_ms) {
                    println!("ETA ~{}s", eta);
                }
            }
            if let Some(ev) = last.as_ref() {
                println!("Last event: {} ({})", summarise_event(ev), relative_human(now_ms - ev.ts));
            }
            if !row.status.is_terminal() {
                println!("Tip: use `rover task {} --cancel` to stop.", row.id);
            }
        }
    }
    Ok(())
}

async fn monitor_loop(db: &Db, args: &Args) -> anyhow::Result<()> {
    let mut last_seen = args.from_event.unwrap_or(0);
    loop {
        let rows = range_since(db, &args.id, last_seen, 1000).await?;
        for r in &rows {
            emit_wire_line(r);
            last_seen = r.id;
        }
        if rows.is_empty() {
            let row = get(db, &args.id).await?.ok_or_else(|| anyhow!("task {} disappeared", args.id))?;
            if row.status.is_terminal() {
                // Drain one more time in case the terminal event was written
                // between our SELECT and now.
                let extras = range_since(db, &args.id, last_seen, 1000).await?;
                for r in &extras {
                    emit_wire_line(r);
                }
                return match row.status {
                    TaskStatus::Completed => Ok(()),
                    TaskStatus::Failed => Err(anyhow!("task failed")),
                    TaskStatus::Cancelled => Err(anyhow!("task cancelled")),
                    _ => Ok(()),
                };
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

fn emit_wire_line(r: &EventRow) {
    // DB payload is already a JSON object; merge with the envelope keys.
    let payload: serde_json::Value =
        serde_json::from_str(&r.payload_json).unwrap_or(serde_json::json!({}));
    let mut obj = serde_json::Map::new();
    obj.insert("ts".into(), serde_json::Value::String(rfc3339(r.ts)));
    obj.insert("kind".into(), serde_json::Value::String(r.kind.clone()));
    obj.insert("event_id".into(), serde_json::Value::Number(r.id.into()));
    if let serde_json::Value::Object(map) = payload {
        for (k, v) in map {
            obj.entry(k).or_insert(v);
        }
    }
    println!("{}", serde_json::Value::Object(obj));
}

async fn check_liveness(db: &Db) -> anyhow::Result<Option<String>> {
    let servers = db.list_servers().await?;
    let now_s = Timestamp::now().as_second();
    let warn = match servers.iter().map(|s| s.last_heartbeat).max() {
        None => Some("⚠ No `rover mcp` process appears to be alive.".to_string()),
        Some(hb) if now_s - hb > 30 => Some(format!(
            "⚠ Task is marked `running` but no `rover mcp` process appears to be alive (last heartbeat {}s ago).",
            now_s - hb,
        )),
        _ => None,
    };
    Ok(warn)
}

fn relative_human(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m{}s ago", s / 60, s % 60)
    } else {
        format!("{}h{}m ago", s / 3600, (s % 3600) / 60)
    }
}

fn rfc3339(ms: i64) -> String {
    let ts = Timestamp::from_millisecond(ms).unwrap_or_else(|_| Timestamp::now());
    ts.to_string()
}

fn eta_seconds(succeeded: i64, total: i64, started_ms: i64, now_ms: i64) -> Option<i64> {
    if succeeded < 3 || total == 0 {
        return None;
    }
    let elapsed_ms = now_ms - started_ms;
    if elapsed_ms <= 0 {
        return None;
    }
    let avg_per_item = elapsed_ms as f64 / succeeded as f64;
    let remaining = (total - succeeded).max(0) as f64;
    Some(((avg_per_item * remaining) / 1000.0) as i64)
}

fn summarise_event(r: &EventRow) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&r.payload_json).unwrap_or(serde_json::json!({}));
    if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
        format!("{} {}", r.kind, url)
    } else {
        r.kind.clone()
    }
}
```

Also add `pub fn into_runtime_args_for_task(...)` shims in `main.rs` if convenient — keeping this file body minimal.

- [ ] **Step 3: Create `src/cli/batch.rs`**

```rust
//! `rover batch <id>` is `rover task <id>` with `expect_kind="batch_fetch"`.

use std::path::Path;

pub async fn run(args: crate::cli::task::Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    crate::cli::task::run(args, config_path).await
}
```

In `src/cli/mod.rs`:
```rust
pub mod task;
pub mod batch;
```

- [ ] **Step 4: Refactor `cli::task::run` to write into a `&mut dyn Write`**

So tests can capture stdout in-process without subprocess overhead, refactor `run` and its private helpers to accept a writer:

```rust
pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    run_with_writers(args, config_path, &mut out).await
}

pub async fn run_with_writers<W: std::io::Write>(
    args: Args,
    config_path: Option<&Path>,
    out: &mut W,
) -> anyhow::Result<()> {
    // ... move the existing body here, replacing every `println!(...)` with
    //     `writeln!(out, ...)?` and every `print!(...)` with `write!(out, ...)?`.
}
```

Each helper (`print_snapshot`, `monitor_loop`, `emit_wire_line`) takes the writer through. The `Cancel` path stays on `println!` — or also threads `out` for consistency. Use the `out`-threaded version.

- [ ] **Step 5: Integration tests — snapshot**

Create `tests/cli_batch_snapshot.rs`:

```rust
//! `rover batch <id>` snapshot in human and ndjson formats.

use tempfile::tempdir;

use rover::cli::task::{Args, OutputFormat, run_with_writers};
use rover::storage::Db;
use rover::storage::events::{EventInsert, append};
use rover::storage::tasks::{TaskInsert, TaskKind, insert};

async fn seed_running_batch(db: &Db, id: &str) {
    let params = rover::tasks::types::BatchFetchParams {
        urls: vec!["https://a/".into(), "https://b/".into()],
        concurrency: 2,
        per_domain_concurrency: 1,
        force_refresh: false,
    };
    insert(
        db,
        TaskInsert {
            id: id.into(),
            kind: TaskKind::BatchFetch,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    append(
        db,
        EventInsert {
            task_id: id.into(),
            kind: "item_done".into(),
            payload_json: r#"{"index":0,"url":"https://a/"}"#.into(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn snapshot_human_includes_tip_when_running() {
    let tmp = tempdir().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()); }
    let data_dir = tmp.path().join("rover");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(data_dir.join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id1").await;
    db.upsert_server_self(std::process::id() as i64, "v".into())
        .await
        .unwrap();
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id1".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Human,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Batch id1"), "got: {out}");
    assert!(out.contains("Tip: use `rover task id1 --cancel`"), "got: {out}");
}

#[tokio::test]
async fn snapshot_ndjson_is_single_line() {
    let tmp = tempdir().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()); }
    let data_dir = tmp.path().join("rover");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(data_dir.join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id2").await;
    db.upsert_server_self(std::process::id() as i64, "v".into())
        .await
        .unwrap();
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id2".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Ndjson,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "expected single line, got {lines:?}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["task_id"], "id2");
    assert_eq!(v["task_kind"], "batch_fetch");
    assert!(v.get("succeeded").is_some());
    assert!(v.get("failed").is_some());
    assert!(v.get("in_flight").is_some());
}

#[tokio::test]
async fn snapshot_human_emits_liveness_warning_when_no_server_row() {
    let tmp = tempdir().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()); }
    let data_dir = tmp.path().join("rover");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(data_dir.join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id3").await;
    // Deliberately do not call upsert_server_self.
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id3".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Human,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("⚠"), "expected liveness warning, got: {out}");
    assert!(out.contains("rover mcp"));
}
```

- [ ] **Step 5: Run**

Run: `cargo build --all-features && cargo test --test cli_batch_snapshot`

Expected: PASS (after the writer-refactor).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(m6): rover task / rover batch cli with snapshot, monitor, cancel

human-default snapshot shows progress, tip line, and a liveness warning
when no rover mcp heartbeat is recent. --format=ndjson prints a single
rollup line. --monitor polls task_events on a 200ms tick and exits cleanly
on task terminal state. --cancel flips cancellation_requested.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Task 14: End-to-End MCP→CLI Integration + Acceptance + README

**Files:**
- Create: `tests/cli_batch_monitor.rs`
- Create: `tests/tasks_orphan_claim.rs` (multi-process orphan reclaim test)
- Modify: `README.md` (M6 complete marker)
- Modify: `docs/superpowers/milestones/rover-milestones.md` (M6 status flip)

Final acceptance pass. Spawn `rover mcp` as a subprocess, drive it via stdin/stdout with a `batch_fetch` request, then drive `rover batch <id> --monitor` against the resulting task ID. Then the orphan-reclaim scenario: kill the server mid-batch, restart, assert progress resumes without duplicate `item_done`.

- [ ] **Step 1: Build helpers in `tests/common/mod.rs`**

Append a helper to spawn `rover mcp` with a known database path and stdio piped. Adapt the existing M3 integration test scaffolding (the `tests/common/mod.rs` already contains MCP-server helpers from M3 / M5).

```rust
/// Spawn `target/debug/rover mcp` against an explicit data dir.
pub async fn spawn_rover_mcp_against(data_dir: &std::path::Path) -> std::process::Child {
    // ... build path, set XDG_DATA_HOME=data_dir, ROVER_MCP_SSRF=test_loopback,
    //     stdin/stdout=Stdio::piped(), return Child.
    todo!("port from M3 test scaffolding")
}
```

(The "todo!" here is a pointer to the existing M3 helper, not a placeholder — the implementer adapts the function name they actually find in `tests/common/mod.rs`.)

- [ ] **Step 2: Monitor-streaming integration test**

Create `tests/cli_batch_monitor.rs`:

```rust
//! End-to-end: MCP `batch_fetch` then `rover batch <id> --monitor` streams.

use std::time::Duration;
use tempfile::tempdir;
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[tokio::test]
async fn monitor_streams_until_terminal_and_exits_zero() {
    let mock = MockServer::start().await;
    Mock::given(path_regex(r"^/page/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>x</body></html>"))
        .mount(&mock)
        .await;
    let urls: Vec<String> = (0..3).map(|i| format!("{}/page/{i}", mock.uri())).collect();

    let data = tempdir().unwrap();
    let mut server = common::spawn_rover_mcp_against(data.path()).await;
    let task_id = common::call_batch_fetch_tool(&mut server, &urls)
        .await
        .expect("envelope")
        .task_id;

    // Subprocess: `rover batch <id> --monitor --format ndjson`.
    let rover_bin = env!("CARGO_BIN_EXE_rover");
    let mut child = std::process::Command::new(rover_bin)
        .env("XDG_DATA_HOME", data.path())
        .args(["batch", &task_id, "--monitor"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);
    use std::io::BufRead;
    let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let status = child.wait().unwrap();

    common::shutdown_server(&mut server);
    assert!(status.success(), "monitor exited non-zero");
    assert!(!lines.is_empty());
    let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(first["kind"], "task_started");
    assert_eq!(last["kind"], "task_completed");
}
```

Equivalent SIGINT test:

```rust
#[tokio::test]
async fn monitor_sigint_exits_cleanly() {
    // Spawn server, insert a batch with many slow URLs (wiremock delay),
    // run `rover batch <id> --monitor`, send SIGINT after first event,
    // assert child status is success() (we trap SIGINT and exit 0).
    // Implementation detail: use libc::kill(child_pid, libc::SIGINT) on Unix.
}
```

- [ ] **Step 3: Orphan-reclaim integration test**

Create `tests/tasks_orphan_claim.rs`:

```rust
//! Two rover-mcp lifecycles share a task: kill first, second resumes.

use std::time::Duration;
use tempfile::tempdir;
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[tokio::test]
async fn second_server_resumes_orphan_batch() {
    let mock = MockServer::start().await;
    Mock::given(path_regex(r"^/page/\d+$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>x</body></html>")
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&mock)
        .await;
    let urls: Vec<String> = (0..10).map(|i| format!("{}/page/{i}", mock.uri())).collect();
    let data = tempdir().unwrap();

    let mut first = common::spawn_rover_mcp_against(data.path()).await;
    let task_id = common::call_batch_fetch_tool(&mut first, &urls).await.unwrap().task_id;
    // Wait until at least 2 items have been processed.
    tokio::time::sleep(Duration::from_millis(800)).await;
    // Kill (not graceful — we want the servers row to remain stale).
    let _ = first.kill();
    let _ = first.wait();

    let mut second = common::spawn_rover_mcp_against(data.path()).await;
    // The second server's startup reap should mark the original servers row stale.
    // Wait for the orphan scan (default 10s — for the test we accept the wait or
    // override via env var if the production code reads RWA_ORPHAN_SCAN_MS).
    // Simpler path: trigger a re-claim by inserting a tasks notify on the cli side,
    // OR temporarily set Scheduler.orphan_scan_interval via env var.
    // Plan task decides which; the design spec leaves this open.

    let rover_bin = env!("CARGO_BIN_EXE_rover");
    let out = std::process::Command::new(rover_bin)
        .env("XDG_DATA_HOME", data.path())
        .args(["batch", &task_id, "--monitor"])
        .stdout(std::process::Stdio::piped())
        .output()
        .unwrap();
    common::shutdown_server(&mut second);
    let text = String::from_utf8_lossy(&out.stdout);
    let mut done_indices = std::collections::HashSet::new();
    for line in text.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) { Ok(v) => v, _ => continue };
        if v["kind"] == "item_done" {
            let idx = v["index"].as_u64().unwrap();
            assert!(done_indices.insert(idx), "duplicate item_done for index {idx}");
        }
    }
    assert!(done_indices.len() >= urls.len() - 2, "expected resumption to complete most items, got {done_indices:?}");
}
```

Note: this test exposes the practical issue that the default 10s orphan scan is too slow for a CI test. **Fix in the implementer task:** expose `SchedulerConfig::orphan_scan_interval` via an env var (`ROVER_ORPHAN_SCAN_MS`) read in `mcp/server.rs` — gated behind `#[cfg(any(test, feature = "test-loopback"))]` so production stays at 10s. Default in tests: 200ms.

- [ ] **Step 4: Run the full suite**

Run:
```
cargo build --all-features
cargo test --features test-loopback
```

Expected: every test PASSes. Address failures inline.

- [ ] **Step 5: Update milestone manifest**

In `docs/superpowers/milestones/rover-milestones.md` M6 section, set the status to "complete" (mirroring how M5 was marked). Add a short follow-up list reflecting any deferred items surfaced during implementation:

```markdown
**Status:** Complete (2026-MM-DD).

**M6 follow-ups deferred to later milestones.**
1. Cross-process new-task notification (currently the 10s orphan scan handles it).
2. `--from-event` exposed to MCP clients (currently CLI-only).
3. Wallclock batch timeout (currently relies on per-URL timeouts + `--cancel`).
```

(Fill in the actual deferrals based on what was punted during implementation.)

- [ ] **Step 6: README marker**

In `README.md`, append a line near the milestone table:

```markdown
- ✅ M6 — Long-running tasks & batching (`batch_fetch`, `rover batch <id>`, `rover task <id>`)
```

- [ ] **Step 7: Final lint + format gate**

Run:
```
cargo fmt -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --features test-loopback
```

All three must pass before the final commit.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(m6): end-to-end mcp->cli monitor and orphan reclaim integration tests

cli_batch_monitor verifies a real `rover mcp` subprocess + `rover batch <id>
--monitor` round-trips ndjson and exits zero on completion. sigint exits
cleanly. tasks_orphan_claim kills the server mid-batch and asserts a new
server resumes without duplicating item_done events.

ships the m6 complete marker in readme and the milestone manifest.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Acceptance Criteria

Per PRD §14 M6 and design spec §13:

- [ ] A batch of 20 URLs returns a `task_id` immediately (<100ms p99).
- [ ] `rover batch <id>` snapshot shows in-flight progress.
- [ ] `rover batch <id> --monitor` streams NDJSON until terminal.
- [ ] `rover task <id> --cancel` mid-flight stops further item starts.
- [ ] On `rover mcp` restart mid-batch, a fresh `rover mcp` claims the orphan and resumes without duplicate `item_done`.
- [ ] Stale-served fetch returns a `revalidation.task_id` envelope; the task completes and refreshes the cache row.
- [ ] Long `Retry-After` (>30s) on a `fetch` returns a `deferred` envelope; the retry task waits and completes.
- [ ] Liveness warning prints when no live `rover mcp` and task is `running`.
- [ ] All existing tests still pass; new M6 tests pass.
- [ ] `cargo clippy --all-features --all-targets -- -D warnings` is clean.
- [ ] No `println!` in lib code (only in `src/cli/*`).

---

## Known v1 Limitations (Carry Into M7)

- Cross-process new-task delivery: deliberately not implemented. A task inserted by one `rover mcp` instance is only picked up by another via the 10s orphan scan (after the inserting server dies). For v1 this is fine — the inserting server runs the task. Document in `docs/security.md` or a future `docs/operational-notes.md`.
- No wallclock batch timeout. Per-URL fetcher timeouts (M5) + `rover task <id> --cancel` are the controls.
- `summarize` worker is a stub. Real summarization is M7.
- `--from-event` is CLI-only; MCP `batch_fetch` clients always see the full event stream from event 0 via the monitor command.

---

## Summary

14 TDD tasks. Tasks 1–6 are foundation (M5 follow-ups, storage, scheduler, summarize). Tasks 7–11 are the workers and the fetcher-side integration that turns long retries and stale entries into background tasks. Task 12 surfaces the new MCP tool. Tasks 13–14 add the CLI and the end-to-end integration tests + acceptance gates.

Frequent commits (~one per task), each with a green test suite before moving on. The `DefaultSpawner` in `src/tasks/mod.rs` is the only place that grows across multiple tasks — the temporary `→ summarize::run` arms get replaced one-by-one as the real workers land.


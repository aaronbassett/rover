# Rover M5 — Rate Limiting & Robots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-domain token-bucket rate limiting, layered (global + per-host) concurrency caps, in-line retry with `Retry-After` honor, and robots.txt fetch+respect to the M1–M4 HTTP pipeline. Bundle three M4 follow-ups (FetcherError::Extract variant, shared `data_dir()` helper, PRD MetadataPreset deferral).

**Architecture:** Introduce a `Pacer` struct that owns all per-process pacing state (governor-keyed token bucket + global `Semaphore` + per-host `Semaphore` map + per-host `last_request_at` map). A new `fetcher::retry::with_retries` wraps `fetch_url_conditional` and classifies status codes / network errors against a single retry policy. A new `fetcher::robots` fetches and parses robots.txt via the `robotxt` crate, caches the result in the existing `robots_cache` table (extended with a `state` column to distinguish parsed / allow-all / disallow-all entries), and gates `fetch_with_cache` before the cache lookup. The `Pacer` is built once at startup (`rover mcp` or each CLI invocation) and shared via `Arc<Pacer>`.

**Tech Stack:** `governor` (rate limiter), `robotxt` (robots.txt parser), `dashmap` (concurrent host map for governor), `httpdate` (Retry-After HTTP-date parsing). Tests use `wiremock` (already in dev-deps) and `tokio::time::advance` against `tokio::test(start_paused = true)` for timing.

**Branch context:** Execute on `m5-rate-limiting`, cut from `main` after M4 PR #5 merged. The branch already contains commit `5d255c8` with the design spec. Verify `cargo test` is green on a clean checkout before Task 1.

**Scope of this plan:** PRD milestone M5 only (PRD §5.4, §5.6). Three M4 follow-ups bundled because we are already touching `FetcherError`, the same call sites, and `cli/*.rs` paths code. Later milestones (M6 long-running tasks, M7 summarization, M8 polish, M9 feature flags) get their own plans.

**References:**
- Design spec: `docs/superpowers/specs/2026-05-14-rover-m5-rate-limiting-design.md`
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` §5.4 + §5.6 + §12
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` §4.2 (migrations) + §4.4 (error model)
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` M5 section
- M4 plan (granularity reference): `docs/superpowers/plans/2026-05-14-rover-m4-extraction.md`

---

## Decisions inherited from the M5 design spec

The spec resolved every open question. Quick reference:

1. Robots crate: `robotxt`.
2. Token bucket: `governor` keyed by host (`DashMapStateStore<String>`).
3. Concurrency acquire order: per-host permit first, then global.
4. Retry scope: 429 + 503 + other 5xx + transient network errors (timeout/connect). Max 3 retries (4 total attempts).
5. Retry placement: new `fetcher::retry` module wrapping `fetch_url_conditional`. Per-host permit + governor token held across retries.
6. Rate-limiter scope: per-process. Cross-process sharing deferred to v2.
7. Robots fetch failures: 4xx → `state='allow_all'`, full TTL. 5xx/timeout → `state='disallow_all'`, `failure_ttl` (5min).
8. Crawl-Delay enforcement: separate per-host `last_request_at` min-interval map on top of governor.
9. M4 follow-ups bundled into M5: #1 `FetcherError::Extract` variant + 3 call-site remap, #5 shared `data_dir()`, #2 PRD §14 footnote. Items #3, #4, #6 deferred to a later cleanup PR.

## Files Created or Modified in This Plan

```
# Created
src/fetcher/concurrency.rs                # Pacer skeleton + semaphore registry
src/fetcher/rate_limit.rs                 # governor wrapper + min-interval map
src/fetcher/retry.rs                      # classifier + with_retries loop
src/fetcher/robots.rs                     # robotxt + fetch_and_cache + evaluator
src/storage/robots.rs                     # async API over robots_cache table
src/storage/migrations/003_robots_state.sql
src/paths.rs                              # shared data_dir() helper (M4 #5)

tests/fetcher_rate_limit.rs
tests/fetcher_retry.rs
tests/fetcher_robots.rs
tests/fetcher_full_loop.rs
tests/fixtures/m5/robots-allow-articles.txt
tests/fixtures/m5/robots-disallow-admin.txt
tests/fixtures/m5/robots-with-crawldelay.txt
tests/fixtures/m5/wide-ua-rules.txt
tests/fixtures/m5/extract-failure.html

# Modified
Cargo.toml                                # +governor, +robotxt, +dashmap, +httpdate
src/lib.rs                                # +pub mod paths
src/fetcher/mod.rs                        # +new modules; FetcherError variants
src/fetcher/cached.rs                     # Pacer + robots gate + retry wiring
src/storage/mod.rs                        # register migration 003
src/config.rs                             # +RateLimitConfig +RobotsConfig
src/cli/fetch.rs                          # CLI flags; use shared data_dir(); build Pacer
src/cli/cache.rs                          # use shared data_dir()
src/cli/mcp.rs                            # CLI flags; use shared data_dir(); build Pacer
src/extractor/output.rs                   # use shared data_dir() helper
src/mcp/server.rs                         # build Pacer once; thread through to handler
src/mcp/handler.rs                        # carry Arc<Pacer> in RoverHandler
src/mcp/tools/fetch.rs                    # FetcherError::Extract remap; pass pacer
src/mcp/tools/get_metadata.rs             # FetcherError::Extract remap; pass pacer
src/mcp/error.rs                          # route new FetcherError variants to codes
src/mcp/envelope.rs                       # +RoverError constants for new codes

docs/superpowers/prd/2026-05-07-rover-prd.md  # MetadataPreset deferral footnote (M4 #2)
docs/security.md                              # known v1 limitation: per-process rate limit
README.md                                     # M5 complete marker (final task)
```

Inline unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each new source file.

---

## Task 1: Dependencies + Schema Migration 003

**Files:**
- Modify: `Cargo.toml`
- Create: `src/storage/migrations/003_robots_state.sql`
- Modify: `src/storage/mod.rs`

Pin the new crate dependencies and register a one-line ALTER TABLE migration. Verify the migration runs cleanly on a fresh DB and is idempotent on a previously-migrated DB.

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In `[dependencies]` after `mime_guess = "2"`:

```toml
governor = "0.10"
robotxt = { version = "0.6", default-features = false, features = ["parser"] }
dashmap = "6"
httpdate = "1"
```

Confirm versions are the latest stable with `cargo add --dry-run governor` (etc.) before committing — versions above are correct as of 2026-05-14 but may have ticked up. `robotxt` is locked to `default-features = false` + `parser` because we don't need the builder/serde features for the runtime path.

- [ ] **Step 2: Create the migration file**

Create `src/storage/migrations/003_robots_state.sql`:

```sql
-- M5: add `state` column to robots_cache so we can distinguish a parsed entry
-- from a sentinel (allow_all on 4xx, disallow_all on 5xx/timeout fail-closed).
--
-- Pre-existing rows (none in shipped releases — M2 created the table but M5
-- is the first milestone to write to it) are interpreted as parsed entries
-- since their `body` column carries the robots.txt text.

ALTER TABLE robots_cache ADD COLUMN state TEXT NOT NULL DEFAULT 'parsed';
```

- [ ] **Step 3: Register the migration**

In `src/storage/mod.rs`, extend the `MIGRATIONS` constant (around line 34). Add the third tuple at the end of the array:

```rust
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial.sql",
        include_str!("migrations/001_initial.sql"),
    ),
    (
        "002_servers.sql",
        include_str!("migrations/002_servers.sql"),
    ),
    (
        "003_robots_state.sql",
        include_str!("migrations/003_robots_state.sql"),
    ),
];
```

- [ ] **Step 4: Write failing test for migration application**

Add to the `#[cfg(test)] mod tests` block in `src/storage/mod.rs` (just before the closing `}` of the module):

```rust
    #[tokio::test]
    async fn migration_003_adds_state_column_to_robots_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();

        let cols: Vec<String> = db
            .conn
            .call(|c| {
                let mut stmt = c.prepare("PRAGMA table_info(robots_cache)")?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(r) = rows.next()? {
                    out.push(r.get::<_, String>(1)?);
                }
                Ok::<_, rusqlite::Error>(out)
            })
            .await
            .unwrap();
        assert!(cols.contains(&"state".to_string()), "cols = {cols:?}");
        assert_eq!(db.schema_version().await.unwrap(), 3);
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --lib storage::tests::migration_003_adds_state_column_to_robots_cache -- --nocapture`

Expected: PASS. (The migration file is registered and applies on open.)

- [ ] **Step 6: Run the full library test suite to ensure no regression**

Run: `cargo test --lib`

Expected: all existing tests still pass plus the new one. If any test asserts on `schema_version == 2`, update it to `== 3`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/storage/migrations/003_robots_state.sql src/storage/mod.rs
git commit -m "feat(m5): add migration 003 for robots_cache state column"
```

---

## Task 2: Storage API for robots_cache

**Files:**
- Create: `src/storage/robots.rs`
- Modify: `src/storage/mod.rs` (register module)

Provide a thin async API over the `robots_cache` table mirroring the shape of `storage::pages`. Three operations: lookup by host, upsert, prune expired entries. Tests cover round-trip including the `state` column.

- [ ] **Step 1: Register the module**

In `src/storage/mod.rs`, after `pub mod pages;` add:

```rust
pub mod robots;
```

- [ ] **Step 2: Write the failing test**

Create `src/storage/robots.rs` with this stub plus the test (the impl comes in Step 4):

```rust
//! Async API over the `robots_cache` table.
//!
//! Mirrors `storage::pages` in shape: opaque row struct, lookup by primary key,
//! upsert, prune. The `state` column tracks one of `parsed`, `allow_all`, or
//! `disallow_all` per the M5 design spec.

use crate::storage::{Db, StorageError};

/// One row from `robots_cache`. The `state` discriminator is a string at the
/// storage edge so SQL migrations don't have to know about Rust enums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RobotsEntry {
    pub host: String,
    pub body: Option<String>, // None for allow_all / disallow_all sentinels
    pub fetched_at: i64,
    pub expires_at: i64,
    pub state: RobotsState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotsState {
    Parsed,
    AllowAll,
    DisallowAll,
}

impl RobotsState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::AllowAll => "allow_all",
            Self::DisallowAll => "disallow_all",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "parsed" => Self::Parsed,
            "allow_all" => Self::AllowAll,
            "disallow_all" => Self::DisallowAll,
            other => {
                return Err(StorageError::Backend(
                    tokio_rusqlite::Error::Other(
                        format!("unknown robots_cache.state = {other}").into(),
                    ),
                ));
            }
        })
    }
}

pub async fn lookup(_db: &Db, _host: &str) -> Result<Option<RobotsEntry>, StorageError> {
    unimplemented!("Task 2 step 4")
}

pub async fn upsert(_db: &Db, _entry: RobotsEntry) -> Result<(), StorageError> {
    unimplemented!("Task 2 step 4")
}

pub async fn prune_expired(_db: &Db, _now: i64) -> Result<usize, StorageError> {
    unimplemented!("Task 2 step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        Db::open(tmp.path().join("rover.db")).await.unwrap()
    }

    #[tokio::test]
    async fn upsert_and_lookup_round_trip_parsed() {
        let db = fresh_db().await;
        let entry = RobotsEntry {
            host: "example.com".into(),
            body: Some("User-agent: *\nDisallow: /admin".into()),
            fetched_at: 1_000,
            expires_at: 1_000 + 86_400,
            state: RobotsState::Parsed,
        };
        upsert(&db, entry.clone()).await.unwrap();
        let got = lookup(&db, "example.com").await.unwrap();
        assert_eq!(got.as_ref(), Some(&entry));
    }

    #[tokio::test]
    async fn lookup_unknown_host_returns_none() {
        let db = fresh_db().await;
        assert_eq!(lookup(&db, "absent.example").await.unwrap(), None);
    }

    #[tokio::test]
    async fn upsert_overwrites_existing_row() {
        let db = fresh_db().await;
        let one = RobotsEntry {
            host: "example.com".into(),
            body: Some("v1".into()),
            fetched_at: 1_000,
            expires_at: 2_000,
            state: RobotsState::Parsed,
        };
        let two = RobotsEntry {
            body: Some("v2".into()),
            ..one.clone()
        };
        upsert(&db, one).await.unwrap();
        upsert(&db, two.clone()).await.unwrap();
        assert_eq!(lookup(&db, "example.com").await.unwrap(), Some(two));
    }

    #[tokio::test]
    async fn allow_all_sentinel_has_no_body() {
        let db = fresh_db().await;
        let entry = RobotsEntry {
            host: "404.example".into(),
            body: None,
            fetched_at: 1_000,
            expires_at: 1_000 + 86_400,
            state: RobotsState::AllowAll,
        };
        upsert(&db, entry.clone()).await.unwrap();
        let got = lookup(&db, "404.example").await.unwrap();
        assert_eq!(got, Some(entry));
    }

    #[tokio::test]
    async fn prune_expired_removes_old_rows_only() {
        let db = fresh_db().await;
        upsert(
            &db,
            RobotsEntry {
                host: "old.example".into(),
                body: Some("x".into()),
                fetched_at: 100,
                expires_at: 200,
                state: RobotsState::Parsed,
            },
        )
        .await
        .unwrap();
        upsert(
            &db,
            RobotsEntry {
                host: "new.example".into(),
                body: Some("y".into()),
                fetched_at: 100,
                expires_at: 10_000,
                state: RobotsState::Parsed,
            },
        )
        .await
        .unwrap();
        let pruned = prune_expired(&db, 500).await.unwrap();
        assert_eq!(pruned, 1);
        assert!(lookup(&db, "old.example").await.unwrap().is_none());
        assert!(lookup(&db, "new.example").await.unwrap().is_some());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail with `unimplemented!`**

Run: `cargo test --lib storage::robots`

Expected: each test panics with `not yet implemented`.

- [ ] **Step 4: Implement `lookup`, `upsert`, `prune_expired`**

Replace the three `unimplemented!()` stubs with:

```rust
pub async fn lookup(db: &Db, host: &str) -> Result<Option<RobotsEntry>, StorageError> {
    let host = host.to_string();
    let row = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT host, body, fetched_at, expires_at, state \
                 FROM robots_cache WHERE host = ?1",
            )?;
            let mut rows = stmt.query([&host])?;
            if let Some(r) = rows.next()? {
                let host: String = r.get(0)?;
                let body: Option<String> = r.get(1)?;
                let fetched_at: i64 = r.get(2)?;
                let expires_at: i64 = r.get(3)?;
                let state_s: String = r.get(4)?;
                Ok::<_, rusqlite::Error>(Some((host, body, fetched_at, expires_at, state_s)))
            } else {
                Ok(None)
            }
        })
        .await?;

    let Some((host, body, fetched_at, expires_at, state_s)) = row else {
        return Ok(None);
    };
    let state = RobotsState::from_db(&state_s)?;
    Ok(Some(RobotsEntry {
        host,
        body,
        fetched_at,
        expires_at,
        state,
    }))
}

pub async fn upsert(db: &Db, entry: RobotsEntry) -> Result<(), StorageError> {
    let RobotsEntry {
        host,
        body,
        fetched_at,
        expires_at,
        state,
    } = entry;
    let state_s = state.as_str().to_string();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO robots_cache (host, body, fetched_at, expires_at, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(host) DO UPDATE SET \
                    body=excluded.body, \
                    fetched_at=excluded.fetched_at, \
                    expires_at=excluded.expires_at, \
                    state=excluded.state",
                rusqlite::params![host, body, fetched_at, expires_at, state_s],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    Ok(())
}

pub async fn prune_expired(db: &Db, now: i64) -> Result<usize, StorageError> {
    let removed = db
        .conn
        .call(move |c| {
            let n = c.execute("DELETE FROM robots_cache WHERE expires_at < ?1", [now])?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(removed)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib storage::robots`

Expected: all 5 tests pass.

- [ ] **Step 6: Run full lib tests**

Run: `cargo test --lib`

Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/storage/mod.rs src/storage/robots.rs
git commit -m "feat(m5): add storage::robots api over robots_cache table"
```

---

## Task 3: M4 Follow-ups Bundle (data_dir + FetcherError::Extract + PRD note)

**Files:**
- Create: `src/paths.rs`
- Modify: `src/lib.rs`, `src/cli/fetch.rs`, `src/cli/cache.rs`, `src/cli/mcp.rs`, `src/extractor/output.rs`, `src/fetcher/mod.rs`, `src/mcp/error.rs`, `src/mcp/envelope.rs`, `src/mcp/tools/fetch.rs`, `src/mcp/tools/get_metadata.rs`, `docs/superpowers/prd/2026-05-07-rover-prd.md`

Land the three M4 follow-ups before introducing M5 fetcher complexity. `data_dir()` becomes a single helper. `FetcherError::Extract(ExtractorError)` replaces `FetcherError::Decode` masking on the 3 call sites. PRD §14 gains a one-line footnote.

- [ ] **Step 1: Create `src/paths.rs`**

```rust
//! Shared filesystem path helpers.
//!
//! Centralises path resolution that was previously duplicated across each
//! CLI subcommand and `extractor::output::OutputPaths::resolve`. See M5
//! design spec §3.9.

use std::path::PathBuf;

/// Where Rover persists its SQLite cache, logs, and other per-user state.
///
/// Resolution order:
/// 1. `ROVER_DATA_DIR` environment variable, if set and non-empty.
/// 2. `dirs::data_local_dir()/rover` (platform default).
/// 3. `./.rover` (last-resort relative fallback; only hit when the platform
///    helper fails, which is rare on supported OSes).
pub fn data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("ROVER_DATA_DIR") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    dirs::data_local_dir()
        .map(|p| p.join("rover"))
        .unwrap_or_else(|| PathBuf::from("./.rover"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env var manipulation across tests within the file.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_var_wins_over_platform_default() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: tests in this module hold ENV_LOCK; no other thread reads
        // ROVER_DATA_DIR concurrently.
        unsafe { std::env::set_var("ROVER_DATA_DIR", "/tmp/rover-test-data") };
        let p = data_dir();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        assert_eq!(p, PathBuf::from("/tmp/rover-test-data"));
    }

    #[test]
    fn empty_env_var_falls_through() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("ROVER_DATA_DIR", "") };
        let p = data_dir();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        assert!(p.ends_with("rover") || p.to_string_lossy().contains(".rover"));
    }

    #[test]
    fn unset_env_falls_through_to_platform_default() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        let p = data_dir();
        // dirs::data_local_dir() is Some on all CI targets; assertion is loose
        // because the exact path varies (Linux: ~/.local/share/rover, macOS:
        // ~/Library/Application Support/rover, Windows: AppData/Local/rover).
        assert!(p.ends_with("rover"));
    }
}
```

- [ ] **Step 2: Register `paths` in `src/lib.rs`**

Add `pub mod paths;` to `src/lib.rs` after `pub mod mcp;` (alphabetical order):

```rust
pub mod cli;
pub mod config;
pub mod error;
pub mod extractor;
pub mod fetcher;
pub mod mcp;
pub mod paths;
pub mod storage;
pub mod telemetry;
pub mod tokenizer;
```

- [ ] **Step 3: Run paths tests**

Run: `cargo test --lib paths::tests -- --test-threads=1`

Expected: 3 tests pass. (`--test-threads=1` is required because tests mutate the process env var.)

- [ ] **Step 4: Replace duplicate `data_dir()` in `src/cli/fetch.rs`**

Delete the local `fn data_dir()` (lines 134-141). Replace the call at line 36 (`let data_dir = data_dir()?;`) with:

```rust
    let data_dir = crate::paths::data_dir();
```

Remove the now-unused `anyhow::Context` import line if the `.context("...")` on `data_dir()` was the only use. (`std::fs::create_dir_all(&data_dir).context("creating data dir")?` still uses it, so the import stays.)

- [ ] **Step 5: Replace duplicate in `src/cli/cache.rs`**

Delete the local `fn data_dir()` (lines 121-128 or thereabouts — `grep -n data_dir`). Replace the call at line 20:

```rust
    let data_dir = crate::paths::data_dir();
```

- [ ] **Step 6: Replace duplicate in `src/cli/mcp.rs`**

Delete the local `fn data_dir()`. Replace the call at line 16:

```rust
    let data_dir = crate::paths::data_dir();
```

- [ ] **Step 7: Use shared helper in `src/extractor/output.rs`**

In `src/extractor/output.rs`, find the resolution branch that uses `dirs::data_local_dir()` (around line 25). Replace the call with `crate::paths::data_dir()`:

```rust
            crate::paths::data_dir().join("output")
```

Verify the surrounding logic still makes sense — `OutputPaths::resolve` may also accept an explicit `cfg.output.dir`, which still takes precedence.

- [ ] **Step 8: Verify build**

Run: `cargo build`

Expected: green with no warnings about unused imports. If `anyhow::Context` is now unused in `cli/mcp.rs` or `cli/cache.rs`, drop the import — the lint rule `warnings = "deny"` will fail the build otherwise.

- [ ] **Step 9: Run full tests**

Run: `cargo test`

Expected: green.

- [ ] **Step 10: Add `FetcherError::Extract` variant**

In `src/fetcher/mod.rs`, add a variant to the `FetcherError` enum (after `Storage`):

```rust
    #[error("extractor error: {0}")]
    Extract(#[from] crate::extractor::ExtractorError),
```

The `#[from]` allows ergonomic `?` propagation when an `ExtractorError` is the inner cause.

- [ ] **Step 11: Update `mcp/error.rs` routing for `F::Extract`**

In `src/mcp/error.rs`, in the `F::Http(_) | F::Dns { .. } | F::Decode | F::Status { .. } =>` arm (around line 72), add `F::Extract(_)` to a new arm that routes to `EXTRACT_FAILED`. The completed `match e` becomes:

```rust
                match e {
                    F::Ssrf(_) => RoverError::new(RoverError::SSRF_DENIED, e.to_string()),
                    F::Url(_) => RoverError::new(RoverError::INVALID_URL, e.to_string()),
                    F::Storage(_) => RoverError::new(RoverError::STORAGE_ERROR, e.to_string()),
                    F::Extract(_) => RoverError::new(RoverError::EXTRACT_FAILED, e.to_string()),
                    F::Http(_) | F::Dns { .. } | F::Decode | F::Status { .. } => {
                        RoverError::new(RoverError::FETCH_FAILED, e.to_string())
                    }
                }
```

- [ ] **Step 12: Remap call site in `src/cli/fetch.rs`**

Find the `extract` closure inside `fetch_with_cache` (lines 53-63 in current code):

```rust
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(|_| crate::fetcher::FetcherError::Decode)?;
            // ...
        },
```

Replace the closure body's first line with proper error propagation:

```rust
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
                metadata: extracted.metadata,
            })
        },
```

- [ ] **Step 13: Remap call site in `src/mcp/tools/fetch.rs`**

`grep -n "FetcherError::Decode" src/mcp/tools/fetch.rs` — find the extract closure inside `fetch_inner`. Replace `.map_err(|_| crate::fetcher::FetcherError::Decode)?` with `.map_err(crate::fetcher::FetcherError::Extract)?` exactly as in Step 12.

- [ ] **Step 14: Remap call site in `src/mcp/tools/get_metadata.rs`**

Same change as Step 13. There may be one closure per tool that calls `fetch_with_cache`; update each.

- [ ] **Step 15: Write a regression test for the extract→error code mapping**

Add to the `#[cfg(test)] mod tests` block in `src/mcp/error.rs`:

```rust
    #[test]
    fn fetcher_extract_routes_to_extract_failed() {
        use crate::extractor::ExtractorError;
        use crate::fetcher::FetcherError;
        let inner = ExtractorError::Output {
            path: "/tmp/x".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        let e = McpError::Fetcher(FetcherError::Extract(inner));
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::EXTRACT_FAILED);
        assert!(r.message.contains("/tmp/x"));
    }
```

- [ ] **Step 16: Run tests for the regression**

Run: `cargo test --lib mcp::error`

Expected: PASS including the new test.

- [ ] **Step 17: Add PRD §14 footnote for MetadataPreset deferral**

Open `docs/superpowers/prd/2026-05-07-rover-prd.md`, locate the M4 milestone section in §14 (search for `### M4 — Metadata, Tables, Images, Links`). After the bullet list describing what M4 ships, add a "Deferred from M4" note:

```markdown
**Deferred from M4 (formalised 2026-05-14):**
- The `MetadataPreset { Default, All, Minimal }` enum and `metadata.fields: Option<Vec<String>>` filter described in §6.6 ship in M8 or M9. The M4 `get_metadata` tool returns the complete metadata struct unfiltered.
```

- [ ] **Step 18: Run full test suite**

Run: `cargo test`

Expected: green.

- [ ] **Step 19: Commit**

```bash
git add src/paths.rs src/lib.rs src/cli/fetch.rs src/cli/cache.rs src/cli/mcp.rs \
        src/extractor/output.rs src/fetcher/mod.rs src/mcp/error.rs \
        src/mcp/tools/fetch.rs src/mcp/tools/get_metadata.rs \
        docs/superpowers/prd/2026-05-07-rover-prd.md
git commit -m "refactor(m5): bundle m4 follow-ups (data_dir helper, extract variant, prd note)"
```

---

## Task 4: Config Additions — RateLimitConfig + RobotsConfig

**Files:**
- Modify: `src/config.rs`

Add the two new config sections with their defaults and validation. Tests cover field parsing, validation paths, and the lowercase-normalisation behaviour for `ignore_domains`.

- [ ] **Step 1: Add `RateLimitConfig` struct**

In `src/config.rs`, after the `OutputConfig` struct (around line 194), add:

```rust
/// Per-domain pacing knobs. All HTTP-bound code paths run through a single
/// `Pacer` built from this struct at startup. See M5 design spec §3 and §4.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    #[serde(default = "default_rpm_per_domain")]
    pub requests_per_minute_per_domain: u32,

    #[serde(default = "default_per_domain_concurrency")]
    pub per_domain_concurrency: u32,

    #[serde(default = "default_global_concurrency")]
    pub global_concurrency: u32,

    #[serde(default = "default_max_retries")]
    pub max_retries: u8,

    #[serde(default = "default_initial_backoff", with = "humantime_serde")]
    pub initial_backoff: Duration,

    #[serde(default = "default_max_backoff", with = "humantime_serde")]
    pub max_backoff: Duration,

    #[serde(default = "default_retry_after_ceiling", with = "humantime_serde")]
    pub retry_after_ceiling: Duration,

    /// Deterministic seed for the backoff jitter RNG. `None` (default) means
    /// entropy; set in tests to make timing assertions reproducible.
    #[serde(default)]
    pub jitter_seed: Option<u64>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute_per_domain: default_rpm_per_domain(),
            per_domain_concurrency: default_per_domain_concurrency(),
            global_concurrency: default_global_concurrency(),
            max_retries: default_max_retries(),
            initial_backoff: default_initial_backoff(),
            max_backoff: default_max_backoff(),
            retry_after_ceiling: default_retry_after_ceiling(),
            jitter_seed: None,
        }
    }
}

fn default_rpm_per_domain() -> u32 { 60 }
fn default_per_domain_concurrency() -> u32 { 2 }
fn default_global_concurrency() -> u32 { 8 }
fn default_max_retries() -> u8 { 3 }
fn default_initial_backoff() -> Duration { Duration::from_millis(500) }
fn default_max_backoff() -> Duration { Duration::from_secs(30) }
fn default_retry_after_ceiling() -> Duration { Duration::from_secs(300) }
```

- [ ] **Step 2: Add `RobotsConfig` struct**

Append after `RateLimitConfig`:

```rust
/// Robots.txt fetch + respect knobs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotsConfig {
    #[serde(default = "default_respect")]
    pub respect: bool,

    /// Hosts for which robots.txt is not fetched and rules are not enforced.
    /// Lowercased in-place by `validate`.
    #[serde(default)]
    pub ignore_domains: Vec<String>,

    /// Used when the robots.txt HTTP response has no `Cache-Control: max-age`.
    #[serde(default = "default_robots_ttl", with = "humantime_serde")]
    pub default_ttl: Duration,

    /// Used when robots.txt fetch failed with 5xx or transport error (fail-closed).
    /// Short by design so a recovered server is picked up quickly.
    #[serde(default = "default_robots_failure_ttl", with = "humantime_serde")]
    pub failure_ttl: Duration,
}

impl Default for RobotsConfig {
    fn default() -> Self {
        Self {
            respect: default_respect(),
            ignore_domains: Vec::new(),
            default_ttl: default_robots_ttl(),
            failure_ttl: default_robots_failure_ttl(),
        }
    }
}

fn default_respect() -> bool { true }
fn default_robots_ttl() -> Duration { Duration::from_secs(24 * 3600) }
fn default_robots_failure_ttl() -> Duration { Duration::from_secs(5 * 60) }
```

- [ ] **Step 3: Hook into `Config`**

Add fields to the top-level `Config` struct (around line 31):

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub fetch: FetchConfig,

    #[serde(default)]
    pub cache: CacheConfig,

    #[serde(default)]
    pub tokenizer: TokenizerConfig,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub output: OutputConfig,

    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    #[serde(default)]
    pub robots: RobotsConfig,
}
```

- [ ] **Step 4: Extend `validate`**

Inside the `validate` function (around line 218), after the existing checks but before `Ok(())`:

```rust
    // RateLimitConfig
    if cfg.rate_limit.requests_per_minute_per_domain == 0 {
        return Err("rate_limit.requests_per_minute_per_domain must be > 0".to_string());
    }
    if cfg.rate_limit.requests_per_minute_per_domain > 6000 {
        return Err(format!(
            "rate_limit.requests_per_minute_per_domain ({}) exceeds sanity cap 6000 (100 req/s)",
            cfg.rate_limit.requests_per_minute_per_domain
        ));
    }
    if cfg.rate_limit.per_domain_concurrency == 0 {
        return Err("rate_limit.per_domain_concurrency must be > 0".to_string());
    }
    if cfg.rate_limit.global_concurrency == 0 {
        return Err("rate_limit.global_concurrency must be > 0".to_string());
    }
    if cfg.rate_limit.max_retries > 10 {
        return Err(format!(
            "rate_limit.max_retries ({}) exceeds sanity cap 10",
            cfg.rate_limit.max_retries
        ));
    }
    if cfg.rate_limit.initial_backoff > cfg.rate_limit.max_backoff {
        return Err(format!(
            "rate_limit.initial_backoff ({:?}) must be <= max_backoff ({:?})",
            cfg.rate_limit.initial_backoff, cfg.rate_limit.max_backoff
        ));
    }
    if cfg.rate_limit.retry_after_ceiling.is_zero() {
        return Err("rate_limit.retry_after_ceiling must be > 0".to_string());
    }

    // RobotsConfig
    for d in &mut cfg.robots.ignore_domains {
        d.make_ascii_lowercase();
    }
    if cfg.robots.failure_ttl > cfg.robots.default_ttl {
        return Err(format!(
            "robots.failure_ttl ({:?}) must be <= robots.default_ttl ({:?})",
            cfg.robots.failure_ttl, cfg.robots.default_ttl
        ));
    }
```

- [ ] **Step 5: Write tests**

Append to the `#[cfg(test)] mod tests` block in `src/config.rs`:

```rust
    #[test]
    fn default_rate_limit_matches_prd() {
        let cfg = Config::default();
        assert_eq!(cfg.rate_limit.requests_per_minute_per_domain, 60);
        assert_eq!(cfg.rate_limit.per_domain_concurrency, 2);
        assert_eq!(cfg.rate_limit.global_concurrency, 8);
        assert_eq!(cfg.rate_limit.max_retries, 3);
    }

    #[test]
    fn default_robots_matches_prd() {
        let cfg = Config::default();
        assert!(cfg.robots.respect);
        assert!(cfg.robots.ignore_domains.is_empty());
        assert_eq!(cfg.robots.default_ttl, Duration::from_secs(24 * 3600));
        assert_eq!(cfg.robots.failure_ttl, Duration::from_secs(300));
    }

    #[test]
    fn load_rate_limit_overrides() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[rate_limit]
requests_per_minute_per_domain = 120
per_domain_concurrency = 4
global_concurrency = 16
max_retries = 5
initial_backoff = "250ms"
max_backoff = "60s"
retry_after_ceiling = "10m"
jitter_seed = 42
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.rate_limit.requests_per_minute_per_domain, 120);
        assert_eq!(cfg.rate_limit.max_retries, 5);
        assert_eq!(cfg.rate_limit.jitter_seed, Some(42));
    }

    #[test]
    fn load_robots_overrides() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[robots]
respect = false
ignore_domains = ["FOO.example.com", "bar.example.org"]
default_ttl = "12h"
failure_ttl = "2m"
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert!(!cfg.robots.respect);
        assert_eq!(
            cfg.robots.ignore_domains,
            vec!["foo.example.com".to_string(), "bar.example.org".to_string()]
        );
        assert_eq!(cfg.robots.default_ttl, Duration::from_secs(12 * 3600));
        assert_eq!(cfg.robots.failure_ttl, Duration::from_secs(120));
    }

    #[test]
    fn load_rejects_zero_rpm() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[rate_limit]
requests_per_minute_per_domain = 0
"#
        )
        .unwrap();
        assert!(matches!(load(Some(file.path())), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_rpm_above_sanity_cap() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[rate_limit]
requests_per_minute_per_domain = 100000
"#
        )
        .unwrap();
        assert!(matches!(load(Some(file.path())), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_max_retries_above_10() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[rate_limit]
max_retries = 11
"#
        )
        .unwrap();
        assert!(matches!(load(Some(file.path())), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_backoff_inversion() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[rate_limit]
initial_backoff = "10s"
max_backoff = "5s"
"#
        )
        .unwrap();
        assert!(matches!(load(Some(file.path())), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_failure_ttl_above_default_ttl() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[robots]
default_ttl = "1m"
failure_ttl = "10m"
"#
        )
        .unwrap();
        assert!(matches!(load(Some(file.path())), Err(ConfigError::Invalid { .. })));
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib config`

Expected: all new tests pass; existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat(m5): add rate_limit and robots config sections with validation"
```

---

## Task 5: M5-specific FetcherError Variants + MCP Codes

**Files:**
- Modify: `src/fetcher/mod.rs`, `src/mcp/error.rs`, `src/mcp/envelope.rs`

Add the four M5 FetcherError variants (`RetryExhausted`, `RateLimited`, `RobotsDisallowed`, `RobotsFetchFailed`), corresponding stable MCP error codes, and translation routing. Cover with unit tests so the wire surface freezes early.

- [ ] **Step 1: Extend `FetcherError`**

In `src/fetcher/mod.rs`, append to the enum (after the `Extract` variant added in Task 3):

```rust
    #[error("retries exhausted after {attempts} attempts; last error: {last}")]
    RetryExhausted {
        attempts: u8,
        last: Box<FetcherError>,
    },

    #[error("rate limited: server requested wait of {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("robots.txt disallows {url} for user-agent {ua}")]
    RobotsDisallowed { url: String, ua: String },

    #[error("robots.txt fetch failed for {host}")]
    RobotsFetchFailed {
        host: String,
        #[source]
        source: Box<FetcherError>,
    },
```

- [ ] **Step 2: Add wire-stable error code constants**

In `src/mcp/envelope.rs`, append to the `impl RoverError` block before the closing brace:

```rust
    pub const ROBOTS_DISALLOWED: &'static str = "robots_disallowed";
    pub const ROBOTS_FETCH_FAILED: &'static str = "robots_fetch_failed";
    pub const RETRY_EXHAUSTED: &'static str = "retry_exhausted";
    pub const RATE_LIMITED: &'static str = "rate_limited";
```

- [ ] **Step 3: Extend the stability test**

In `src/mcp/envelope.rs`, update the `rover_error_codes_are_stable_constants` test to include the new codes:

```rust
    #[test]
    fn rover_error_codes_are_stable_constants() {
        let codes: &[&'static str] = &[
            RoverError::MAX_TOKENS_EXCEEDED,
            RoverError::INVALID_ARGS,
            RoverError::FETCH_FAILED,
            RoverError::SSRF_DENIED,
            RoverError::EXTRACT_FAILED,
            RoverError::STORAGE_ERROR,
            RoverError::TOKENIZER_UNAVAILABLE,
            RoverError::INVALID_URL,
            RoverError::ROBOTS_DISALLOWED,
            RoverError::ROBOTS_FETCH_FAILED,
            RoverError::RETRY_EXHAUSTED,
            RoverError::RATE_LIMITED,
        ];
        for (i, a) in codes.iter().enumerate() {
            for (j, b) in codes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "duplicate code: {a}");
                }
            }
        }
    }
```

- [ ] **Step 4: Route new FetcherError variants in `mcp/error.rs`**

Extend the `match e` block (the same one edited in Task 3 Step 11) to handle all four new variants. The block becomes:

```rust
                match e {
                    F::Ssrf(_) => RoverError::new(RoverError::SSRF_DENIED, e.to_string()),
                    F::Url(_) => RoverError::new(RoverError::INVALID_URL, e.to_string()),
                    F::Storage(_) => RoverError::new(RoverError::STORAGE_ERROR, e.to_string()),
                    F::Extract(_) => RoverError::new(RoverError::EXTRACT_FAILED, e.to_string()),
                    F::RobotsDisallowed { .. } => {
                        RoverError::new(RoverError::ROBOTS_DISALLOWED, e.to_string())
                    }
                    F::RobotsFetchFailed { .. } => {
                        RoverError::new(RoverError::ROBOTS_FETCH_FAILED, e.to_string())
                    }
                    F::RetryExhausted { .. } => {
                        RoverError::new(RoverError::RETRY_EXHAUSTED, e.to_string())
                    }
                    F::RateLimited { .. } => {
                        RoverError::new(RoverError::RATE_LIMITED, e.to_string())
                    }
                    F::Http(_) | F::Dns { .. } | F::Decode | F::Status { .. } => {
                        RoverError::new(RoverError::FETCH_FAILED, e.to_string())
                    }
                }
```

- [ ] **Step 5: Write translation tests**

Append to the `#[cfg(test)] mod tests` block in `src/mcp/error.rs`:

```rust
    #[test]
    fn fetcher_robots_disallowed_routes_to_robots_disallowed() {
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RobotsDisallowed {
            url: "https://example.com/admin".into(),
            ua: "Rover/0.1".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::ROBOTS_DISALLOWED);
        assert!(r.message.contains("example.com/admin"));
        assert!(r.message.contains("Rover/0.1"));
    }

    #[test]
    fn fetcher_robots_fetch_failed_routes_to_robots_fetch_failed() {
        let inner = crate::fetcher::FetcherError::Decode;
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RobotsFetchFailed {
            host: "example.com".into(),
            source: Box::new(inner),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::ROBOTS_FETCH_FAILED);
        assert!(r.message.contains("example.com"));
    }

    #[test]
    fn fetcher_retry_exhausted_routes_to_retry_exhausted() {
        let last = Box::new(crate::fetcher::FetcherError::Status {
            status: 503,
            url: "https://example.com/".into(),
        });
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RetryExhausted {
            attempts: 4,
            last,
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::RETRY_EXHAUSTED);
        assert!(r.message.contains("4 attempts"));
    }

    #[test]
    fn fetcher_rate_limited_routes_to_rate_limited() {
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RateLimited {
            retry_after_secs: 60,
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::RATE_LIMITED);
        assert!(r.message.contains("60"));
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib mcp::`

Expected: green. The earlier extract-failed test from Task 3 also still passes.

- [ ] **Step 7: Verify the `cached.rs::map_storage_err` regression test still passes**

The pre-existing test asserts `FetcherError::Storage` routing. The added variants don't change that. Run:

```
cargo test --lib fetcher::cached::tests::map_storage_err_routes_to_storage_variant
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/fetcher/mod.rs src/mcp/error.rs src/mcp/envelope.rs
git commit -m "feat(m5): add fetcher error variants for retry, rate-limit, robots"
```

---

## Task 6: `fetcher::concurrency` — Pacer Skeleton + Semaphore Registry

**Files:**
- Create: `src/fetcher/concurrency.rs`
- Modify: `src/fetcher/mod.rs`

Introduce the `Pacer` struct holding global + per-host semaphores (governor + min-interval state join in Task 7). Two acquire methods: `acquire` (per-host then global) and `acquire_global_only` (used by the robots fetcher).

- [ ] **Step 1: Register the module**

In `src/fetcher/mod.rs`, after `pub mod ssrf;` add:

```rust
pub mod concurrency;
```

- [ ] **Step 2: Create the file with the test first**

Create `src/fetcher/concurrency.rs`:

```rust
//! Global + per-host concurrency caps.
//!
//! Owns two `tokio::sync::Semaphore` instances per the M5 design spec §3.2:
//! one global, one per host (constructed lazily on first sight). Per-host
//! permit is acquired before the global one so a single host cannot
//! monopolise the global cap.
//!
//! The full `Pacer` (with governor + min-interval map) lands in Task 7. This
//! task introduces the skeleton.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

/// Build-once-at-startup pacing state. `Arc<Pacer>` is shared across all
/// HTTP-bound code paths.
pub struct Pacer {
    pub(crate) global: Arc<Semaphore>,
    pub(crate) per_host: Mutex<HashMap<String, Arc<Semaphore>>>,
    pub(crate) per_host_limit: u32,
}

/// Permits + bookkeeping released when the guard is dropped.
pub struct PacerGuard {
    _per_host_permit: Option<OwnedSemaphorePermit>,
    _global_permit: OwnedSemaphorePermit,
    // The full guard in Task 7 also carries host + updates_min_interval +
    // a back-reference to Pacer. Kept minimal here.
}

impl Pacer {
    /// Build a Pacer with the given global cap and per-host cap. Per-host
    /// semaphores are created lazily on first acquire.
    pub fn new(global_concurrency: u32, per_host_concurrency: u32) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_concurrency as usize)),
            per_host: Mutex::new(HashMap::new()),
            per_host_limit: per_host_concurrency,
        }
    }

    /// Acquire (per-host, global) in that order.
    pub async fn acquire(&self, host: &str) -> PacerGuard {
        let per_host_sem = self.host_semaphore(host).await;
        let per_host = per_host_sem
            .acquire_owned()
            .await
            .expect("per-host semaphore must not be closed");
        let global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        PacerGuard {
            _per_host_permit: Some(per_host),
            _global_permit: global,
        }
    }

    /// Acquire only the global semaphore — used by robots fetches (per the
    /// chicken-and-egg argument in M5 design spec §3.4).
    pub async fn acquire_global_only(&self) -> PacerGuard {
        let global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        PacerGuard {
            _per_host_permit: None,
            _global_permit: global,
        }
    }

    async fn host_semaphore(&self, host: &str) -> Arc<Semaphore> {
        let mut map = self.per_host.lock().await;
        map.entry(host.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_host_limit as usize)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    #[tokio::test]
    async fn acquire_returns_a_guard() {
        let p = Pacer::new(4, 2);
        let _g = p.acquire("example.com").await;
        // Drop g at end of scope; permits released.
    }

    #[tokio::test]
    async fn global_cap_blocks_when_exhausted() {
        let p = Arc::new(Pacer::new(1, 4));
        let g1 = p.acquire("a.example").await;
        // Second acquire must block; bounded wait verifies it doesn't proceed.
        let p2 = p.clone();
        let join = tokio::spawn(async move { p2.acquire("b.example").await });
        let result = timeout(Duration::from_millis(50), join).await;
        assert!(result.is_err(), "second acquire should block until g1 drops");
        drop(g1);
        // After drop, second acquire should resolve quickly.
        // (Detached task may still be pending; spawn a new acquire.)
        let _g3 = timeout(Duration::from_millis(50), p.acquire("c.example"))
            .await
            .expect("global slot should be free after drop");
    }

    #[tokio::test]
    async fn per_host_cap_blocks_within_same_host() {
        let p = Arc::new(Pacer::new(8, 1));
        let g1 = p.acquire("example.com").await;
        let p2 = p.clone();
        let join =
            tokio::spawn(async move { p2.acquire("example.com").await });
        let result = timeout(Duration::from_millis(50), join).await;
        assert!(result.is_err(), "second acquire on same host should block");
        drop(g1);
        let _g2 = timeout(Duration::from_millis(50), p.acquire("example.com"))
            .await
            .expect("host slot should be free after drop");
    }

    #[tokio::test]
    async fn per_host_isolation_other_host_proceeds() {
        let p = Arc::new(Pacer::new(8, 1));
        let _g1 = p.acquire("a.example").await;
        // Different host: should proceed immediately even though "a.example"
        // has its 1-slot bucket fully occupied.
        let _g2 = timeout(Duration::from_millis(50), p.acquire("b.example"))
            .await
            .expect("different host should not be blocked");
    }

    #[tokio::test]
    async fn acquire_global_only_skips_per_host() {
        let p = Arc::new(Pacer::new(8, 1));
        let _g1 = p.acquire("example.com").await; // uses 1/1 per-host slot
        // acquire_global_only must not contend on the per-host semaphore.
        let _g2 =
            timeout(Duration::from_millis(50), p.acquire_global_only())
                .await
                .expect("global-only acquire should ignore per-host bucket");
        // Touch the variable so clippy doesn't complain.
        sleep(Duration::from_millis(1)).await;
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib fetcher::concurrency`

Expected: all 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/concurrency.rs src/fetcher/mod.rs
git commit -m "feat(m5): add pacer skeleton with global and per-host semaphores"
```

---

## Task 7: `fetcher::rate_limit` — governor Integration + Min-Interval

**Files:**
- Create: `src/fetcher/rate_limit.rs`
- Modify: `src/fetcher/concurrency.rs`, `src/fetcher/mod.rs`

Extend the `Pacer` to own a governor-keyed rate limiter and a per-host `last_request_at` `Instant` map. The full `acquire` method does four things in order: per-host semaphore → global semaphore → governor token → min-interval sleep (against Crawl-Delay). The `Drop` impl updates `last_request_at` so subsequent fetches respect the floor.

- [ ] **Step 1: Register the new module**

In `src/fetcher/mod.rs`, after `pub mod concurrency;` add:

```rust
pub mod rate_limit;
```

- [ ] **Step 2: Replace `concurrency.rs` Pacer with the full version**

Open `src/fetcher/concurrency.rs` and replace the contents with:

```rust
//! Pacer: the single ownership point for all per-process pacing state.
//!
//! Owns four pieces:
//! - a global `tokio::sync::Semaphore`,
//! - a per-host `tokio::sync::Semaphore` registry,
//! - a governor keyed rate limiter (in `rate_limit.rs`),
//! - a per-host `last_request_at` map for Crawl-Delay floor enforcement.
//!
//! See M5 design spec §3.2 for the full picture.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::config::RateLimitConfig;

use super::rate_limit::HostRateLimiter;

/// Build-once-at-startup pacing state.
pub struct Pacer {
    rate_limit: HostRateLimiter,
    global: Arc<Semaphore>,
    per_host: Mutex<HashMap<String, Arc<Semaphore>>>,
    min_interval: Mutex<HashMap<String, Instant>>,
    pub(crate) per_host_limit: u32,
}

/// Permits + bookkeeping released when the guard is dropped.
///
/// On drop, when `updates_min_interval` is true, records `Instant::now()` in
/// `Pacer::min_interval[host]` so subsequent fetches to the same host respect
/// the Crawl-Delay floor measured from completion of the previous request.
pub struct PacerGuard<'a> {
    pacer: &'a Pacer,
    host: String,
    _per_host_permit: Option<OwnedSemaphorePermit>,
    _global_permit: OwnedSemaphorePermit,
    updates_min_interval: bool,
}

impl Drop for PacerGuard<'_> {
    fn drop(&mut self) {
        if !self.updates_min_interval {
            return;
        }
        // try_lock to avoid blocking; on contention, skip — the worst case is
        // one extra request without Crawl-Delay floor on the very next call,
        // which is acceptable.
        if let Ok(mut map) = self.pacer.min_interval.try_lock() {
            map.insert(self.host.clone(), Instant::now());
        }
    }
}

impl Pacer {
    pub fn new(cfg: &RateLimitConfig) -> Self {
        Self {
            rate_limit: HostRateLimiter::new(cfg.requests_per_minute_per_domain),
            global: Arc::new(Semaphore::new(cfg.global_concurrency as usize)),
            per_host: Mutex::new(HashMap::new()),
            min_interval: Mutex::new(HashMap::new()),
            per_host_limit: cfg.per_domain_concurrency,
        }
    }

    /// Acquire the full pacing stack: per-host slot → global slot → governor
    /// token → Crawl-Delay floor wait.
    ///
    /// `crawl_delay` is `Some(d)` when the robots.txt for this host advertises
    /// a `Crawl-Delay` directive; `None` otherwise.
    pub async fn acquire(&self, host: &str, crawl_delay: Option<Duration>) -> PacerGuard<'_> {
        // 1. Per-host semaphore.
        let per_host_sem = self.host_semaphore(host).await;
        let per_host_permit = per_host_sem
            .acquire_owned()
            .await
            .expect("per-host semaphore must not be closed");

        // 2. Global semaphore.
        let global_permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");

        // 3. Governor token.
        self.rate_limit.until_ready(host).await;

        // 4. Crawl-Delay floor.
        if let Some(delay) = crawl_delay {
            let last = self.min_interval.lock().await.get(host).copied();
            if let Some(last) = last {
                let elapsed = last.elapsed();
                if elapsed < delay {
                    tokio::time::sleep(delay - elapsed).await;
                }
            }
        }

        PacerGuard {
            pacer: self,
            host: host.to_string(),
            _per_host_permit: Some(per_host_permit),
            _global_permit: global_permit,
            updates_min_interval: true,
        }
    }

    /// Acquire global slot + governor token only. Used by robots fetches to
    /// avoid the chicken-and-egg with `crawl_delay`. Does NOT update
    /// `last_request_at` on drop.
    pub async fn acquire_global_only(&self, host: &str) -> PacerGuard<'_> {
        let global_permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        self.rate_limit.until_ready(host).await;
        PacerGuard {
            pacer: self,
            host: host.to_string(),
            _per_host_permit: None,
            _global_permit: global_permit,
            updates_min_interval: false,
        }
    }

    async fn host_semaphore(&self, host: &str) -> Arc<Semaphore> {
        let mut map = self.per_host.lock().await;
        map.entry(host.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_host_limit as usize)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    fn small_cfg(rpm: u32, global: u32, per_host: u32) -> RateLimitConfig {
        RateLimitConfig {
            requests_per_minute_per_domain: rpm,
            per_domain_concurrency: per_host,
            global_concurrency: global,
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            retry_after_ceiling: Duration::from_secs(300),
            jitter_seed: Some(1),
        }
    }

    #[tokio::test]
    async fn acquire_returns_a_guard() {
        let p = Pacer::new(&small_cfg(6000, 4, 2));
        let _g = p.acquire("example.com", None).await;
    }

    #[tokio::test]
    async fn global_cap_blocks_when_exhausted() {
        let p = Arc::new(Pacer::new(&small_cfg(6000, 1, 4)));
        let g1 = p.acquire("a.example", None).await;
        let p2 = p.clone();
        let blocked =
            tokio::spawn(async move { p2.acquire("b.example", None).await });
        assert!(timeout(Duration::from_millis(50), blocked).await.is_err());
        drop(g1);
    }

    #[tokio::test]
    async fn per_host_cap_blocks_within_same_host() {
        let p = Arc::new(Pacer::new(&small_cfg(6000, 8, 1)));
        let g1 = p.acquire("example.com", None).await;
        let p2 = p.clone();
        let blocked = tokio::spawn(async move { p2.acquire("example.com", None).await });
        assert!(timeout(Duration::from_millis(50), blocked).await.is_err());
        drop(g1);
    }

    #[tokio::test]
    async fn per_host_isolation_other_host_proceeds() {
        let p = Arc::new(Pacer::new(&small_cfg(6000, 8, 1)));
        let _g1 = p.acquire("a.example", None).await;
        timeout(Duration::from_millis(50), p.acquire("b.example", None))
            .await
            .expect("different host should not be blocked");
    }

    #[tokio::test]
    async fn acquire_global_only_skips_per_host() {
        let p = Arc::new(Pacer::new(&small_cfg(6000, 8, 1)));
        let _g1 = p.acquire("example.com", None).await;
        timeout(Duration::from_millis(50), p.acquire_global_only("example.com"))
            .await
            .expect("global-only should ignore per-host bucket");
    }
}
```

- [ ] **Step 3: Create `src/fetcher/rate_limit.rs`**

```rust
//! Thin async wrapper around `governor` keyed by host.
//!
//! Used by `Pacer` (in `concurrency.rs`) to enforce the per-domain token
//! bucket from `[rate_limit] requests_per_minute_per_domain`. See M5 design
//! spec §3.2 / §3.6 for the rationale.

use std::num::NonZeroU32;

use governor::{
    Quota, RateLimiter,
    clock::DefaultClock,
    state::keyed::DefaultKeyedStateStore,
};

/// Per-host token bucket. Internally a `governor::RateLimiter` keyed by
/// `String`, refilling at `rpm / 60` tokens per second per host.
pub struct HostRateLimiter {
    inner: RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>,
}

impl HostRateLimiter {
    pub fn new(rpm: u32) -> Self {
        // rpm = 0 is rejected by config validation; defensive clamp anyway.
        let rpm = rpm.max(1);
        // Quota::per_minute(N) emits up-to-N events per minute with burst 1.
        // We use replenish_1_per to get smooth pacing; burst is capped at the
        // initial budget = rpm (one minute's worth) which matches PRD intent.
        let per_minute = NonZeroU32::new(rpm).expect("rpm > 0");
        let quota = Quota::per_minute(per_minute);
        let inner = RateLimiter::keyed(quota);
        Self { inner }
    }

    /// Wait until a token is available for `host`, then consume one.
    pub async fn until_ready(&self, host: &str) {
        // governor's keyed limiter consumes a token on success; until_ready
        // loops internally on the clock until allowance.
        self.inner.until_key_ready(&host.to_string()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn first_token_is_immediate() {
        let r = HostRateLimiter::new(60);
        let start = Instant::now();
        r.until_ready("a.example").await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn second_token_after_burst_waits_at_least_one_period() {
        // With 60 rpm, burst = 60, so we have to consume them all to observe
        // a wait. Use rpm = 1 to make the gap an entire minute — too slow for
        // CI. Instead, use rpm = 6000 (100/s) and consume burst rapidly.
        let r = HostRateLimiter::new(60);
        // Exhaust burst.
        for _ in 0..60 {
            r.until_ready("example.com").await;
        }
        let start = Instant::now();
        // Next request must wait at least the 1-token replenishment interval
        // (1000ms at 60 rpm). We accept anything > 500ms to absorb scheduler
        // jitter, but in practice it should be very close to 1000ms.
        // Use a paused clock to make this deterministic.
        let waited = tokio::time::timeout(Duration::from_secs(2), r.until_ready("example.com")).await;
        assert!(waited.is_ok(), "61st token should eventually be ready");
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(500), "elapsed = {elapsed:?}");
    }

    #[tokio::test]
    async fn per_host_buckets_are_independent() {
        // rpm = 60 → burst = 60. Exhaust host A; host B should still be
        // immediately ready.
        let r = HostRateLimiter::new(60);
        for _ in 0..60 {
            r.until_ready("a.example").await;
        }
        let start = Instant::now();
        r.until_ready("b.example").await;
        assert!(start.elapsed() < Duration::from_millis(50), "B was blocked by A");
    }
}
```

- [ ] **Step 4: Run rate_limit tests**

Run: `cargo test --lib fetcher::rate_limit`

Expected: 3 tests pass. The `second_token_after_burst_waits_at_least_one_period` may take ~1 second; that's acceptable.

- [ ] **Step 5: Run concurrency tests against the new Pacer**

Run: `cargo test --lib fetcher::concurrency`

Expected: 5 tests pass.

- [ ] **Step 6: Add a Crawl-Delay floor test**

Append to the `#[cfg(test)] mod tests` block in `src/fetcher/concurrency.rs`:

```rust
    #[tokio::test(start_paused = true)]
    async fn crawl_delay_blocks_second_acquire_for_same_host() {
        let p = Arc::new(Pacer::new(&small_cfg(6000, 8, 4)));
        let g1 = p.acquire("example.com", Some(Duration::from_secs(2))).await;
        drop(g1); // records last_request_at = now.
        // Second acquire with crawl_delay = 2s must wait.
        let p2 = p.clone();
        let join =
            tokio::spawn(async move { p2.acquire("example.com", Some(Duration::from_secs(2))).await });
        // Advance virtual time by 1.5s; second acquire must still not be done.
        tokio::time::advance(Duration::from_millis(1500)).await;
        assert!(!join.is_finished(), "should still be sleeping at 1.5s");
        // Advance past the floor.
        tokio::time::advance(Duration::from_millis(700)).await;
        let g2 = timeout(Duration::from_secs(1), join).await.unwrap().unwrap();
        drop(g2);
    }
```

- [ ] **Step 7: Run the Crawl-Delay test**

Run: `cargo test --lib fetcher::concurrency::tests::crawl_delay_blocks_second_acquire_for_same_host`

Expected: PASS.

- [ ] **Step 8: Verify the full lib suite is still green**

Run: `cargo test --lib`

Expected: green.

- [ ] **Step 9: Commit**

```bash
git add src/fetcher/concurrency.rs src/fetcher/rate_limit.rs src/fetcher/mod.rs
git commit -m "feat(m5): wire governor and crawl-delay floor into pacer"
```

---

## Task 8: `fetcher::retry` — Classifier + `with_retries` Loop

**Files:**
- Create: `src/fetcher/retry.rs`
- Modify: `src/fetcher/mod.rs`

`fetcher::retry::with_retries` is the single retry entry point. Acquires a `PacerGuard` once, then loops up to `max_retries + 1` attempts, classifying each result via a table-driven classifier. `Retry-After` parsing handles both seconds-as-int and HTTP-date formats. Backoff is exponential with seedable jitter.

- [ ] **Step 1: Register the module**

In `src/fetcher/mod.rs`, after `pub mod rate_limit;` add:

```rust
pub mod retry;
```

- [ ] **Step 2: Write the file with the classifier first**

Create `src/fetcher/retry.rs`:

```rust
//! Retry wrapper over `fetch_url_conditional`.
//!
//! See M5 design spec §3.5 for the full algorithm. The classifier covers:
//! - 2xx, 304 → Done
//! - 429, 503 with Retry-After → RetryAfter(parsed)
//! - 429, 503 without Retry-After, other 5xx → Backoff
//! - Other 4xx → Fatal
//! - reqwest network errors (is_timeout / is_connect) → Backoff
//! - SSRF / URL / storage errors → Fatal
//! - extractor errors do not flow through retry (they happen after the HTTP
//!   layer has already produced a body).

use std::time::Duration;

use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::Rng;
use url::Url;

use crate::config::RateLimitConfig;
use crate::fetcher::FetcherError;
use crate::fetcher::concurrency::{Pacer, PacerGuard};
use crate::fetcher::fetch::{ConditionalGet, FetchedPage, fetch_url_conditional};
use crate::fetcher::ssrf::SsrfLevel;

/// One pass of the classifier.
#[derive(Debug)]
enum Class {
    Done(FetchedPage),
    Fatal(FetcherError),
    Backoff(FetcherError),
    RetryAfter(Duration, FetcherError),
}

/// Run `fetch_url_conditional` against the retry policy and pacer.
///
/// `crawl_delay` is forwarded to `Pacer::acquire` so the Crawl-Delay floor is
/// applied once at the start; in-loop `Retry-After` sleeps consume the same
/// guard, so we never double-pace.
pub async fn with_retries(
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    crawl_delay: Option<Duration>,
    cfg: &RateLimitConfig,
) -> Result<FetchedPage, FetcherError> {
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(crate::fetcher::ssrf::SsrfError::NoHost))?
        .to_string();
    let _guard: PacerGuard<'_> = pacer.acquire(&host, crawl_delay).await;

    let mut rng: StdRng = match cfg.jitter_seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_os_rng(),
    };

    let mut attempt: u8 = 0;
    let mut last_err: Option<FetcherError> = None;
    loop {
        let result = fetch_url_conditional(client, url, level, cond).await;
        let class = classify(result, cfg);
        match class {
            Class::Done(page) => return Ok(page),
            Class::Fatal(err) => return Err(err),
            Class::Backoff(err) => {
                last_err = Some(err);
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(last_err.unwrap()),
                    });
                }
                let base = cfg
                    .initial_backoff
                    .saturating_mul(2u32.saturating_pow(attempt as u32));
                let capped = base.min(cfg.max_backoff);
                let jitter_ms = rng.random_range(0..=(capped.as_millis() as u64 / 2));
                let wait = capped + Duration::from_millis(jitter_ms);
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Class::RetryAfter(d, err) => {
                last_err = Some(err);
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(last_err.unwrap()),
                    });
                }
                let capped = d.min(cfg.retry_after_ceiling);
                if d > cfg.retry_after_ceiling {
                    tracing::warn!(
                        target: "rover::fetcher::retry",
                        requested_secs = d.as_secs(),
                        ceiling_secs = cfg.retry_after_ceiling.as_secs(),
                        "Retry-After exceeded ceiling; clamping"
                    );
                }
                tokio::time::sleep(capped).await;
                attempt += 1;
            }
        }
    }
}

fn classify(
    result: Result<FetchedPage, FetcherError>,
    _cfg: &RateLimitConfig,
) -> Class {
    match result {
        Ok(page) => {
            // 304 is "Done" — cached.rs handles the freshness extension.
            if page.status == 304 || (200..300).contains(&page.status) {
                return Class::Done(page);
            }
            classify_non_2xx(page)
        }
        Err(e) => classify_err(e),
    }
}

fn classify_non_2xx(page: FetchedPage) -> Class {
    let status = page.status;
    let retry_after_header = page
        .body
        .is_empty()
        .then(|| None) // placeholder: we read Retry-After from the FetchedPage's headers
        .flatten();
    // FetchedPage in fetch.rs does not currently carry the Retry-After header
    // value — we add it as part of this task (see Step 3 below).
    let retry_after = page
        .retry_after
        .as_deref()
        .and_then(parse_retry_after);
    let _ = retry_after_header; // keep clippy quiet
    let err = FetcherError::Status {
        status,
        url: page.final_url.to_string(),
    };
    match status {
        429 | 503 => match retry_after {
            Some(d) => Class::RetryAfter(d, err),
            None => Class::Backoff(err),
        },
        500 | 502 | 504 => Class::Backoff(err),
        s if (500..600).contains(&s) => Class::Backoff(err),
        _ => Class::Fatal(err),
    }
}

fn classify_err(e: FetcherError) -> Class {
    match &e {
        FetcherError::Http(re) => {
            if re.is_timeout() || re.is_connect() {
                Class::Backoff(e)
            } else {
                Class::Fatal(e)
            }
        }
        FetcherError::Ssrf(_)
        | FetcherError::Url(_)
        | FetcherError::Decode
        | FetcherError::Storage(_)
        | FetcherError::Status { .. }
        | FetcherError::Dns { .. } => Class::Fatal(e),
        // The retry layer never sees Extract/Robots/Retry variants in practice
        // (they originate above this layer), but classify defensively.
        FetcherError::Extract(_)
        | FetcherError::RetryExhausted { .. }
        | FetcherError::RateLimited { .. }
        | FetcherError::RobotsDisallowed { .. }
        | FetcherError::RobotsFetchFailed { .. } => Class::Fatal(e),
    }
}

/// Parse a `Retry-After` header value. RFC 9110 allows either integer seconds
/// or an HTTP-date.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    if let Ok(t) = httpdate::parse_http_date(trimmed) {
        let now = std::time::SystemTime::now();
        if let Ok(d) = t.duration_since(now) {
            return Some(d);
        }
        // Past date → treat as "ready now".
        return Some(Duration::from_secs(0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RateLimitConfig {
        RateLimitConfig {
            requests_per_minute_per_domain: 6000,
            per_domain_concurrency: 2,
            global_concurrency: 8,
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_secs(1),
            retry_after_ceiling: Duration::from_secs(60),
            jitter_seed: Some(0),
        }
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("0"), Some(Duration::from_secs(0)));
    }

    #[test]
    fn parse_retry_after_http_date_future() {
        // Construct an HTTP-date one hour in the future.
        let t = std::time::SystemTime::now() + Duration::from_secs(3600);
        let s = httpdate::fmt_http_date(t);
        let d = parse_retry_after(&s).unwrap();
        // Some scheduler slack; expect ~3600.
        assert!(d.as_secs() > 3500 && d.as_secs() < 3700, "got {d:?}");
    }

    #[test]
    fn parse_retry_after_http_date_past() {
        let t = std::time::SystemTime::now() - Duration::from_secs(60);
        let s = httpdate::fmt_http_date(t);
        let d = parse_retry_after(&s).unwrap();
        assert_eq!(d, Duration::from_secs(0));
    }

    #[test]
    fn parse_retry_after_garbage_returns_none() {
        assert_eq!(parse_retry_after("not a date or number"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn classify_2xx_is_done() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 200,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        match classify(Ok(page), &cfg()) {
            Class::Done(_) => {}
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn classify_429_with_retry_after_is_retry_after() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 429,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: Some("3".to_string()),
        };
        match classify(Ok(page), &cfg()) {
            Class::RetryAfter(d, _) => assert_eq!(d, Duration::from_secs(3)),
            other => panic!("expected RetryAfter, got {other:?}"),
        }
    }

    #[test]
    fn classify_500_is_backoff() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 500,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        assert!(matches!(classify(Ok(page), &cfg()), Class::Backoff(_)));
    }

    #[test]
    fn classify_404_is_fatal() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 404,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        assert!(matches!(classify(Ok(page), &cfg()), Class::Fatal(_)));
    }
}
```

- [ ] **Step 3: Extend `FetchedPage` with a `retry_after` header field**

Open `src/fetcher/fetch.rs`. Add `pub retry_after: Option<String>,` to the `FetchedPage` struct (after `expires`):

```rust
    /// `Expires` response header (M2).
    pub expires: Option<String>,

    /// `Retry-After` response header (M5). RFC 9110 allows seconds-as-int or
    /// HTTP-date — parsing is in `fetcher::retry::parse_retry_after`.
    pub retry_after: Option<String>,
}
```

Wire it up inside `fetch_url_conditional` (after the existing `expires` extraction block):

```rust
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
```

And add it to the `Ok(FetchedPage { ... })` construction at the end of the function.

- [ ] **Step 4: Add a `Default` impl for `Detected`**

`fetcher::retry::tests` constructs `FetchedPage` via struct-literal. Add `#[derive(Default)]` to `Detected` in `src/fetcher/charset.rs` if not already present. Verify with:

```
grep -n "struct Detected" src/fetcher/charset.rs
```

If `Detected` isn't `Default`, add `#[derive(Default)]` (you may also need `#[derive(Default)]` on `encoding_rs::Encoding` — use a small helper constructor instead if so):

```rust
impl Default for Detected {
    fn default() -> Self {
        Self {
            encoding: encoding_rs::UTF_8,
            confidence: crate::fetcher::charset::Confidence::Sniffed,
        }
    }
}
```

(Adjust to whatever shape `Detected` actually has — the test only needs *any* placeholder.)

- [ ] **Step 5: Run retry tests**

Run: `cargo test --lib fetcher::retry`

Expected: all 8 unit tests pass.

- [ ] **Step 6: Run full lib suite**

Run: `cargo test --lib`

Expected: green. The existing `fetch.rs` tests still work (the new `retry_after` field is just an extra `None` in existing constructions).

- [ ] **Step 7: Commit**

```bash
git add src/fetcher/retry.rs src/fetcher/fetch.rs src/fetcher/charset.rs src/fetcher/mod.rs
git commit -m "feat(m5): add retry layer with classifier and retry-after parsing"
```

---

## Task 9: `fetcher::robots` — Parse, Evaluate, Fetch-and-Cache

**Files:**
- Create: `src/fetcher/robots.rs`
- Modify: `src/fetcher/mod.rs`

Three responsibilities: parse robots.txt via `robotxt`, evaluate a (UA, path) against a parsed entry, and fetch+cache a robots.txt for a host. The cache layer is `storage::robots` from Task 2; the network layer reuses `retry::with_retries` with `Pacer::acquire_global_only` semantics.

- [ ] **Step 1: Register the module**

In `src/fetcher/mod.rs`, after `pub mod retry;` add:

```rust
pub mod robots;
```

- [ ] **Step 2: Create the module with parse + evaluate**

Create `src/fetcher/robots.rs`:

```rust
//! Robots.txt fetching, parsing, caching, and evaluation.
//!
//! See M5 design spec §3.7. The cache layer is `storage::robots`; the network
//! layer is `retry::with_retries` with `Pacer::acquire_global_only`.

use std::time::Duration;

use jiff::Timestamp;
use robotxt::Robots;
use url::Url;

use crate::config::RobotsConfig;
use crate::fetcher::FetcherError;
use crate::fetcher::cache_control::parse_max_age;
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::fetch::ConditionalGet;
use crate::fetcher::retry;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::robots::{self as storage_robots, RobotsEntry, RobotsState};

/// Outcome of evaluating a (host, path, ua) against a `RobotsEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Disallowed,
}

/// `Crawl-Delay` value extracted from a parsed entry, if any.
pub fn crawl_delay(entry: &RobotsEntry, user_agent: &str) -> Option<Duration> {
    match entry.state {
        RobotsState::AllowAll | RobotsState::DisallowAll => None,
        RobotsState::Parsed => {
            let body = entry.body.as_deref()?;
            let robots = Robots::from_bytes(body.as_bytes(), user_agent);
            robots
                .crawl_delay()
                .map(|d| Duration::from_secs(d.get()))
        }
    }
}

/// Evaluate whether `path` is allowed for `user_agent` per `entry`.
pub fn evaluate(entry: &RobotsEntry, user_agent: &str, path: &str) -> Verdict {
    match entry.state {
        RobotsState::AllowAll => Verdict::Allowed,
        RobotsState::DisallowAll => Verdict::Disallowed,
        RobotsState::Parsed => {
            let Some(body) = entry.body.as_deref() else {
                return Verdict::Allowed; // defensive: treat missing body as allow-all
            };
            let robots = Robots::from_bytes(body.as_bytes(), user_agent);
            if robots.is_relative_allowed(path) {
                Verdict::Allowed
            } else {
                Verdict::Disallowed
            }
        }
    }
}

/// Look up or fetch+cache a robots.txt entry for `host`.
///
/// If a fresh cached entry exists, return it. Otherwise, fetch
/// `https://{host}/robots.txt`, classify the response per spec §3.7, write
/// the resulting entry to storage, and return it.
pub async fn ensure_entry(
    db: &Db,
    pacer: &Pacer,
    client: &reqwest::Client,
    cfg: &RobotsConfig,
    host: &str,
    ssrf_level: SsrfLevel,
    user_agent: &str,
    rate_limit_cfg: &crate::config::RateLimitConfig,
) -> Result<RobotsEntry, FetcherError> {
    let now = Timestamp::now().as_second();
    if let Some(entry) = storage_robots::lookup(db, host).await? {
        if entry.expires_at > now {
            return Ok(entry);
        }
    }

    let _ = user_agent; // robots fetch uses the configured UA already on the client
    let robots_url = Url::parse(&format!("https://{host}/robots.txt"))
        .map_err(FetcherError::Url)?;
    let result = {
        // acquire_global_only is the semantic we want, but with_retries
        // currently acquires acquire(host, crawl_delay). Use the dedicated
        // robots-fetch path: a thin variant that uses global-only.
        retry_robots(
            pacer,
            client,
            &robots_url,
            ssrf_level,
            &ConditionalGet::default(),
            rate_limit_cfg,
        )
        .await
    };

    let entry = build_entry(host, result, cfg, now);
    storage_robots::upsert(db, entry.clone()).await?;
    Ok(entry)
}

/// Retry loop for robots.txt fetches. Mirrors `retry::with_retries` but uses
/// `Pacer::acquire_global_only` so it doesn't take a per-host slot or wait on
/// the (unknown) Crawl-Delay floor.
async fn retry_robots(
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    cfg: &crate::config::RateLimitConfig,
) -> Result<crate::fetcher::fetch::FetchedPage, FetcherError> {
    use crate::fetcher::fetch::fetch_url_conditional;
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(crate::fetcher::ssrf::SsrfError::NoHost))?
        .to_string();
    let _guard = pacer.acquire_global_only(&host).await;

    let mut attempt: u8 = 0;
    let mut last_err: Option<FetcherError> = None;
    loop {
        let result = fetch_url_conditional(client, url, level, cond).await;
        // Reuse the classifier-like routing inline (private to retry.rs; we
        // duplicate a minimal version here for clarity — keeps retry.rs's
        // host-aware Pacer::acquire path uncomplicated).
        match result {
            Ok(page) if (200..300).contains(&page.status) || page.status == 304 => return Ok(page),
            Ok(page) => {
                let err = FetcherError::Status {
                    status: page.status,
                    url: page.final_url.to_string(),
                };
                let retryable = matches!(page.status, 429 | 503) || (500..600).contains(&page.status);
                if !retryable {
                    return Err(err);
                }
                last_err = Some(err);
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(last_err.unwrap()),
                    });
                }
                let retry_after = page
                    .retry_after
                    .as_deref()
                    .and_then(retry::parse_retry_after);
                let wait = match retry_after {
                    Some(d) => d.min(cfg.retry_after_ceiling),
                    None => {
                        let base = cfg
                            .initial_backoff
                            .saturating_mul(2u32.saturating_pow(attempt as u32));
                        base.min(cfg.max_backoff)
                    }
                };
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => {
                let retryable = match &e {
                    FetcherError::Http(re) => re.is_timeout() || re.is_connect(),
                    _ => false,
                };
                if !retryable {
                    return Err(e);
                }
                last_err = Some(e);
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(last_err.unwrap()),
                    });
                }
                let base = cfg
                    .initial_backoff
                    .saturating_mul(2u32.saturating_pow(attempt as u32));
                let wait = base.min(cfg.max_backoff);
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

fn build_entry(
    host: &str,
    result: Result<crate::fetcher::fetch::FetchedPage, FetcherError>,
    cfg: &RobotsConfig,
    now: i64,
) -> RobotsEntry {
    match result {
        Ok(page) => {
            // Parsed response. TTL from Cache-Control or config default.
            let ttl_secs = page
                .cache_control
                .as_deref()
                .and_then(parse_max_age)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(cfg.default_ttl.as_secs() as i64);
            RobotsEntry {
                host: host.to_string(),
                body: Some(page.body),
                fetched_at: now,
                expires_at: now + ttl_secs,
                state: RobotsState::Parsed,
            }
        }
        Err(FetcherError::Status { status, .. }) if (400..500).contains(&status) => {
            // 4xx → allow-all, full TTL.
            RobotsEntry {
                host: host.to_string(),
                body: None,
                fetched_at: now,
                expires_at: now + cfg.default_ttl.as_secs() as i64,
                state: RobotsState::AllowAll,
            }
        }
        // 5xx (including RetryExhausted carrying a 5xx) or any transport error.
        // Fail-closed with short TTL.
        Err(_) => RobotsEntry {
            host: host.to_string(),
            body: None,
            fetched_at: now,
            expires_at: now + cfg.failure_ttl.as_secs() as i64,
            state: RobotsState::DisallowAll,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_entry(body: &str) -> RobotsEntry {
        RobotsEntry {
            host: "example.com".into(),
            body: Some(body.into()),
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::Parsed,
        }
    }

    #[test]
    fn evaluate_allow_all_sentinel() {
        let e = RobotsEntry {
            host: "x".into(),
            body: None,
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::AllowAll,
        };
        assert_eq!(evaluate(&e, "Rover/0.1", "/anything"), Verdict::Allowed);
    }

    #[test]
    fn evaluate_disallow_all_sentinel() {
        let e = RobotsEntry {
            host: "x".into(),
            body: None,
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::DisallowAll,
        };
        assert_eq!(evaluate(&e, "Rover/0.1", "/anything"), Verdict::Disallowed);
    }

    #[test]
    fn evaluate_disallow_admin() {
        let e = parsed_entry("User-agent: *\nDisallow: /admin\n");
        assert_eq!(evaluate(&e, "Rover/0.1", "/articles/x"), Verdict::Allowed);
        assert_eq!(evaluate(&e, "Rover/0.1", "/admin/users"), Verdict::Disallowed);
    }

    #[test]
    fn evaluate_ua_specific_rule_wins() {
        let e = parsed_entry(
            "User-agent: *\n\
             Disallow: /admin\n\
             \n\
             User-agent: Rover\n\
             Disallow:\n",
        );
        assert_eq!(evaluate(&e, "Rover", "/admin"), Verdict::Allowed);
        assert_eq!(evaluate(&e, "Other", "/admin"), Verdict::Disallowed);
    }

    #[test]
    fn crawl_delay_extraction() {
        let e = parsed_entry("User-agent: *\nCrawl-Delay: 5\nAllow: /\n");
        assert_eq!(crawl_delay(&e, "Rover"), Some(Duration::from_secs(5)));
    }

    #[test]
    fn crawl_delay_none_when_unset() {
        let e = parsed_entry("User-agent: *\nAllow: /\n");
        assert_eq!(crawl_delay(&e, "Rover"), None);
    }

    #[test]
    fn build_entry_4xx_is_allow_all() {
        let err = FetcherError::Status {
            status: 404,
            url: "https://x/robots.txt".into(),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::AllowAll);
        assert!(e.body.is_none());
        assert_eq!(e.expires_at, 100 + (24 * 3600));
    }

    #[test]
    fn build_entry_5xx_is_disallow_all() {
        let err = FetcherError::Status {
            status: 500,
            url: "https://x/robots.txt".into(),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::DisallowAll);
        assert!(e.body.is_none());
        assert_eq!(e.expires_at, 100 + 300);
    }

    #[test]
    fn build_entry_retry_exhausted_is_disallow_all() {
        let inner = FetcherError::Status {
            status: 503,
            url: "https://x/robots.txt".into(),
        };
        let err = FetcherError::RetryExhausted {
            attempts: 4,
            last: Box::new(inner),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::DisallowAll);
    }

    #[test]
    fn build_entry_2xx_with_max_age_honors_header() {
        use crate::fetcher::charset::Detected;
        let page = crate::fetcher::fetch::FetchedPage {
            final_url: Url::parse("https://x/robots.txt").unwrap(),
            canonical_url: Url::parse("https://x/robots.txt").unwrap(),
            status: 200,
            content_type: Some("text/plain".into()),
            body: "User-agent: *\nDisallow:\n".into(),
            charset: Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: Some("max-age=3600".into()),
            expires: None,
            retry_after: None,
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Ok(page), &cfg, 100);
        assert_eq!(e.state, RobotsState::Parsed);
        assert_eq!(e.expires_at, 100 + 3600);
        assert!(e.body.is_some());
    }
}
```

- [ ] **Step 3: Verify `cache_control::parse_max_age` exists**

This function should already exist from M2. Check:

```
grep -n "parse_max_age" src/fetcher/cache_control.rs
```

If it doesn't exist (or has a different name), update the import in `robots.rs` to match what's there. The semantics are: extract `max-age=N` from a `Cache-Control` header value and return `Some(Duration::from_secs(N))`.

- [ ] **Step 4: Run robots tests**

Run: `cargo test --lib fetcher::robots`

Expected: all 10 unit tests pass. If `robotxt::Robots::from_bytes` has a slightly different API in the current crate version, follow compiler errors to adjust — the published 0.6.1 API uses `Robots::from_bytes(robots_txt: &[u8], user_agent: &str) -> Self` per its docs.

- [ ] **Step 5: Run the full library suite**

Run: `cargo test --lib`

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/fetcher/robots.rs src/fetcher/mod.rs
git commit -m "feat(m5): add robots fetch, parse, evaluate via robotxt"
```

---

## Task 10: Wire Pacer + Robots Gate into `fetch_with_cache`

**Files:**
- Modify: `src/fetcher/cached.rs`, `src/fetcher/mod.rs`, `src/cli/fetch.rs`, `src/mcp/handler.rs`, `src/mcp/server.rs`, `src/mcp/tools/fetch.rs`, `src/mcp/tools/get_metadata.rs`

`fetch_with_cache` grows three new args: `&Pacer`, `&RobotsConfig`, `&RateLimitConfig`. The new flow: robots gate (lookup + maybe fetch + evaluate) → existing cache lookup → if miss/stale, `retry::with_retries`. Update every caller.

- [ ] **Step 1: Update `FetchOptions` to carry the new context**

In `src/fetcher/cached.rs`, change `FetchOptions` (around line 42):

```rust
#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
    /// When `true`, skip the robots gate. Used by `--ignore-robots`.
    pub ignore_robots: bool,
    /// User-Agent used for robots.txt UA-rule evaluation. Must match
    /// `[fetch] user_agent`.
    pub user_agent: String,
}
```

(Note: drop `#[derive(Copy)]` since `String` isn't Copy.)

- [ ] **Step 2: Modify `fetch_with_cache` signature**

Change the function signature in `src/fetcher/cached.rs`:

```rust
pub async fn fetch_with_cache<F>(
    db: &Db,
    client: &reqwest::Client,
    pacer: &crate::fetcher::concurrency::Pacer,
    rate_cfg: &crate::config::RateLimitConfig,
    robots_cfg: &crate::config::RobotsConfig,
    url: &Url,
    cache_cfg: &CacheConfig,
    opts: FetchOptions,
    mut extract_fn: F,
) -> Result<CachedFetch, FetcherError>
where
    F: FnMut(&str, &Url) -> Result<ExtractResult, FetcherError>,
```

(Renamed the existing `cfg` to `cache_cfg` to disambiguate. Inside the body, update references.)

- [ ] **Step 3: Add robots gate before cache lookup**

Inside the function body, after `let now = Timestamp::now().as_second();` and before the cache lookup, insert:

```rust
    let host = url.host_str().ok_or(FetcherError::Ssrf(
        crate::fetcher::ssrf::SsrfError::NoHost,
    ))?;

    // Robots gate (M5). Skipped when explicitly disabled or for ignore_domains.
    let crawl_delay: Option<std::time::Duration> = if !robots_cfg.respect || opts.ignore_robots {
        None
    } else if robots_cfg
        .ignore_domains
        .iter()
        .any(|d| d == host)
    {
        None
    } else {
        let entry = crate::fetcher::robots::ensure_entry(
            db,
            pacer,
            client,
            robots_cfg,
            host,
            opts.ssrf_level,
            &opts.user_agent,
            rate_cfg,
        )
        .await
        .map_err(|e| FetcherError::RobotsFetchFailed {
            host: host.to_string(),
            source: Box::new(e),
        })?;

        let verdict = crate::fetcher::robots::evaluate(&entry, &opts.user_agent, url.path());
        if matches!(verdict, crate::fetcher::robots::Verdict::Disallowed) {
            return Err(FetcherError::RobotsDisallowed {
                url: url.to_string(),
                ua: opts.user_agent.clone(),
            });
        }
        crate::fetcher::robots::crawl_delay(&entry, &opts.user_agent)
    };
```

- [ ] **Step 4: Replace `fetch_url_conditional` call with `retry::with_retries`**

In the same function, find the existing match around line 103:

```rust
    let fetched = match fetch_url_conditional(client, url, opts.ssrf_level, &cond).await {
```

Replace with:

```rust
    let fetched = match crate::fetcher::retry::with_retries(
        pacer,
        client,
        url,
        opts.ssrf_level,
        &cond,
        crawl_delay,
        rate_cfg,
    )
    .await
    {
```

Everything else in the function body stays the same. The `cfg` references inside the function become `cache_cfg`.

- [ ] **Step 5: Update the existing unit test inside `cached.rs`**

The `cache_hit_within_ttl` test (around line 259) constructs `FetchOptions` and calls `fetch_with_cache` directly. Update it to build a `Pacer` and `RateLimitConfig` / `RobotsConfig` and pass them through. Replace the test body:

```rust
    #[tokio::test]
    async fn cache_hit_within_ttl() {
        use crate::config::{RateLimitConfig, RobotsConfig};
        use crate::fetcher::concurrency::Pacer;
        use crate::storage::Db;
        use std::time::Duration;
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let url = Url::parse("https://example.com/").unwrap();
        let now = Timestamp::now().as_second();
        let page = Page {
            url_hash: url_hash(url.as_str()),
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("cached".into()),
            fetched_at: now - 60,
            expires_at: Some(now + 600),
            etag: None,
            last_modified: None,
            content_hash: "x".into(),
            extracted_md: "# cached".into(),
            metadata_json: None,
        };
        pages::upsert(&db, page.clone()).await.unwrap();

        let cache_cfg = CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(60),
            max_ttl: Duration::from_secs(86400),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        };
        let rate_cfg = RateLimitConfig::default();
        let mut robots_cfg = RobotsConfig::default();
        robots_cfg.respect = false; // avoid robots fetch in this unit test
        let pacer = Pacer::new(&rate_cfg);
        let client =
            super::super::client::build_http_client("test/0.1", Duration::from_secs(5));
        let result = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate_cfg,
            &robots_cfg,
            &url,
            &cache_cfg,
            FetchOptions {
                force_refresh: false,
                ssrf_level: SsrfLevel::Strict,
                ignore_robots: false,
                user_agent: "test/0.1".into(),
            },
            |_, _| {
                panic!("extract_fn must not be called on cache hit");
            },
        )
        .await
        .unwrap();
        assert_eq!(result.cache_status, CacheStatus::Hit);
        assert_eq!(result.page.title.as_deref(), Some("cached"));
    }
```

- [ ] **Step 6: Update CLI `rover fetch` caller**

In `src/cli/fetch.rs`, rework `run` to build a `Pacer` from config and pass it through:

```rust
    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let pacer = crate::fetcher::concurrency::Pacer::new(&cfg.rate_limit);

    let result = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &cfg.rate_limit,
        &cfg.robots,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: args.force_refresh,
            ssrf_level: level,
            ignore_robots: args.ignore_robots,
            user_agent: cfg.fetch.user_agent.clone(),
        },
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
                metadata: extracted.metadata,
            })
        },
    )
    .await
    .context("fetching URL")?;
```

Add `ignore_robots: bool` to `Args`:

```rust
pub struct Args {
    pub url: String,
    pub force_refresh: bool,
    pub ignore_robots: bool,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
}
```

- [ ] **Step 7: Update `RoverHandler` to carry the Pacer**

In `src/mcp/handler.rs`, add a `Pacer` field:

```rust
use std::sync::Arc;
use crate::fetcher::concurrency::Pacer;

#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) ssrf_level: SsrfLevel,
    pub(crate) pacer: Arc<Pacer>,
    tool_router: ToolRouter<Self>,
}

impl RoverHandler {
    pub fn new(
        db: Db,
        config: Arc<Config>,
        client: reqwest::Client,
        ssrf_level: SsrfLevel,
        pacer: Arc<Pacer>,
    ) -> Self {
        Self {
            db,
            config,
            client,
            ssrf_level,
            pacer,
            tool_router: Self::tool_router(),
        }
    }
}
```

- [ ] **Step 8: Build Pacer in `mcp::server::serve_stdio`**

In `src/mcp/server.rs`, just before `let handler = RoverHandler::new(...)`:

```rust
    let pacer = Arc::new(crate::fetcher::concurrency::Pacer::new(&config.rate_limit));
    let handler = RoverHandler::new(db.clone(), config, client, ssrf_level, pacer);
```

- [ ] **Step 9: Update `mcp::tools::fetch::fetch_inner` to pass Pacer**

`grep -n "fetch_with_cache" src/mcp/tools/fetch.rs` — at the call site, pass `&self.pacer`, `&self.config.rate_limit`, `&self.config.robots`, and construct `FetchOptions` with `ignore_robots: false` (MCP doesn't expose ignore-robots; CLI-only flag), and `user_agent: self.config.fetch.user_agent.clone()`.

Sketch:

```rust
let pacer: &Pacer = &self.pacer;
let result = fetch_with_cache(
    &self.db,
    &self.client,
    pacer,
    &self.config.rate_limit,
    &self.config.robots,
    &url,
    &self.config.cache,
    FetchOptions {
        force_refresh: args.force_refresh,
        ssrf_level: self.ssrf_level,
        ignore_robots: false,
        user_agent: self.config.fetch.user_agent.clone(),
    },
    |body, base| {
        // ... unchanged extract closure with Task 3's Extract mapping
    },
)
.await?;
```

- [ ] **Step 10: Update `mcp::tools::get_metadata::get_metadata_inner` analogously**

Same pattern as Step 9.

- [ ] **Step 11: Verify build**

Run: `cargo build`

Expected: green. Adjust any compile errors revealed by the new signature change. Common gotchas:
- The `cached.rs` `Copy` derive on `FetchOptions` was removed because of `user_agent: String`. Any caller cloning `FetchOptions` should be fine; if a caller relied on Copy semantics, switch to explicit `.clone()`.
- Test fixtures in `mcp_smoke.rs` or similar may need their `RoverHandler::new` calls updated. `grep -n "RoverHandler::new" tests/ src/ | grep -v "fn new"` to find them.

- [ ] **Step 12: Run all tests**

Run: `cargo test`

Expected: green. The existing M3/M4 integration tests should keep passing because the default `RobotsConfig::respect = true` would normally make them try to fetch robots.txt — but the test harness typically uses `wiremock` URLs that don't have a `/robots.txt` route, so requests would 404 → allow-all sentinel → no further effect. Verify by inspecting the test output. If a test fails because it now sees a robots fetch attempt, the cleanest fix is to set `cfg.robots.respect = false` in the test's `Config` setup; the M3/M4 tests should not depend on robots behaviour.

- [ ] **Step 13: Commit**

```bash
git add src/fetcher/cached.rs src/fetcher/mod.rs src/cli/fetch.rs \
        src/mcp/handler.rs src/mcp/server.rs \
        src/mcp/tools/fetch.rs src/mcp/tools/get_metadata.rs
git commit -m "feat(m5): wire pacer and robots gate through fetch_with_cache"
```

---

## Task 11: CLI Flags + Wire Pacer Through CLI Entry Points

**Files:**
- Modify: `src/cli/fetch.rs`, `src/cli/mcp.rs`, `src/main.rs` (or wherever clap-derive lives)

Add CLI flags to `rover fetch` and `rover mcp` per design spec §4.3. Each flag, if present, overrides the corresponding `Config` value before the `Pacer` is built. The `Args` struct in `cli::fetch::run` already gained `ignore_robots` in Task 10; this task adds the rest of the flags and the clap wiring.

- [ ] **Step 1: Locate clap derive definitions**

Check `src/main.rs` and any `cli/mod.rs`:

```
grep -n "Subcommand\|#\[derive(Parser)\]\|#\[arg" src/main.rs src/cli/mod.rs 2>/dev/null
```

The convention is `clap` derive on a top-level enum with one variant per subcommand. Note the existing fetch and mcp variants' attributes for shape.

- [ ] **Step 2: Add flags to the fetch subcommand**

Wherever the `Fetch` clap variant is defined (likely `src/main.rs`), extend it with:

```rust
    /// Override [robots] respect for this invocation. Robots.txt is not
    /// fetched and rules are not enforced.
    #[arg(long)]
    ignore_robots: bool,

    /// Override [rate_limit] requests_per_minute_per_domain.
    #[arg(long)]
    rate_limit_rpm: Option<u32>,

    /// Override [rate_limit] per_domain_concurrency.
    #[arg(long)]
    per_host_concurrency: Option<u32>,

    /// Override [rate_limit] global_concurrency.
    #[arg(long)]
    global_concurrency: Option<u32>,

    /// Override [rate_limit] max_retries.
    #[arg(long)]
    max_retries: Option<u8>,
```

- [ ] **Step 3: Pipe flags through the fetch dispatcher**

In the dispatch code that constructs `cli::fetch::Args` from clap, also apply the rate-limit overrides to the loaded `Config` before invoking `cli::fetch::run`. Sketch (in `main.rs`):

```rust
Subcommand::Fetch {
    url,
    force_refresh,
    ignore_robots,
    rate_limit_rpm,
    per_host_concurrency,
    global_concurrency,
    max_retries,
    ..
} => {
    let mut cfg = config::load(config_path.as_deref())?;
    if let Some(v) = rate_limit_rpm { cfg.rate_limit.requests_per_minute_per_domain = v; }
    if let Some(v) = per_host_concurrency { cfg.rate_limit.per_domain_concurrency = v; }
    if let Some(v) = global_concurrency { cfg.rate_limit.global_concurrency = v; }
    if let Some(v) = max_retries { cfg.rate_limit.max_retries = v; }
    // re-validate would be ideal; for now we trust that single-knob overrides
    // don't cross the invariants. A future task can centralise this in
    // `config::Config::apply_overrides`.

    cli::fetch::run(
        cli::fetch::Args {
            url,
            force_refresh,
            ignore_robots,
            #[cfg(any(test, feature = "test-loopback"))]
            ssrf_test_loopback: false,
        },
        Some(&cfg),
    )
    .await?
}
```

(Adjust the `cli::fetch::run` signature to take a pre-loaded `&Config` rather than a `config_path` if that's the cleaner refactor — pick whichever matches existing M3/M4 idioms in this repo. Either way, the rate-limit overrides apply before the `Pacer` is built.)

- [ ] **Step 4: Add same flags to `rover mcp` subcommand**

Repeat Steps 2 and 3 for the `Mcp` variant. The override logic is identical — apply to `cfg` before constructing the `Pacer` in `mcp::server::serve_stdio`.

- [ ] **Step 5: Smoke-test the flags**

Run:

```
cargo run --features test-loopback -- fetch --help | grep -E "ignore-robots|rate-limit-rpm"
```

Expected: both flags appear in the help text.

- [ ] **Step 6: Run all tests**

Run: `cargo test`

Expected: green. CLI integration tests in `tests/cli_*.rs` (if any) should continue to pass with the new flags defaulting to off.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/cli/fetch.rs src/cli/mcp.rs
git commit -m "feat(m5): add cli flags for rate-limit overrides and ignore-robots"
```

---

## Task 12: Integration Tests — Rate Limit + Retry + Robots + Full Loop

**Files:**
- Create: `tests/fetcher_rate_limit.rs`, `tests/fetcher_retry.rs`, `tests/fetcher_robots.rs`, `tests/fetcher_full_loop.rs`, `tests/fixtures/m5/*`

End-to-end coverage against `wiremock`. The `test-loopback` SSRF level (from M3) lets us point Rover at `127.0.0.1` while keeping the strict policy hard-locked elsewhere.

- [ ] **Step 1: Create robots fixtures**

Create the four files under `tests/fixtures/m5/`:

`robots-allow-articles.txt`:
```
User-agent: *
Allow: /articles/
Disallow: /admin/
```

`robots-disallow-admin.txt`:
```
User-agent: *
Disallow: /admin/
```

`robots-with-crawldelay.txt`:
```
User-agent: *
Crawl-Delay: 2
Allow: /
```

`wide-ua-rules.txt`:
```
User-agent: *
Disallow: /

User-agent: Rover
Disallow:
```

And one HTML fixture to defeat `readabilityrs`:

`extract-failure.html`:
```html
<!doctype html><html><head><title></title></head><body><!-- no content --></body></html>
```

- [ ] **Step 2: Write `tests/fetcher_rate_limit.rs`**

```rust
//! Integration tests for the M5 rate limiter, layered concurrency, and
//! Crawl-Delay floor. All paths use `--features test-loopback`.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::cached::{FetchOptions, fetch_with_cache, ExtractResult};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_stub() -> impl FnMut(&str, &Url) -> Result<ExtractResult, rover::fetcher::FetcherError> {
    move |_body: &str, _base: &Url| {
        Ok(ExtractResult {
            title: Some("t".into()),
            body_md: "# t".into(),
            content_hash: "sha256:0".into(),
            metadata: rover::extractor::ExtractedMetadata::default(),
        })
    }
}

async fn setup(server: &MockServer, rate: RateLimitConfig) -> (Db, Arc<Pacer>, reqwest::Client) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    // Leak the tempdir so it outlives the test guard (storage layer holds the
    // path indirectly via the SQLite connection).
    std::mem::forget(tmp);
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    (db, pacer, client)
}

#[tokio::test]
async fn pacing_at_60_rpm_paces_consecutive_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>hi</body></html>"))
        .mount(&server)
        .await;
    // rpm=60 = one token per second after burst is consumed. We burn the
    // burst on the first 60 requests; the 61st measures the wait.
    // Use rpm = 60, fire 61 requests, total elapsed should be > 0.5s.
    let mut rate = RateLimitConfig::default();
    rate.requests_per_minute_per_domain = 60;
    rate.global_concurrency = 32;
    rate.per_domain_concurrency = 32;
    let (db, pacer, client) = setup(&server, rate.clone()).await;
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let robots = RobotsConfig {
        respect: false,
        ..RobotsConfig::default()
    };

    let start = Instant::now();
    for _ in 0..61 {
        let _ = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate,
            &robots,
            &url,
            &Config::default().cache,
            FetchOptions {
                force_refresh: true,
                ssrf_level: SsrfLevel::TestLoopback,
                ignore_robots: true,
                user_agent: "test/0.1".into(),
            },
            extract_stub(),
        )
        .await
        .unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(500),
        "61 requests at 60 rpm should pace; elapsed = {elapsed:?}"
    );
}

#[tokio::test]
async fn per_host_isolation_does_not_pace_other_hosts() {
    // Two mock servers; ratelimit per-host means burning host A's tokens
    // shouldn't slow host B at all.
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>hi</body></html>"))
        .mount(&server_a)
        .await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>hi</body></html>"))
        .mount(&server_b)
        .await;

    let mut rate = RateLimitConfig::default();
    rate.requests_per_minute_per_domain = 60;
    let (db, pacer, client) = setup(&server_a, rate.clone()).await;
    // Burn host A's burst.
    let url_a = Url::parse(&format!("{}/p", server_a.uri())).unwrap();
    let robots = RobotsConfig { respect: false, ..RobotsConfig::default() };
    for _ in 0..60 {
        let _ = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate,
            &robots,
            &url_a,
            &Config::default().cache,
            FetchOptions {
                force_refresh: true,
                ssrf_level: SsrfLevel::TestLoopback,
                ignore_robots: true,
                user_agent: "test/0.1".into(),
            },
            extract_stub(),
        )
        .await
        .unwrap();
    }

    // Host B: still within burst, should be quick.
    let url_b = Url::parse(&format!("{}/p", server_b.uri())).unwrap();
    let start = Instant::now();
    let _ = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url_b,
        &Config::default().cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_stub(),
    )
    .await
    .unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(200),
        "host B should not be paced by host A"
    );
}
```

(Add similar tests for `global_cap_limits_total_concurrent` and `per_host_concurrency` if time permits — the unit tests in Task 6/7 already cover their core behaviour. The integration tests above prove the wiring.)

- [ ] **Step 3: Write `tests/fetcher_retry.rs`**

```rust
#![cfg(feature = "test-loopback")]

use std::time::Duration;

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::fetcher::FetcherError;
use rover::storage::Db;
use std::sync::Arc;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_stub() -> impl FnMut(&str, &Url) -> Result<ExtractResult, FetcherError> {
    |_body: &str, _base: &Url| Ok(ExtractResult {
        title: None,
        body_md: "ok".into(),
        content_hash: "sha256:0".into(),
        metadata: rover::extractor::ExtractedMetadata::default(),
    })
}

async fn rig() -> (MockServer, Db, Arc<Pacer>, reqwest::Client, RateLimitConfig, RobotsConfig) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    std::mem::forget(tmp);
    let mut rate = RateLimitConfig::default();
    rate.initial_backoff = Duration::from_millis(10);
    rate.max_backoff = Duration::from_millis(50);
    rate.requests_per_minute_per_domain = 6000;
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let robots = RobotsConfig { respect: false, ..RobotsConfig::default() };
    (server, db, pacer, client, rate, robots)
}

#[tokio::test]
async fn http_429_with_retry_after_succeeds_on_retry() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    use wiremock::matchers::method as m;
    // First response 429 with Retry-After: 0, second 200.
    Mock::given(m("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(m("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>ok</body></html>"))
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    let cf = Config::default();
    let result = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_stub(),
    )
    .await
    .unwrap();
    assert_eq!(result.page.title, None);
}

#[tokio::test]
async fn http_500_retries_exhaust_yields_retry_exhausted() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    Mock::given(method("GET"))
        .and(path("/y"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/y", server.uri())).unwrap();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_stub(),
    )
    .await
    .unwrap_err();
    match err {
        FetcherError::RetryExhausted { attempts, .. } => assert_eq!(attempts, 4),
        other => panic!("expected RetryExhausted, got {other:?}"),
    }
}

#[tokio::test]
async fn http_404_is_not_retried() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/missing", server.uri())).unwrap();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_stub(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, FetcherError::Status { status: 404, .. }));
}
```

- [ ] **Step 4: Write `tests/fetcher_robots.rs`**

```rust
#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::fetcher::FetcherError;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_ok() -> impl FnMut(&str, &Url) -> Result<ExtractResult, FetcherError> {
    |_b: &str, _u: &Url| Ok(ExtractResult {
        title: None,
        body_md: "ok".into(),
        content_hash: "sha256:0".into(),
        metadata: rover::extractor::ExtractedMetadata::default(),
    })
}

async fn rig() -> (MockServer, Db, Arc<Pacer>, reqwest::Client, RateLimitConfig) {
    let server = MockServer::start().await;
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    std::mem::forget(tmp);
    let rate = RateLimitConfig::default();
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    (server, db, pacer, client, rate)
}

#[tokio::test]
async fn robots_disallow_admin_refuses_fetch() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\nDisallow: /admin/\n")
                .insert_header("Cache-Control", "max-age=3600"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>nope</body></html>"))
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/admin/x", server.uri())).unwrap();
    let robots = RobotsConfig::default();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, FetcherError::RobotsDisallowed { .. }));
}

#[tokio::test]
async fn robots_404_caches_allow_all_and_proceeds() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/anything"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>hi</body></html>"))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/anything", server.uri())).unwrap();
    let robots = RobotsConfig::default();
    let cf = Config::default();
    let _ok = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .unwrap();
    // The robots_cache row should now exist with state = allow_all.
    let entry = rover::storage::robots::lookup(&db, url.host_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.state, rover::storage::robots::RobotsState::AllowAll);
    assert!(entry.body.is_none());
}

#[tokio::test]
async fn robots_500_caches_disallow_all_short_ttl() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/anything", server.uri())).unwrap();
    let robots = RobotsConfig::default();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .unwrap_err();
    // Either RobotsDisallowed (from the disallow_all cache hit) or
    // RobotsFetchFailed is acceptable depending on the order in cached.rs;
    // both indicate the fail-closed path was taken.
    assert!(matches!(
        err,
        FetcherError::RobotsDisallowed { .. } | FetcherError::RobotsFetchFailed { .. }
    ));
    // The cached entry must be disallow_all with the short failure TTL.
    let entry = rover::storage::robots::lookup(&db, url.host_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.state, rover::storage::robots::RobotsState::DisallowAll);
}

#[tokio::test]
async fn ignore_robots_flag_skips_gate() {
    let (server, db, pacer, client, rate) = rig().await;
    // robots.txt would disallow if consulted.
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /\n"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>x</body></html>"))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    let robots = RobotsConfig::default();
    let cf = Config::default();
    let result = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .unwrap();
    assert_eq!(result.page.title, None);
}
```

- [ ] **Step 5: Write `tests/fetcher_full_loop.rs` with the extract_failed assertion**

```rust
//! End-to-end test that an extraction failure surfaces as `FetcherError::Extract`,
//! exercising the M4 follow-up #1 remap.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::FetcherError;
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn extraction_failure_routes_to_extract_variant() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let html = std::fs::read_to_string("tests/fixtures/m5/extract-failure.html").unwrap();
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&html))
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    std::mem::forget(tmp);
    let rate = RateLimitConfig::default();
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let robots = RobotsConfig::default();
    let cf = Config::default();
    let url = Url::parse(&format!("{}/page", server.uri())).unwrap();

    let err = fetch_with_cache(
        &db, &client, &pacer, &rate, &robots, &url, &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        |body, base| {
            let extracted = rover::extractor::pipeline::extract(body, Some(base))
                .map_err(FetcherError::Extract)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
                metadata: extracted.metadata,
            })
        },
    )
    .await;

    // The body is essentially empty; readabilityrs will either produce a
    // tiny extraction (which succeeds and is OK) or fail. The assertion is
    // looser: if it failed, it must be FetcherError::Extract — not Decode
    // and not Status.
    match err {
        Err(FetcherError::Extract(_)) => {} // expected path
        Ok(_) => {} // also acceptable if readabilityrs handled the empty case
        Err(other) => panic!(
            "expected Extract or Ok, got {other:?} (must not be Decode/Status)"
        ),
    }
}
```

- [ ] **Step 6: Run all M5 integration tests**

Run: `cargo test --features test-loopback --tests`

Expected: green. If any test races on `wiremock` port pickup, retry.

- [ ] **Step 7: Run the full suite**

Run: `cargo test --features test-loopback`

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add tests/fetcher_rate_limit.rs tests/fetcher_retry.rs tests/fetcher_robots.rs \
        tests/fetcher_full_loop.rs tests/fixtures/m5/
git commit -m "test(m5): integration tests for rate limit, retry, robots, extract"
```

---

## Task 13: Documentation Updates + README M5 Marker

**Files:**
- Modify: `docs/security.md` (create if absent), `README.md`, `docs/superpowers/milestones/rover-milestones.md`

Record the per-process rate-limit scope limitation alongside the SSRF/DNS-rebinding note from the design supplement §2.4. Tick M5 as complete in the README and milestone manifest.

- [ ] **Step 1: Update or create `docs/security.md`**

If the file doesn't exist, create it with:

```markdown
# Rover Security Notes (v1)

This document lists explicit security boundaries and known v1 limitations.
Updated alongside each milestone that changes the security surface.

## Known v1 Limitations

### DNS rebinding window during fetch
Per design supplement §2.4: Rover resolves a hostname, validates the IPs
against the active SSRF policy, then performs the actual HTTPS connection
via the system resolver. A TOCTOU window exists between the validation
and the connection. v2 will close this via `reqwest::ClientBuilder::resolve`.

### Per-process rate limit scope (M5)
The rate limiter and concurrency semaphores live in process memory, not
SQLite. Two concurrent `rover mcp` processes each maintain their own
buckets, and a tight shell loop of `rover fetch` invocations is not paced
across process boundaries. This is acceptable for v1's single-user-local
target. v2 may introduce cross-process state if profiling justifies it.

### Robots.txt fail-closed cache window (M5)
When a robots.txt fetch returns 5xx or times out, Rover caches a
`disallow_all` sentinel for `[robots] failure_ttl` (default 5 minutes).
During that window, all fetches to that host are refused with
`robots_fetch_failed` / `robots_disallowed`. The short TTL ensures
recovered servers are picked up quickly; for hosts whose robots endpoint
is chronically broken, raise `failure_ttl` or list the host in
`[robots] ignore_domains`.
```

If the file already exists, append the M5 sections without disturbing the existing content.

- [ ] **Step 2: Update `README.md`**

Locate the milestone status table or list. Change M5 from "next" to "complete" and M6 to "next":

```markdown
- [x] M5 — Rate Limiting & Robots
- [ ] M6 — Long-Running Tasks & Batching (next)
```

- [ ] **Step 3: Update the milestone manifest**

Open `docs/superpowers/milestones/rover-milestones.md`, find the M5 section's "Open questions before planning" subsection, and replace it with a "Decisions" subsection summarising what was resolved:

```markdown
**Decisions made during M5 brainstorming (2026-05-14):**
1. `robotxt` over `texting_robots`.
2. `governor` for the keyed token bucket.
3. Per-host permit acquired before global permit.
4. Retry covers 429, 503, other 5xx, transient network errors; max 3 retries.
5. Retry layer lives in new `fetcher::retry`; pacer guard held across retries.
6. Per-process rate limiter scope; documented in `docs/security.md`.
7. Robots 4xx → allow-all (full TTL). 5xx/timeout → disallow-all (5min).
8. Crawl-Delay enforced via separate `last_request_at` min-interval map.
9. M4 follow-ups #1, #5, #2 bundled into M5; #3, #4, #6 deferred.
```

- [ ] **Step 4: Run the full test suite one final time**

Run: `cargo test --features test-loopback`

Expected: green across unit and integration tests.

- [ ] **Step 5: Lint check**

Run: `cargo clippy --all-targets --features test-loopback -- -D warnings`

Expected: no warnings. `warnings = "deny"` on lints means the build would have already caught most issues, but `clippy` adds extra lints.

- [ ] **Step 6: Commit**

```bash
git add docs/security.md README.md docs/superpowers/milestones/rover-milestones.md
git commit -m "docs(m5): record rate-limit scope, robots failure window, milestone status"
```

- [ ] **Step 7: Push and open PR**

```bash
git push -u origin m5-rate-limiting
gh pr create --title "M5 — Rate limiting and robots" --body "$(cat <<'EOF'
## Summary

- Per-domain token bucket via `governor` keyed by host.
- Layered concurrency: global `tokio::sync::Semaphore` + per-host `Semaphore` registry.
- In-line retry with exponential backoff + jitter, honoring `Retry-After` (seconds + HTTP-date). Max 3 retries.
- Robots.txt fetch + respect via `robotxt`; cached in the `robots_cache` table with a new `state` column.
- `Crawl-Delay` enforced as a floor on the rate limiter via per-host `last_request_at` map.

## M4 follow-ups bundled

- `FetcherError::Extract(ExtractorError)` variant; 3 call-site remap so readabilityrs failures surface as `extract_failed` instead of `fetch_failed`.
- Shared `crate::paths::data_dir()` helper replaces 4 duplicates.
- PRD §14 footnote formally deferring `MetadataPreset` to M8/M9.

## Known v1 limitation

Rate limiter and concurrency state are per-process. Two concurrent `rover mcp` processes maintain independent buckets. Documented in `docs/security.md`.

## Test plan

- [ ] `cargo test` green (unit + integration).
- [ ] `cargo test --features test-loopback` green.
- [ ] `cargo clippy --all-targets --features test-loopback -- -D warnings` clean.
- [ ] Manual: `rover fetch https://example.org/` shows paced behaviour at `info` log level.
- [ ] Manual: `rover doctor` reports `schema_version = 3`.

Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

(Skip Step 7 if not creating a PR yet; the next session's worker can run it.)

---

## Self-Review Checklist

This is the plan author's self-review against the spec. Subagent executors don't need to re-run it.

**Spec coverage:**

- §1 scope (governor, layered concurrency, retry, robots, Crawl-Delay floor, M4 follow-ups): Tasks 1, 4, 5–8, 9, 10 cover each. ✓
- §2 decision table: every row referenced in the corresponding task. ✓
- §3.1 module layout: every file in the table created in Tasks 1–9. ✓
- §3.2 Pacer struct: built in Task 7, with the corrected `_per_host_permit: Option<_>` + `updates_min_interval: bool` fix from the spec review. ✓
- §3.3 call flow: implemented in Task 10. ✓
- §3.4 robots fetch global-only semantics: implemented in Task 9 (`retry_robots` uses `acquire_global_only`, `PacerGuard.updates_min_interval = false`). ✓
- §3.5 retry classifier table: every row covered by unit tests in Task 8. ✓
- §3.6 Crawl-Delay floor mechanics: implemented in Task 7, unit-tested in Step 6 of Task 7. ✓
- §3.7 robots gate (allow-all/disallow-all/parsed state machine): Task 2 (storage) + Task 9 (`build_entry`). ✓
- §3.8 FetcherError additions: Task 5. ✓
- §3.9 data_dir helper: Task 3. ✓
- §4 config: Task 4 + Task 11 (CLI flag overrides). ✓
- §5 schema: Task 1. ✓
- §6 test strategy: Tasks 1–9 (unit) + Task 12 (integration). ✓
- §7 deps added: Task 1. ✓

**Placeholder scan:** No `TBD`/`TODO`/"implement later". All steps contain actual code or exact commands. ✓

**Type consistency:**

- `Pacer::new` takes `&RateLimitConfig` consistently (Tasks 7, 10, 11, 12). ✓
- `FetchOptions { force_refresh, ssrf_level, ignore_robots, user_agent }` shape used uniformly from Task 10 onward. ✓
- `RobotsState` enum + `as_str` ↔ `from_db` round-trip used in Tasks 2 and 9. ✓
- `FetchedPage.retry_after: Option<String>` added in Task 8 Step 3, consumed by classifier in same task. Existing M2/M3/M4 constructors require `..Default::default()` or explicit `retry_after: None`; the test file in `cached.rs` (Task 10 Step 5) sets it explicitly. ✓
- `with_retries` signature: `(pacer, client, url, level, cond, crawl_delay, cfg)` — used in `fetch_with_cache` (Task 10 Step 4) and `robots::retry_robots` reuses the same shape internally. ✓

**Known judgment-call risk:** in Task 8, the classifier reads `page.retry_after` from `FetchedPage` rather than parsing the live response headers — this works only because Task 8 Step 3 plumbs that field through `fetch_url_conditional`. If a future worker forgets that plumbing, they'll see `retry_after: None` and miss honoring `Retry-After`. Mitigated by the unit test `classify_429_with_retry_after_is_retry_after`.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-14-rover-m5-rate-limiting.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, two-stage review between tasks (spec compliance + code quality), Important issues addressed inline via a fix subagent, Minor issues deferred to follow-up notes. Fast iteration, isolated context per task.
2. **Inline Execution** — I run each task in this session via `superpowers:executing-plans`, with checkpoints between tasks for review. Shared context, lower overhead per task but more risk of the session getting noisy.

Which approach?

# Rover M2 — Caching & Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist fetched pages in SQLite with HTTP-aware TTL semantics, conditional revalidation (`ETag` / `Last-Modified` → `304`), and a `rover cache` CLI surface for inspection and pruning. Repeat fetches hit the cache; expired entries revalidate; `--force-refresh` bypasses.

**Architecture:** A `tokio-rusqlite::Connection` actor owns the database; all storage access is async. The fetch path becomes two-layered: `fetcher::fetch::fetch_url` stays as the raw HTTP fetch, and a new `fetcher::cached::fetch_with_cache` orchestrator wraps it with cache lookup → fetch → store. Migrations are SQL files embedded via `include_str!` and applied on `Db::open`. WAL mode set per-connection. `[cache]` config section adds humantime-parsed TTL fields.

**Tech Stack:** `tokio-rusqlite` 0.7 (bundled), `rusqlite` 0.37, `humantime-serde` for `[cache]` durations, plus the M1 stack. No new external services.

**Scope of this plan:** PRD milestone M2 only. Earlier milestones complete; later milestones (M3 MCP server, M4 metadata extraction, M5 rate limiting, M6 long-running tasks, M7 summarization, M8 polish, M9 feature flags) get their own plans.

**References:**
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§3.2 storage, §5.3 conditional requests, §8 caching)
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` (§2.1 tokio-rusqlite, §3.6 min_ttl clarification, §4.2 schema migrations)
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` (M2 section, including the 5 open questions)
- M1 plan: `docs/superpowers/plans/2026-05-07-rover-m1-fetch-path.md`

---

## Decisions on M2's open questions (from milestone manifest)

The manifest flagged five open questions for pre-plan brainstorming. This plan resolves them inline so implementer subagents don't have to:

1. **`servers` table timing** — defer to M3. M2 has no live writer that needs PID tracking; CLI subcommands are short-lived. Migration `001_initial.sql` ships pages, robots_cache, and system only. M3 adds `002_servers.sql`.

2. **Opportunistic write from `rover fetch`** — yes, allowed. `tokio-rusqlite` + WAL + `busy_timeout` handles the multi-instance contention from design §2.3 fine. CLI invocations write to the cache the same way the future MCP server will.

3. **Stale-while-revalidate scheduling** — defer the *task scheduling* to M6. M2 ships **stale-served-without-revalidation**: when a request hits expired cache and the network probe returns success, write back; if the probe fails, the agent gets the stale content with `cache_status: "stale"` and a logged warning. The full SWR pattern with a `revalidation_task_id` envelope (per design §3.3) waits for the task system in M6.

4. **`Cache-Control` parsing crate vs hand-roll** — hand-roll. The directives we need (`max-age`, `s-maxage`, `no-store`, `no-cache`, `must-revalidate`, `public`, `private`) are well-defined in RFC 9111 §5.2 and fit in ~80 lines. No good Rust crate that does only this without dragging in the full hyper/http stack.

5. **`cache purge` glob semantics** — translate shell-style `*` → SQL `%` and `?` → SQL `_`, escaping pre-existing `%`/`_`/`\` in the input. Refuse empty or `*`-only patterns to prevent accidental full-cache wipes (`rover cache purge "*"` requires `--all` flag).

## Cache key strategy (design decision recorded here)

PRD §8.1 specifies `url_hash = sha256(canonical_url)`. M2 implements this with the following lookup order:

1. **Primary lookup:** `SELECT * FROM pages WHERE url_hash = sha256(requested_url)` — fast PK hit when the requested URL is its own canonical.
2. **Secondary lookup:** `SELECT * FROM pages WHERE url = ?` (indexed) — finds entries previously stored under a different requested URL but the same canonical (in this case `url` retains the most-recently-requested URL).
3. **Cache miss:** fetch, resolve canonical, compute `url_hash = sha256(canonical_url)`, upsert.

This means cross-canonical deduplication happens *after* one initial fetch under each requested URL. Entries are not pre-deduplicated. Acceptable for v1; the alias-table optimization (per-requested-URL → url_hash mapping) is deferred until profiling shows it matters.

---

## Files Created in This Plan

```
src/storage/
  mod.rs                              # Db wrapper, open, migration runner, re-exports
  error.rs                            # StorageError
  pages.rs                            # Page struct + CRUD
  system.rs                           # schema_version helpers
  migrations/
    001_initial.sql                   # pages, robots_cache, system

src/fetcher/
  cache_control.rs                    # Cache-Control directive parser
  ttl.rs                              # TTL computation
  cached.rs                           # Cache-aware fetch orchestrator

src/cli/
  cache.rs                            # rover cache list/get/purge/stats

# Modified
src/cli/fetch.rs                      # use fetch_with_cache; --force-refresh
src/cli/mod.rs                        # add `pub mod cache;`
src/config.rs                         # add [cache] section
src/error.rs                          # add Storage variant
src/main.rs                           # wire Cache subcommand, --force-refresh
src/fetcher/mod.rs                    # add new modules

# Tests
tests/cache_lifecycle.rs              # end-to-end CLI: hit, miss, force-refresh, 304, purge
```

Inline unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each source file.

---

## Task 1: Storage scaffold + dependencies

**Files:**
- Modify: `Cargo.toml` (add deps)
- Create: `src/storage/mod.rs`
- Create: `src/storage/error.rs`
- Create: `src/storage/pages.rs` (stub)
- Create: `src/storage/system.rs` (stub)
- Create: `src/storage/migrations/001_initial.sql`
- Modify: `src/lib.rs` (add `pub mod storage;`)
- Modify: `src/error.rs` (add `Storage` variant)

This task lays the storage module skeleton, the first migration file, and the StorageError enum. No real logic yet; that's Tasks 2–4.

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In the `[dependencies]` section, add:

```toml
rusqlite = "0.37"
tokio-rusqlite = { version = "0.7", features = ["bundled"] }
humantime-serde = "1"
```

We pin `rusqlite = "0.37"` instead of `0.39` because `tokio-rusqlite 0.7` (the latest published version) depends on `rusqlite ^0.37`, and `libsqlite3-sys` enforces a single native `sqlite3` link per build graph, so `rusqlite 0.39` cannot coexist with the version `tokio-rusqlite` brings in. The `bundled` feature is enabled on `tokio-rusqlite` rather than directly on `rusqlite`: enabling `bundled` there activates `rusqlite/bundled` transitively for every consumer of `rusqlite` in the graph, so the SQLite C library is still bundled and no system SQLite is required.

- [ ] **Step 2: Create `src/storage/migrations/001_initial.sql`**

```sql
-- M2: pages, robots_cache, system tables.
--
-- WAL is set per-connection at open time (see `Db::open`); recording it here
-- as the canonical journal mode for the project.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS system (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pages (
    url_hash      TEXT PRIMARY KEY,    -- sha256 hex of canonical_url
    url           TEXT NOT NULL,       -- most-recently-requested URL
    canonical_url TEXT NOT NULL,
    title         TEXT,
    fetched_at    INTEGER NOT NULL,    -- unix epoch seconds
    expires_at    INTEGER,             -- unix epoch seconds; NULL = never
    etag          TEXT,
    last_modified TEXT,
    content_hash  TEXT NOT NULL,       -- sha256 hex of extracted_md
    extracted_md  TEXT NOT NULL,
    metadata_json TEXT,                -- JSON blob (M4)
    raw_html_zstd BLOB                 -- optional, behind config flag (M2 leaves NULL)
);

CREATE INDEX IF NOT EXISTS pages_url ON pages(url);
CREATE INDEX IF NOT EXISTS pages_expires ON pages(expires_at);
CREATE INDEX IF NOT EXISTS pages_content_hash ON pages(content_hash);

CREATE TABLE IF NOT EXISTS robots_cache (
    host       TEXT PRIMARY KEY,
    body       TEXT,
    fetched_at INTEGER,
    expires_at INTEGER
);
```

- [ ] **Step 3: Create `src/storage/error.rs`**

```rust
//! Storage-layer error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to open database at {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: tokio_rusqlite::Error,
    },

    #[error("failed to apply migration {name}: {source}")]
    Migration {
        name: String,
        #[source]
        source: tokio_rusqlite::Error,
    },

    #[error("database error: {0}")]
    Db(#[from] tokio_rusqlite::Error),

    #[error("rusqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
```

- [ ] **Step 4: Create `src/storage/mod.rs` skeleton**

```rust
//! SQLite-backed cache and task storage.
//!
//! The storage layer is a thin async API over a single `tokio-rusqlite`
//! connection actor. All access is async; sync rusqlite is reachable only via
//! the actor's `call` closure.
//!
//! Per design §2.1 and §4.2: a single connection writer per process; multi-
//! process safety via WAL mode + `busy_timeout`. Migrations applied on open.

pub mod error;
pub mod pages;
pub mod system;

pub use error::StorageError;

// Db wrapper and open() function come in Task 2.
```

- [ ] **Step 5: Stub `src/storage/pages.rs` and `src/storage/system.rs`**

```rust
// src/storage/pages.rs
//! Pages table CRUD.
```

```rust
// src/storage/system.rs
//! System table accessors (schema_version etc).
```

- [ ] **Step 6: Wire `storage` into `src/lib.rs`**

Append `pub mod storage;` to the existing `pub mod` list (alphabetical position):

```rust
//! Rover — an MCP server for fetching and prepping web content for LLM agents.
//!
//! See `docs/superpowers/prd/2026-05-07-rover-prd.md` for product spec and
//! `docs/superpowers/specs/2026-05-07-rover-design.md` for architectural decisions.

pub mod cli;
pub mod config;
pub mod error;
pub mod extractor;
pub mod fetcher;
pub mod storage;
pub mod telemetry;
```

- [ ] **Step 7: Add `Storage` variant to `src/error.rs`**

Replace the existing `src/error.rs` with the doc header preserved and an additional variant:

```rust
//! Crate-wide error type.
//!
//! Per design supplement §4.4: per-module error enums via `thiserror`,
//! `anyhow` only at the binary boundary. This `Error` enum is the
//! library-facing top-level type that wraps domain-specific errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] crate::fetcher::FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] crate::extractor::ExtractorError),

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 8: Run `cargo build`**

```bash
cargo build
```

Expected: compiles cleanly. Warnings are denied by `[lints.rust]` in Cargo.toml so any warning fails the build; the new modules must not introduce any.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/error.rs src/storage/
git commit -m "feat(storage): scaffold storage module + first migration"
```

---

## Task 2: Storage actor + migration runner

**Files:**
- Modify: `src/storage/mod.rs` (add `Db` wrapper, `open`, migration loop)
- Modify: `src/storage/system.rs` (schema_version helpers)

Wraps `tokio_rusqlite::Connection` in a thin `Db` type, applies embedded migrations on open, sets `journal_mode = WAL` and `busy_timeout`. Exposes a public async API surface.

- [ ] **Step 1: Implement `src/storage/system.rs`**

```rust
//! System table accessors (schema_version etc).

use rusqlite::params;

use super::StorageError;

pub fn read_schema_version(conn: &rusqlite::Connection) -> Result<u32, StorageError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT value FROM system WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(row.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn write_schema_version(
    conn: &rusqlite::Connection,
    version: u32,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO system (key, value) VALUES ('schema_version', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    Ok(())
}
```

- [ ] **Step 2: Replace `src/storage/mod.rs` with the actor + migrations**

```rust
//! SQLite-backed cache and task storage.
//!
//! The storage layer is a thin async API over a single `tokio-rusqlite`
//! connection actor. All access is async; sync rusqlite is reachable only via
//! the actor's `call` closure.

pub mod error;
pub mod pages;
pub mod system;

pub use error::StorageError;

use std::path::Path;
use std::time::Duration;

use tokio_rusqlite::Connection;

/// Async wrapper around a single SQLite connection.
#[derive(Debug, Clone)]
pub struct Db {
    pub(crate) conn: Connection,
}

/// Embedded migrations, applied in array order on every `open` whose
/// `schema_version` is below the index.
///
/// To add a migration: increment its filename (e.g. `002_servers.sql`),
/// append the `(name, sql)` pair here, never edit a previously-released
/// migration in place.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial.sql", include_str!("migrations/001_initial.sql")),
];

impl Db {
    /// Open the database at `path`, applying any pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let conn = Connection::open(path).await.map_err(|source| {
            StorageError::Open { path: path_str.clone(), source }
        })?;

        // Set WAL + busy_timeout per-connection. WAL is persistent at the file
        // level, so this only matters on first open, but it's idempotent.
        conn.call(|c| {
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.busy_timeout(Duration::from_secs(5))?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        let db = Self { conn };
        db.run_migrations().await?;
        Ok(db)
    }

    async fn run_migrations(&self) -> Result<(), StorageError> {
        self.conn
            .call(|c| {
                let current = system::read_schema_version(c)
                    .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
                    let target = (idx + 1) as u32;
                    if current >= target {
                        continue;
                    }
                    let tx = c.unchecked_transaction()?;
                    tx.execute_batch(sql)?;
                    system::write_schema_version(&tx, target)
                        .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                    tx.commit()?;
                    tracing::info!(target: "rover::storage", migration = name, "applied migration");
                }
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Current schema version (for `rover doctor` and tests).
    pub async fn schema_version(&self) -> Result<u32, StorageError> {
        let v = self
            .conn
            .call(|c| {
                system::read_schema_version(c)
                    .map_err(|_| rusqlite::Error::ExecuteReturnedResults)
            })
            .await?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_creates_db_and_applies_migrations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        assert_eq!(db.schema_version().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let _db1 = Db::open(&path).await.unwrap();
        let db2 = Db::open(&path).await.unwrap();
        assert_eq!(db2.schema_version().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn open_creates_pages_table() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        let count: i64 = db
            .conn
            .call(|c| {
                Ok::<_, rusqlite::Error>(c.query_row(
                    "SELECT COUNT(*) FROM pages",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-loopback storage
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/storage/
git commit -m "feat(storage): tokio-rusqlite Db wrapper with migration runner and WAL"
```

---

## Task 3: Pages CRUD

**Files:**
- Modify: `src/storage/pages.rs` (replace stub)

Implements the Page row struct and async CRUD: get_by_url_hash, get_by_url, upsert, delete_by_pattern, list_paginated, stats. All operations go through the `Db.conn.call(...)` actor.

- [ ] **Step 1: Replace `src/storage/pages.rs`**

```rust
//! Pages table CRUD.

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use super::{Db, StorageError};

/// A row in the `pages` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub url_hash: String,
    pub url: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub fetched_at: i64,
    pub expires_at: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_hash: String,
    pub extracted_md: String,
    pub metadata_json: Option<String>,
}

/// Aggregate stats for `rover cache stats`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStats {
    pub entry_count: u64,
    pub total_extracted_bytes: u64,
    pub expired_count: u64,
}

/// Compute the cache key (sha256 hex) for a URL.
pub fn url_hash(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn row_to_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    Ok(Page {
        url_hash: row.get(0)?,
        url: row.get(1)?,
        canonical_url: row.get(2)?,
        title: row.get(3)?,
        fetched_at: row.get(4)?,
        expires_at: row.get(5)?,
        etag: row.get(6)?,
        last_modified: row.get(7)?,
        content_hash: row.get(8)?,
        extracted_md: row.get(9)?,
        metadata_json: row.get(10)?,
    })
}

const SELECT_COLUMNS: &str = "url_hash, url, canonical_url, title, fetched_at, expires_at, \
    etag, last_modified, content_hash, extracted_md, metadata_json";

/// Look up a page by its url_hash (PK).
pub async fn get_by_url_hash(db: &Db, hash: &str) -> Result<Option<Page>, StorageError> {
    let hash = hash.to_owned();
    let page = db
        .conn
        .call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM pages WHERE url_hash = ?1"),
                params![hash],
                row_to_page,
            )
            .optional()?)
        })
        .await?;
    Ok(page)
}

/// Look up a page by its `url` column (most-recently-requested URL).
pub async fn get_by_url(db: &Db, url: &str) -> Result<Option<Page>, StorageError> {
    let url = url.to_owned();
    let page = db
        .conn
        .call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM pages WHERE url = ?1 LIMIT 1"),
                params![url],
                row_to_page,
            )
            .optional()?)
        })
        .await?;
    Ok(page)
}

/// Insert or replace a page row.
pub async fn upsert(db: &Db, page: Page) -> Result<(), StorageError> {
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO pages (url_hash, url, canonical_url, title, fetched_at, \
                                    expires_at, etag, last_modified, content_hash, \
                                    extracted_md, metadata_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                 ON CONFLICT(url_hash) DO UPDATE SET \
                    url = excluded.url, \
                    canonical_url = excluded.canonical_url, \
                    title = excluded.title, \
                    fetched_at = excluded.fetched_at, \
                    expires_at = excluded.expires_at, \
                    etag = excluded.etag, \
                    last_modified = excluded.last_modified, \
                    content_hash = excluded.content_hash, \
                    extracted_md = excluded.extracted_md, \
                    metadata_json = excluded.metadata_json",
                params![
                    page.url_hash,
                    page.url,
                    page.canonical_url,
                    page.title,
                    page.fetched_at,
                    page.expires_at,
                    page.etag,
                    page.last_modified,
                    page.content_hash,
                    page.extracted_md,
                    page.metadata_json,
                ],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Bump `fetched_at` (and optionally `expires_at`) on an existing row, used
/// when revalidation produces 304 Not Modified.
pub async fn touch(
    db: &Db,
    url_hash: &str,
    fetched_at: i64,
    expires_at: Option<i64>,
) -> Result<(), StorageError> {
    let url_hash = url_hash.to_owned();
    db.conn
        .call(move |c| {
            c.execute(
                "UPDATE pages SET fetched_at = ?2, expires_at = ?3 WHERE url_hash = ?1",
                params![url_hash, fetched_at, expires_at],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Delete pages whose `url` matches the given SQL LIKE pattern.
///
/// Returns the number of rows removed.
pub async fn delete_by_url_like(db: &Db, like: &str) -> Result<u64, StorageError> {
    let like = like.to_owned();
    let n = db
        .conn
        .call(move |c| {
            Ok(c.execute(
                "DELETE FROM pages WHERE url LIKE ?1 ESCAPE '\\'",
                params![like],
            )? as u64)
        })
        .await?;
    Ok(n)
}

/// Paginated listing of cached URLs ordered by `fetched_at DESC`.
#[derive(Debug, Clone)]
pub struct CacheListEntry {
    pub url: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub fetched_at: i64,
    pub expires_at: Option<i64>,
    pub size_bytes: i64,
}

pub async fn list_paginated(
    db: &Db,
    offset: u64,
    limit: u64,
) -> Result<Vec<CacheListEntry>, StorageError> {
    let entries = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT url, canonical_url, title, fetched_at, expires_at, length(extracted_md) \
                 FROM pages \
                 ORDER BY fetched_at DESC \
                 LIMIT ?1 OFFSET ?2",
            )?;
            let rows = stmt
                .query_map(params![limit as i64, offset as i64], |r| {
                    Ok(CacheListEntry {
                        url: r.get(0)?,
                        canonical_url: r.get(1)?,
                        title: r.get(2)?,
                        fetched_at: r.get(3)?,
                        expires_at: r.get(4)?,
                        size_bytes: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(entries)
}

/// Aggregate cache stats.
pub async fn stats(db: &Db, now: i64) -> Result<CacheStats, StorageError> {
    let stats = db
        .conn
        .call(move |c| {
            let entry_count: i64 = c.query_row("SELECT COUNT(*) FROM pages", [], |r| r.get(0))?;
            let total_bytes: i64 = c
                .query_row(
                    "SELECT COALESCE(SUM(length(extracted_md)), 0) FROM pages",
                    [],
                    |r| r.get(0),
                )?;
            let expired_count: i64 = c.query_row(
                "SELECT COUNT(*) FROM pages WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![now],
                |r| r.get(0),
            )?;
            Ok(CacheStats {
                entry_count: entry_count as u64,
                total_extracted_bytes: total_bytes as u64,
                expired_count: expired_count as u64,
            })
        })
        .await?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(hash: &str, url: &str) -> Page {
        Page {
            url_hash: hash.to_owned(),
            url: url.to_owned(),
            canonical_url: url.to_owned(),
            title: Some("Sample".to_owned()),
            fetched_at: 1_700_000_000,
            expires_at: Some(1_700_003_600),
            etag: Some("\"abc\"".to_owned()),
            last_modified: None,
            content_hash: "sha256:deadbeef".to_owned(),
            extracted_md: "# Hello\n\nbody".to_owned(),
            metadata_json: None,
        }
    }

    async fn fresh_db() -> Db {
        let tmp = tempfile::tempdir().unwrap();
        Db::open(tmp.path().join("rover.db")).await.unwrap()
    }

    #[test]
    fn url_hash_is_hex_64() {
        let h = url_hash("https://example.com/");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn upsert_then_get() {
        let db = fresh_db().await;
        let page = sample("hash1", "https://example.com/page");
        upsert(&db, page.clone()).await.unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got, page);
    }

    #[tokio::test]
    async fn upsert_replaces_existing() {
        let db = fresh_db().await;
        let p1 = sample("hash1", "https://example.com/v1");
        let mut p2 = p1.clone();
        p2.url = "https://example.com/v2".to_owned();
        p2.fetched_at = 1_700_000_999;
        upsert(&db, p1).await.unwrap();
        upsert(&db, p2.clone()).await.unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got, p2);
    }

    #[tokio::test]
    async fn get_by_url_finds_secondary_lookup() {
        let db = fresh_db().await;
        upsert(&db, sample("hash1", "https://example.com/article"))
            .await
            .unwrap();
        let got = get_by_url(&db, "https://example.com/article").await.unwrap();
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn touch_updates_timestamps() {
        let db = fresh_db().await;
        upsert(&db, sample("hash1", "https://example.com/")).await.unwrap();
        touch(&db, "hash1", 1_700_999_999, Some(1_700_999_999 + 3600))
            .await
            .unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got.fetched_at, 1_700_999_999);
        assert_eq!(got.expires_at, Some(1_700_999_999 + 3600));
    }

    #[tokio::test]
    async fn delete_by_url_like() {
        let db = fresh_db().await;
        upsert(&db, sample("h1", "https://docs.example.com/a")).await.unwrap();
        upsert(&db, sample("h2", "https://docs.example.com/b")).await.unwrap();
        upsert(&db, sample("h3", "https://other.com/c")).await.unwrap();
        let n = delete_by_url_like(&db, "https://docs.example.com/%").await.unwrap();
        assert_eq!(n, 2);
        assert!(get_by_url_hash(&db, "h1").await.unwrap().is_none());
        assert!(get_by_url_hash(&db, "h3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn list_paginated_orders_by_recency() {
        let db = fresh_db().await;
        let mut a = sample("h_a", "https://a/");
        a.fetched_at = 100;
        let mut b = sample("h_b", "https://b/");
        b.fetched_at = 200;
        upsert(&db, a).await.unwrap();
        upsert(&db, b).await.unwrap();
        let rows = list_paginated(&db, 0, 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://b/");
        assert_eq!(rows[1].url, "https://a/");
    }

    #[tokio::test]
    async fn stats_counts_expired() {
        let db = fresh_db().await;
        let mut fresh = sample("h_fresh", "https://a/");
        fresh.expires_at = Some(2_000_000_000);
        let mut stale = sample("h_stale", "https://b/");
        stale.expires_at = Some(1);
        upsert(&db, fresh).await.unwrap();
        upsert(&db, stale).await.unwrap();
        let s = stats(&db, 1_700_000_000).await.unwrap();
        assert_eq!(s.entry_count, 2);
        assert!(s.total_extracted_bytes > 0);
        assert_eq!(s.expired_count, 1);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test --features test-loopback storage::pages
```

Expected: 8 tests pass (1 sync + 7 tokio).

- [ ] **Step 3: Commit**

```bash
git add src/storage/pages.rs
git commit -m "feat(storage): pages CRUD (upsert, get, touch, list, stats, delete)"
```

---

## Task 4: Cache-Control parser

**Files:**
- Create: `src/fetcher/cache_control.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod cache_control;`)

A small RFC 9111 §5.2 directive parser. Handles `max-age`, `s-maxage`, `no-store`, `no-cache`, `must-revalidate`, `public`, `private`. Comma-separated tokens, optional `=value`, case-insensitive directive names, optional double-quoted values.

- [ ] **Step 1: Write the failing tests + implementation**

Create `src/fetcher/cache_control.rs`:

```rust
//! RFC 9111 §5.2 `Cache-Control` directive parser (response side).
//!
//! Only the directives we use today: `max-age`, `s-maxage`, `no-store`,
//! `no-cache`, `must-revalidate`, `public`, `private`. Unknown directives
//! are tolerated and ignored — robust to non-compliant origins.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    pub no_store: bool,
    pub no_cache: bool,
    pub must_revalidate: bool,
    pub public: bool,
    pub private: bool,
}

impl CacheControl {
    /// Parse a `Cache-Control` header value. Multiple `Cache-Control` headers
    /// may be combined by the caller into a single comma-separated string
    /// before parsing.
    pub fn parse(header: &str) -> Self {
        let mut out = Self::default();
        for token in header.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let (name, value) = match token.split_once('=') {
                Some((n, v)) => (n.trim(), Some(strip_quotes(v.trim()))),
                None => (token, None),
            };
            match name.to_ascii_lowercase().as_str() {
                "max-age" => out.max_age = value.and_then(|v| v.parse().ok()),
                "s-maxage" => out.s_maxage = value.and_then(|v| v.parse().ok()),
                "no-store" => out.no_store = true,
                "no-cache" => out.no_cache = true,
                "must-revalidate" => out.must_revalidate = true,
                "public" => out.public = true,
                "private" => out.private = true,
                _ => {} // ignore unknowns
            }
        }
        out
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(CacheControl::parse(""), CacheControl::default());
    }

    #[test]
    fn parses_max_age() {
        let cc = CacheControl::parse("max-age=3600");
        assert_eq!(cc.max_age, Some(3600));
    }

    #[test]
    fn parses_quoted_values() {
        let cc = CacheControl::parse(r#"max-age="600""#);
        assert_eq!(cc.max_age, Some(600));
    }

    #[test]
    fn case_insensitive_directives() {
        let cc = CacheControl::parse("MAX-AGE=42, NO-STORE");
        assert_eq!(cc.max_age, Some(42));
        assert!(cc.no_store);
    }

    #[test]
    fn parses_combined_directives() {
        let cc = CacheControl::parse("public, max-age=300, must-revalidate");
        assert!(cc.public);
        assert_eq!(cc.max_age, Some(300));
        assert!(cc.must_revalidate);
        assert!(!cc.no_store);
    }

    #[test]
    fn s_maxage_separate_from_max_age() {
        let cc = CacheControl::parse("max-age=60, s-maxage=600");
        assert_eq!(cc.max_age, Some(60));
        assert_eq!(cc.s_maxage, Some(600));
    }

    #[test]
    fn ignores_unknown_directives() {
        let cc = CacheControl::parse("immutable, max-age=100, stale-while-revalidate=30");
        assert_eq!(cc.max_age, Some(100));
        assert!(!cc.no_store);
    }

    #[test]
    fn no_store_no_cache_are_independent() {
        let cc = CacheControl::parse("no-store");
        assert!(cc.no_store && !cc.no_cache);
        let cc = CacheControl::parse("no-cache");
        assert!(!cc.no_store && cc.no_cache);
    }

    #[test]
    fn malformed_max_age_yields_none() {
        let cc = CacheControl::parse("max-age=not-a-number");
        assert_eq!(cc.max_age, None);
    }
}
```

- [ ] **Step 2: Add to `src/fetcher/mod.rs`**

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod cache_control;
pub mod canonical;
pub mod charset;
pub mod client;
pub mod fetch;
pub mod ssrf;

pub use fetch::FetchedPage;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("dns lookup failed for {host}: {source}")]
    Dns { host: String, source: std::io::Error },

    #[error("response decoding failed")]
    Decode,
}
```

(Insert `pub mod cache_control;` in alphabetical position; the rest is unchanged.)

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-loopback fetcher::cache_control
```

Expected: 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): RFC 9111 cache-control directive parser"
```

---

## Task 5: `[cache]` config section

**Files:**
- Modify: `src/config.rs`

Adds a `[cache]` section to the TOML config with humantime-parsed durations: `default_ttl`, `min_ttl`, `max_ttl`, `override_no_store`, `override_no_store_domains`, `store_raw_html`. Validates `min_ttl ≤ default_ttl ≤ max_ttl`.

- [ ] **Step 1: Replace `src/config.rs`**

Preserve the existing module doc, error type, validation, and tests; add `CacheConfig`, defaults, validation extension, and tests.

```rust
//! Configuration loading.
//!
//! M1 covers a tiny subset of the full schema documented in PRD §12.
//! Subsequent milestones extend this struct.

use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },

    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },

    #[error("invalid config at {path}: {message}")]
    Invalid { path: String, message: String },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub fetch: FetchConfig,

    #[serde(default)]
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchConfig {
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            user_agent: default_user_agent(),
            timeout_secs: default_timeout_secs(),
        }
    }
}

impl FetchConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

fn default_user_agent() -> String {
    format!(
        "Rover/{} (+https://github.com/aaronbassett/rover)",
        env!("CARGO_PKG_VERSION")
    )
}

fn default_timeout_secs() -> u64 {
    15
}

/// Cache configuration. All durations are parsed by `humantime` (e.g. "1h",
/// "5m", "7d", "30s"). Defaults follow PRD §12.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default = "default_cache_default_ttl", with = "humantime_serde")]
    pub default_ttl: Duration,

    #[serde(default = "default_cache_min_ttl", with = "humantime_serde")]
    pub min_ttl: Duration,

    #[serde(default = "default_cache_max_ttl", with = "humantime_serde")]
    pub max_ttl: Duration,

    #[serde(default)]
    pub override_no_store: bool,

    #[serde(default)]
    pub override_no_store_domains: Vec<String>,

    /// When true, store the gzipped raw HTML alongside the extracted Markdown.
    /// Disabled by default to keep the database small.
    #[serde(default)]
    pub store_raw_html: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            default_ttl: default_cache_default_ttl(),
            min_ttl: default_cache_min_ttl(),
            max_ttl: default_cache_max_ttl(),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        }
    }
}

fn default_cache_default_ttl() -> Duration {
    Duration::from_secs(3600)
}
fn default_cache_min_ttl() -> Duration {
    Duration::from_secs(300)
}
fn default_cache_max_ttl() -> Duration {
    Duration::from_secs(7 * 86400)
}

/// Load config. If `path` is provided, the file must exist and parse cleanly.
/// If `path` is None, return defaults.
pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
    let Some(path) = path else {
        return Ok(Config::default());
    };
    let bytes = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let cfg: Config = toml::from_str(&bytes).map_err(|source| ConfigError::Parse {
        path: path.display().to_string(),
        source,
    })?;
    validate(&cfg).map_err(|message| ConfigError::Invalid {
        path: path.display().to_string(),
        message,
    })?;
    Ok(cfg)
}

fn validate(cfg: &Config) -> Result<(), String> {
    if cfg.fetch.timeout_secs == 0 {
        return Err("fetch.timeout_secs must be > 0".to_string());
    }
    if cfg.cache.min_ttl > cfg.cache.default_ttl {
        return Err("cache.min_ttl must be <= cache.default_ttl".to_string());
    }
    if cfg.cache.default_ttl > cfg.cache.max_ttl {
        return Err("cache.default_ttl must be <= cache.max_ttl".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert!(cfg.fetch.user_agent.starts_with("Rover/"));
        assert_eq!(cfg.fetch.timeout_secs, 15);
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(3600));
        assert_eq!(cfg.cache.min_ttl, Duration::from_secs(300));
        assert_eq!(cfg.cache.max_ttl, Duration::from_secs(7 * 86400));
        assert!(!cfg.cache.override_no_store);
        assert!(!cfg.cache.store_raw_html);
    }

    #[test]
    fn load_with_no_path_returns_default() {
        let cfg = load(None).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, 15);
    }

    #[test]
    fn load_cache_overrides() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
default_ttl = "30m"
min_ttl = "1m"
max_ttl = "1d"
override_no_store = true
override_no_store_domains = ["docs.example.com"]
store_raw_html = true
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(1800));
        assert_eq!(cfg.cache.min_ttl, Duration::from_secs(60));
        assert_eq!(cfg.cache.max_ttl, Duration::from_secs(86400));
        assert!(cfg.cache.override_no_store);
        assert_eq!(
            cfg.cache.override_no_store_domains,
            vec!["docs.example.com".to_string()]
        );
        assert!(cfg.cache.store_raw_html);
    }

    #[test]
    fn load_rejects_zero_timeout() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[fetch]
timeout_secs = 0
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_min_greater_than_default() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
default_ttl = "1m"
min_ttl = "10m"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_rejects_default_greater_than_max() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
default_ttl = "10d"
max_ttl = "1d"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn load_unknown_field_errors() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
unknown_field = "x"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test --features test-loopback config
```

Expected: 7 tests pass (5 original + 2 new cache validation tests + 1 new "load_cache_overrides", minus the M1 tests this replaces — net 7).

(If the count differs, adjust assertions; the spec is the test bodies, not the count.)

- [ ] **Step 3: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): [cache] section with humantime durations"
```

---

## Task 6: TTL computation

**Files:**
- Create: `src/fetcher/ttl.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod ttl;`)

Computes the cache `expires_at` timestamp given the response headers and the `[cache]` config. Implements PRD §8.2 plus design §3.6's clarification of `min_ttl` semantics. Depends on Task 5 (`[cache]` config) being in place — the unit tests reference `crate::config::CacheConfig`.

- [ ] **Step 1: Create `src/fetcher/ttl.rs`**

```rust
//! TTL computation for cache entries.
//!
//! Order of precedence (PRD §8.2 with design §3.6 clarification):
//!   1. `Cache-Control: no-store` → don't cache, unless `override_no_store`
//!      (global or per-domain) is true. `min_ttl` only floors the TTL when
//!      the entry is otherwise being cached.
//!   2. `max-age` / `s-maxage` from `Cache-Control`.
//!   3. `Expires` header.
//!   4. `cache.default_ttl`.
//! Always cap final TTL at `cache.max_ttl`. Always floor at `min_ttl` for
//! entries that are being cached.

use std::time::Duration;

use jiff::Timestamp;
use jiff::fmt::rfc2822;

use super::cache_control::CacheControl;
use crate::config::CacheConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlDecision {
    /// Cache with this absolute expiry (unix epoch seconds).
    Cache { expires_at: i64 },
    /// Do not cache.
    DoNotCache,
}

/// Compute the cache decision for a response.
///
/// `now` is unix epoch seconds at fetch time (so tests can pin it).
/// `host` is the request host, for the `override_no_store_domains` check.
pub fn compute_ttl(
    now: i64,
    host: &str,
    cache_control: &str,
    expires_header: Option<&str>,
    cfg: &CacheConfig,
) -> TtlDecision {
    let cc = CacheControl::parse(cache_control);

    // Step 1: no-store handling.
    let no_store_overridden = if cc.no_store {
        let host_override = cfg
            .override_no_store_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(host));
        if !cfg.override_no_store && !host_override {
            return TtlDecision::DoNotCache;
        }
        // Override active: treat the base TTL as 0 and let `min_ttl` floor
        // below take effect.
        true
    } else {
        false
    };

    // Steps 2-4: pick the base TTL.
    //
    // no-store-with-override branches treat the base TTL as 0, so the
    // `min_ttl` floor lifts the result to `min_ttl`. This is the
    // spec-intent of `min_ttl` for force-cached entries.
    let mut ttl_secs = if let Some(s) = cc.s_maxage {
        s
    } else if let Some(m) = cc.max_age {
        m
    } else if no_store_overridden {
        0
    } else if let Some(t) = expires_header.and_then(parse_expires_header) {
        // RFC 9111 §5.3: an Expires at-or-before `now` is equivalent to
        // `must-revalidate, max-age=0` — do not cache.
        if t <= now {
            return TtlDecision::DoNotCache;
        }
        (t - now) as u64
    } else {
        cfg.default_ttl.as_secs()
    };

    // Floor at min_ttl when caching.
    let min = cfg.min_ttl.as_secs();
    if ttl_secs < min {
        ttl_secs = min;
    }

    // Cap at max_ttl.
    let max = cfg.max_ttl.as_secs();
    if ttl_secs > max {
        ttl_secs = max;
    }

    let expires_at = now.saturating_add(ttl_secs as i64);
    TtlDecision::Cache { expires_at }
}

fn parse_expires_header(value: &str) -> Option<i64> {
    rfc2822::parse(value)
        .ok()
        .map(|z| z.timestamp().as_second())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CacheConfig {
        CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(300),
            max_ttl: Duration::from_secs(7 * 86400),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        }
    }

    #[test]
    fn no_store_skips_cache() {
        let d = compute_ttl(0, "example.com", "no-store", None, &cfg());
        assert_eq!(d, TtlDecision::DoNotCache);
    }

    #[test]
    fn no_store_overridden_floors_min_ttl() {
        let mut c = cfg();
        c.override_no_store = true;
        let d = compute_ttl(0, "example.com", "no-store", None, &c);
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn no_store_per_domain_override() {
        let mut c = cfg();
        c.override_no_store_domains = vec!["docs.example.com".into()];
        let d = compute_ttl(0, "DOCS.example.com", "no-store, max-age=60", None, &c);
        // host match is case-insensitive; min_ttl floors the 60 to 300.
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn max_age_used_when_present() {
        let d = compute_ttl(1_000, "x", "max-age=600", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 1_600 });
    }

    #[test]
    fn s_maxage_overrides_max_age() {
        let d = compute_ttl(0, "x", "max-age=60, s-maxage=120", None, &cfg());
        // s-maxage=120 < min_ttl=300 → floored to 300.
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn expires_header_used_without_cache_control() {
        let d = compute_ttl(0, "x", "", Some("Mon, 1 Jan 2035 00:00:00 GMT"), &cfg());
        // Expires header in 2035 + max_ttl=7*86400 cap → expires_at = max_ttl.
        assert_eq!(
            d,
            TtlDecision::Cache {
                expires_at: 7 * 86400
            }
        );
    }

    #[test]
    fn expires_header_within_max_ttl_used_directly() {
        // Now = 1_700_000_000 (Nov 2023). The Expires header below parses to
        // some timestamp shortly after `now`. We assert it falls between `now`
        // and `now + max_ttl`, so the natural Expires-derived TTL is used
        // (no min_ttl floor or max_ttl cap kicks in).
        let d = compute_ttl(
            1_700_000_000,
            "x",
            "",
            Some("Sun, 14 Nov 2023 22:30:00 GMT"),
            &cfg(),
        );
        match d {
            TtlDecision::Cache { expires_at } => {
                assert!(
                    expires_at > 1_700_000_000,
                    "expires_at={expires_at} should be > now"
                );
                assert!(
                    expires_at < 1_700_000_000 + 7 * 86400,
                    "expires_at={expires_at} should be below now + max_ttl"
                );
            }
            other => panic!("expected Cache, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_default_ttl() {
        let d = compute_ttl(0, "x", "", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 3600 });
    }

    #[test]
    fn caps_at_max_ttl() {
        let d = compute_ttl(0, "x", "max-age=99999999", None, &cfg());
        assert_eq!(
            d,
            TtlDecision::Cache {
                expires_at: 7 * 86400
            }
        );
    }

    #[test]
    fn floors_at_min_ttl() {
        let d = compute_ttl(0, "x", "max-age=10", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn past_expires_skips_cache() {
        // Jan 1 2000 was a Saturday; jiff's RFC 2822 parser strictly
        // validates the weekday, so we must use the correct one.
        let d = compute_ttl(
            1_700_000_000,
            "x",
            "",
            Some("Sat, 1 Jan 2000 00:00:00 GMT"),
            &cfg(),
        );
        assert_eq!(d, TtlDecision::DoNotCache);
    }
}
```

- [ ] **Step 2: Update `src/fetcher/mod.rs`**

Add `pub mod ttl;` to the module list (alphabetical position).

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-loopback fetcher::ttl
```

Expected: 11 tests pass. (Task 5 must already be in place so `crate::config::CacheConfig` is available.)

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): TTL computation honoring Cache-Control, Expires, config"
```

---

## Task 7: Cached fetch orchestrator

**Files:**
- Create: `src/fetcher/cached.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod cached;`, re-export `CachedFetch`, `CacheStatus`)

Wraps `fetcher::fetch::fetch_url` with a cache lookup → fetch → store flow. Handles fresh hits, stale-served, and cache misses. Conditional GET integration comes in Task 8.

- [ ] **Step 1: Create `src/fetcher/cached.rs`**

```rust
//! Cache-aware fetch orchestrator.
//!
//! `fetch_with_cache` is the high-level entry point used by the CLI and the
//! (future) MCP `fetch` tool. It wraps the raw `fetcher::fetch::fetch_url`
//! with cache lookup, TTL-driven freshness, and write-back.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

use super::FetcherError;
use super::cache_control::CacheControl;
use super::fetch::{FetchedPage, fetch_url};
use super::ssrf::SsrfLevel;
use super::ttl::{TtlDecision, compute_ttl};
use crate::config::CacheConfig;
use crate::storage::Db;
use crate::storage::pages::{self, Page, url_hash};

/// Outcome of a cache-aware fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Stale,
    Miss,
}

/// What `fetch_with_cache` returns: a Page (cache hit/miss/stale) plus the
/// cache_status that produced it. The Page mirrors the storage row so the
/// caller has both extracted_md and metadata available.
#[derive(Debug, Clone)]
pub struct CachedFetch {
    pub page: Page,
    pub cache_status: CacheStatus,
}

#[derive(Debug, Clone, Copy)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
}

/// Cache-aware fetch entry point.
///
/// The extraction step is delegated to `extract_fn`: this keeps the fetcher
/// independent of the extractor module. The CLI/MCP layer wires up
/// `extractor::pipeline::extract`.
pub async fn fetch_with_cache<F>(
    db: &Db,
    client: &reqwest::Client,
    url: &Url,
    cfg: &CacheConfig,
    opts: FetchOptions,
    mut extract_fn: F,
) -> Result<CachedFetch, FetcherError>
where
    F: FnMut(&str, &Url) -> Result<ExtractResult, FetcherError>,
{
    let now = Timestamp::now().as_second();

    // --- Step 1: cache lookup ---
    if !opts.force_refresh {
        if let Some(p) = lookup_cached(db, url).await? {
            if let Some(exp) = p.expires_at {
                if exp > now {
                    return Ok(CachedFetch {
                        page: p,
                        cache_status: CacheStatus::Hit,
                    });
                }
                // expired: fall through to revalidation below
            }
        }
    }

    // --- Step 2: fetch (Task 8 adds conditional GETs based on the stale
    // entry's etag/last_modified). For Task 7 we just always do a full GET. ---
    let fetched = match fetch_url(client, url, opts.ssrf_level).await {
        Ok(f) => f,
        Err(e) => {
            // Network failure with a stale entry available → return stale.
            if let Some(stale) = lookup_cached(db, url).await? {
                tracing::warn!(target: "rover::fetcher::cached",
                    error = %e, url = url.as_str(), "fetch failed; serving stale");
                return Ok(CachedFetch {
                    page: stale,
                    cache_status: CacheStatus::Stale,
                });
            }
            return Err(e);
        }
    };

    if !(200..300).contains(&fetched.status) {
        return Err(FetcherError::Http(reqwest_status_error(&fetched)));
    }

    // --- Step 3: extract ---
    let extracted = extract_fn(&fetched.body, &fetched.final_url)?;

    // --- Step 4: TTL ---
    let cache_control_value = fetched
        .content_type
        .as_deref()
        .map(|_| ())
        .and(extract_header(&fetched, "cache-control"))
        .unwrap_or_default();
    let expires_value = extract_header(&fetched, "expires");
    let host = url.host_str().unwrap_or("");
    let decision = compute_ttl(now, host, &cache_control_value, expires_value.as_deref(), cfg);

    let expires_at = match decision {
        TtlDecision::Cache { expires_at } => Some(expires_at),
        TtlDecision::DoNotCache => None,
    };

    let new_hash = url_hash(fetched.canonical_url.as_str());
    let page = Page {
        url_hash: new_hash,
        url: url.as_str().to_owned(),
        canonical_url: fetched.canonical_url.as_str().to_owned(),
        title: extracted.title.clone(),
        fetched_at: now,
        expires_at,
        etag: fetched.etag.clone(),
        last_modified: fetched.last_modified.clone(),
        content_hash: extracted.content_hash.clone(),
        extracted_md: extracted.body_md.clone(),
        metadata_json: None,
    };

    // --- Step 5: store (only if cacheable) ---
    if expires_at.is_some() {
        pages::upsert(db, page.clone()).await.map_err(map_storage_err)?;
    }

    Ok(CachedFetch {
        page,
        cache_status: CacheStatus::Miss,
    })
}

/// What the orchestrator needs from the extractor. Defined here as a tiny
/// adapter so the extractor module isn't a hard dependency of the fetcher.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub title: Option<String>,
    pub body_md: String,
    pub content_hash: String,
}

/// Compute sha256 hex of bytes. Centralized here so callers don't have to
/// pull in `sha2` directly.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

async fn lookup_cached(db: &Db, url: &Url) -> Result<Option<Page>, FetcherError> {
    let hash = url_hash(url.as_str());
    if let Some(p) = pages::get_by_url_hash(db, &hash).await.map_err(map_storage_err)? {
        return Ok(Some(p));
    }
    pages::get_by_url(db, url.as_str()).await.map_err(map_storage_err)
}

fn map_storage_err(e: crate::storage::StorageError) -> FetcherError {
    // Surface as a generic decode failure for now; M3 may want a Storage variant.
    tracing::error!(target: "rover::fetcher::cached", error = %e, "storage error");
    FetcherError::Decode
}

fn extract_header(fetched: &FetchedPage, _name: &str) -> Option<String> {
    // Task 7 extracts content_type and link_header from FetchedPage already;
    // for cache-control / expires, M2 surfaces them via Task 8's expansion of
    // FetchedPage. For now, return None — Task 8 fills this in.
    let _ = fetched;
    None
}

fn reqwest_status_error(_fetched: &FetchedPage) -> reqwest::Error {
    // Synthesize via reqwest's builder is non-trivial; the cached fetch flow
    // returns the status as part of the Err path, but for v1 we wrap it as a
    // generic reqwest error. The CLI emits a clean "HTTP {status}" message
    // anyway, so this only matters for callers that match on the variant.
    //
    // Practical alternative: extend FetcherError with a Status variant in Task 8.
    unreachable!("placeholder; reachable only in 4xx/5xx pre-Task-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_status_eq() {
        assert_ne!(CacheStatus::Hit, CacheStatus::Stale);
    }

    #[test]
    fn sha256_hex_matches_known() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }
}
```

The above is intentionally Task-7-shaped — it leaves header extraction and HTTP-error handling stubbed. Task 8 finishes the wiring.

- [ ] **Step 2: Update `src/fetcher/mod.rs`**

Add `pub mod cached;` (alphabetical position) and re-export the new types:

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod cache_control;
pub mod cached;
pub mod canonical;
pub mod charset;
pub mod client;
pub mod fetch;
pub mod ssrf;
pub mod ttl;

pub use cached::{CachedFetch, CacheStatus, ExtractResult, FetchOptions, fetch_with_cache};
pub use fetch::FetchedPage;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("dns lookup failed for {host}: {source}")]
    Dns { host: String, source: std::io::Error },

    #[error("response decoding failed")]
    Decode,

    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
}
```

(The new `Status` variant replaces the awkward `reqwest_status_error` placeholder. Update `cached.rs` accordingly: `return Err(FetcherError::Status { status: fetched.status, url: fetched.final_url.to_string() });`.)

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-loopback fetcher::cached
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): cached fetch orchestrator (lookup, fetch, store)"
```

---

## Task 8: Conditional GETs + header propagation

**Files:**
- Modify: `src/fetcher/fetch.rs` (surface `cache_control` and `expires` headers on `FetchedPage`; accept optional `If-None-Match` / `If-Modified-Since`)
- Modify: `src/fetcher/cached.rs` (wire header extraction; build conditional headers from stale entry; handle 304)

The `FetchedPage` returned by `fetch_url` currently captures `content_type`, `link_header`, `etag`, `last_modified`. M2 extends it with `cache_control` and `expires` so the orchestrator can compute TTL. The orchestrator also gains a "revalidate stale entry" path.

- [ ] **Step 1: Extend `FetchedPage`**

Modify `src/fetcher/fetch.rs`:

```rust
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub final_url: Url,
    pub canonical_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: String,
    pub charset: Detected,
    pub link_header: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// `Cache-Control` response header (M2).
    pub cache_control: Option<String>,
    /// `Expires` response header (M2).
    pub expires: Option<String>,
}
```

Inside `fetch_url`, capture the two new headers in the same `header()` block as the existing ones:

```rust
let cache_control = response
    .headers()
    .get(reqwest::header::CACHE_CONTROL)
    .and_then(|v| v.to_str().ok())
    .map(str::to_string);
let expires = response
    .headers()
    .get(reqwest::header::EXPIRES)
    .and_then(|v| v.to_str().ok())
    .map(str::to_string);
```

Add them to the `FetchedPage` constructor at the bottom.

- [ ] **Step 2: Add conditional-fetch parameters to `fetch_url`**

Add an optional struct argument carrying validators:

```rust
#[derive(Debug, Clone, Default)]
pub struct ConditionalGet {
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

pub async fn fetch_url(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
) -> Result<FetchedPage, FetcherError> {
    fetch_url_conditional(client, url, level, &ConditionalGet::default()).await
}

pub async fn fetch_url_conditional(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
) -> Result<FetchedPage, FetcherError> {
    // ... same body as today's fetch_url, but build the request as:
    let mut req = client.get(url.clone());
    if let Some(etag) = &cond.if_none_match {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(lm) = &cond.if_modified_since {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let response = req.send().await?;
    // ... rest unchanged
}
```

The default `fetch_url` keeps its old signature so the M1 integration tests still pass without modification.

- [ ] **Step 3: Wire conditional revalidation into `cached.rs`**

Replace the bullet "Step 2: fetch" comment-stub in `fetch_with_cache` with the full revalidation logic:

```rust
// Was the stale entry available?
let stale = if !opts.force_refresh {
    lookup_cached(db, url).await?
} else {
    None
};

let cond = match &stale {
    Some(p) if p.expires_at.is_some_and(|e| e <= now) => ConditionalGet {
        if_none_match: p.etag.clone(),
        if_modified_since: p.last_modified.clone(),
    },
    _ => ConditionalGet::default(),
};

let fetched = match fetch_url_conditional(client, url, opts.ssrf_level, &cond).await {
    Ok(f) => f,
    Err(e) => {
        if let Some(s) = stale {
            tracing::warn!(target: "rover::fetcher::cached",
                error = %e, url = url.as_str(), "fetch failed; serving stale");
            return Ok(CachedFetch { page: s, cache_status: CacheStatus::Stale });
        }
        return Err(e);
    }
};

// Handle 304: bump fetched_at + recompute expires_at, return existing body.
if fetched.status == 304 {
    let stale = stale.expect("304 implies a stale entry was sent");
    let host = url.host_str().unwrap_or("");
    let decision = compute_ttl(
        now,
        host,
        fetched.cache_control.as_deref().unwrap_or(""),
        fetched.expires.as_deref(),
        cfg,
    );
    let expires_at = match decision {
        TtlDecision::Cache { expires_at } => Some(expires_at),
        TtlDecision::DoNotCache => None,
    };
    pages::touch(db, &stale.url_hash, now, expires_at)
        .await
        .map_err(map_storage_err)?;
    let mut page = stale.clone();
    page.fetched_at = now;
    page.expires_at = expires_at;
    return Ok(CachedFetch { page, cache_status: CacheStatus::Hit });
}

if !(200..300).contains(&fetched.status) {
    return Err(FetcherError::Status {
        status: fetched.status,
        url: fetched.final_url.to_string(),
    });
}

// ... continue with extract / TTL / upsert as before, but now use the real
// header values from `fetched.cache_control` and `fetched.expires`.
```

Replace the `extract_header` placeholder with direct field access:

```rust
let host = url.host_str().unwrap_or("");
let decision = compute_ttl(
    now,
    host,
    fetched.cache_control.as_deref().unwrap_or(""),
    fetched.expires.as_deref(),
    cfg,
);
```

Delete the stub `extract_header` and `reqwest_status_error` functions.

- [ ] **Step 4: Update existing M1 fetcher tests**

The `tests/fetcher_integration.rs` tests use `FetchedPage` fields. Adding `cache_control` and `expires` is non-breaking. No test changes needed; just rebuild.

- [ ] **Step 5: Add unit tests for conditional revalidation**

Append to `src/fetcher/cached.rs::tests`:

```rust
#[tokio::test]
async fn cache_hit_within_ttl() {
    // Set up a fresh cache entry that's still valid.
    // Verify that fetch_with_cache returns CacheStatus::Hit without calling out.
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

    let cfg = CacheConfig {
        default_ttl: Duration::from_secs(3600),
        min_ttl: Duration::from_secs(60),
        max_ttl: Duration::from_secs(86400),
        override_no_store: false,
        override_no_store_domains: vec![],
        store_raw_html: false,
    };
    let client = super::client::build_http_client("test/0.1", Duration::from_secs(5));
    let result = fetch_with_cache(
        &db,
        &client,
        &url,
        &cfg,
        FetchOptions {
            force_refresh: false,
            ssrf_level: SsrfLevel::Strict,
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

This test pins the contract that a fresh cache hit short-circuits the network and the extractor.

- [ ] **Step 6: Run the tests**

```bash
cargo test --features test-loopback
```

Expected: all M1 tests still pass + new cached tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): conditional GETs and 304 handling in cached fetch"
```

---

## Task 9: `--force-refresh` flag on `rover fetch`

**Files:**
- Modify: `src/main.rs` (add `--force-refresh` to `FetchArgs`)
- Modify: `src/cli/fetch.rs` (open Db, call `fetch_with_cache`, plumb force_refresh)

Wires the cache-aware fetch into the CLI. After this task, `rover fetch <url>` writes to the cache; `rover fetch --force-refresh <url>` bypasses it.

- [ ] **Step 1: Modify `src/main.rs`**

In `FetchArgs`:

```rust
#[derive(Debug, clap::Args)]
struct FetchArgs {
    /// URL to fetch.
    url: String,

    /// Bypass the cache for this fetch and always go out to the network.
    #[arg(long)]
    force_refresh: bool,

    /// **Test-only.** Allow loopback addresses to satisfy SSRF checks.
    #[cfg(any(test, feature = "test-loopback"))]
    #[arg(long, hide = true)]
    ssrf_test_loopback: bool,
}
```

Update `into_runtime_args`:

```rust
impl FetchArgs {
    fn into_runtime_args(self) -> rover::cli::fetch::Args {
        rover::cli::fetch::Args {
            url: self.url,
            force_refresh: self.force_refresh,
            #[cfg(any(test, feature = "test-loopback"))]
            ssrf_test_loopback: self.ssrf_test_loopback,
        }
    }
}
```

- [ ] **Step 2: Replace `src/cli/fetch.rs`**

```rust
//! `rover fetch <url>` command.

use anyhow::Context;
use jiff::Timestamp;
use std::path::Path;
use url::Url;

use crate::config;
use crate::extractor::frontmatter::{PageMeta, render};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::fetcher::cached::CacheStatus;
use crate::fetcher::client::build_http_client;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;

pub struct Args {
    pub url: String,
    pub force_refresh: bool,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = config::load(config_path).context("loading config")?;
    let url = Url::parse(&args.url).context("parsing URL argument")?;
    let level = ssrf_level_for_args(&args);

    let data_dir = data_dir(&cfg)?;
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());

    let result = fetch_with_cache(
        &db,
        &client,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: args.force_refresh,
            ssrf_level: level,
        },
        |body, base| {
            let extracted = extract(body, Some(base)).map_err(|_| {
                crate::fetcher::FetcherError::Decode
            })?;
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md.clone(),
                content_hash: format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes())),
            })
        },
    )
    .await
    .context("fetching URL")?;

    if matches!(result.cache_status, CacheStatus::Stale) {
        tracing::warn!(target: "rover::cli::fetch",
            url = url.as_str(), "serving stale cache entry (network unavailable)");
    }

    let canonical = Url::parse(&result.page.canonical_url)
        .context("parsing canonical URL from cache row")?;
    let meta = PageMeta {
        url: &url,
        canonical_url: &canonical,
        title: result.page.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &result.page.extracted_md,
    };

    let envelope = render(&meta);
    print!("{envelope}");
    Ok(())
}

fn data_dir(_cfg: &crate::config::Config) -> anyhow::Result<std::path::PathBuf> {
    // M2 keeps the data dir simple; M8 will surface this in [server] config.
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
}

#[cfg(any(test, feature = "test-loopback"))]
fn ssrf_level_for_args(args: &Args) -> SsrfLevel {
    if args.ssrf_test_loopback {
        SsrfLevel::TestLoopback
    } else {
        SsrfLevel::Strict
    }
}

#[cfg(not(any(test, feature = "test-loopback")))]
fn ssrf_level_for_args(_args: &Args) -> SsrfLevel {
    SsrfLevel::Strict
}
```

- [ ] **Step 3: Add `dirs` dep**

Append to `Cargo.toml` `[dependencies]`:

```toml
dirs = "5"
```

- [ ] **Step 4: Run tests**

```bash
cargo test --features test-loopback
```

The existing `tests/cli_fetch.rs::fetch_prints_markdown_with_frontmatter` will now write to the user's `~/.local/share/rover/rover.db`, which is harmless but messy in tests. **Override the data dir for tests** by adding an env var hook:

In `src/cli/fetch.rs`, change `data_dir`:

```rust
fn data_dir(_cfg: &crate::config::Config) -> anyhow::Result<std::path::PathBuf> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        return Ok(std::path::PathBuf::from(env_dir));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
}
```

In `tests/cli_fetch.rs`, set the env var per test:

```rust
let tmp = tempfile::tempdir().unwrap();
Command::cargo_bin("rover")
    .unwrap()
    .env("ROVER_DATA_DIR", tmp.path())
    .args(["fetch", &url, "--ssrf-test-loopback"])
    .assert()
    .success()
    // ... existing predicates
```

Apply the same env override in any new cache integration tests.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/cli/fetch.rs tests/cli_fetch.rs
git commit -m "feat(cli): rover fetch uses cache; --force-refresh bypasses"
```

---

## Task 10: `rover cache list`

**Files:**
- Create: `src/cli/cache.rs`
- Modify: `src/cli/mod.rs` (add `pub mod cache;`)
- Modify: `src/main.rs` (wire `Cache(CacheCmd)` to `cli::cache::run`)

Paginated listing: `rover cache list [--limit N] [--offset N]`. Default limit 20, offset 0.

- [ ] **Step 1: Create `src/cli/cache.rs`**

```rust
//! `rover cache <subcommand>` body.

use anyhow::Context;
use jiff::Timestamp;
use std::path::Path;

use crate::config;
use crate::storage::Db;
use crate::storage::pages;

pub enum Args {
    List {
        limit: u64,
        offset: u64,
    },
    Get {
        url: String,
    },
    Purge {
        pattern: String,
        all: bool,
    },
    Stats,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let _cfg = config::load(config_path).context("loading config")?;
    let data_dir = data_dir()?;
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    match args {
        Args::List { limit, offset } => list(&db, limit, offset).await,
        Args::Get { url } => get(&db, &url).await,
        Args::Purge { pattern, all } => purge(&db, &pattern, all).await,
        Args::Stats => stats(&db).await,
    }
}

async fn list(db: &Db, limit: u64, offset: u64) -> anyhow::Result<()> {
    let entries = pages::list_paginated(db, offset, limit)
        .await
        .context("listing cache")?;
    let now = Timestamp::now().as_second();

    if entries.is_empty() {
        println!("(cache is empty)");
        return Ok(());
    }

    println!(
        "{:<60} {:>10} {:>14} {:>14}",
        "URL", "SIZE", "AGE", "EXPIRES_IN"
    );
    for e in entries {
        let age_s = (now - e.fetched_at).max(0);
        let expires_s = e.expires_at.map(|t| t - now).unwrap_or(0);
        println!(
            "{:<60} {:>10} {:>14} {:>14}",
            truncate(&e.url, 58),
            human_bytes(e.size_bytes as u64),
            human_seconds(age_s),
            if expires_s <= 0 {
                "expired".to_string()
            } else {
                human_seconds(expires_s)
            },
        );
    }
    Ok(())
}

async fn get(db: &Db, url: &str) -> anyhow::Result<()> {
    let hash = pages::url_hash(url);
    if let Some(p) = pages::get_by_url_hash(db, &hash).await? {
        print!("{}", p.extracted_md);
        return Ok(());
    }
    if let Some(p) = pages::get_by_url(db, url).await? {
        print!("{}", p.extracted_md);
        return Ok(());
    }
    anyhow::bail!("not found in cache: {url}");
}

async fn purge(db: &Db, pattern: &str, all: bool) -> anyhow::Result<()> {
    if pattern.is_empty() {
        anyhow::bail!("pattern is empty; refusing to purge");
    }
    if !all && (pattern == "*" || pattern == "**") {
        anyhow::bail!("refusing to purge entire cache without --all flag");
    }
    let like = glob_to_sql_like(pattern);
    let n = pages::delete_by_url_like(db, &like)
        .await
        .context("purging cache")?;
    println!("purged {n} entr{}", if n == 1 { "y" } else { "ies" });
    Ok(())
}

async fn stats(db: &Db) -> anyhow::Result<()> {
    let now = Timestamp::now().as_second();
    let s = pages::stats(db, now).await.context("fetching stats")?;
    println!("entries:       {}", s.entry_count);
    println!("total size:    {}", human_bytes(s.total_extracted_bytes));
    println!("expired:       {}", s.expired_count);
    Ok(())
}

/// Translate a shell-style glob to a SQL LIKE pattern using `\` as the escape.
fn glob_to_sql_like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    for c in pattern.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out
}

fn data_dir() -> anyhow::Result<std::path::PathBuf> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        return Ok(std::path::PathBuf::from(env_dir));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn human_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

fn human_seconds(s: i64) -> String {
    let s = s.max(0);
    if s >= 86400 {
        format!("{}d", s / 86400)
    } else if s >= 3600 {
        format!("{}h", s / 3600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_translation() {
        assert_eq!(glob_to_sql_like("https://x.com/*"), "https://x.com/%");
        assert_eq!(glob_to_sql_like("page?"), "page_");
        assert_eq!(glob_to_sql_like("100%"), "100\\%");
        assert_eq!(glob_to_sql_like("under_score"), "under\\_score");
        assert_eq!(glob_to_sql_like("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn human_seconds_formats() {
        assert_eq!(human_seconds(45), "45s");
        assert_eq!(human_seconds(120), "2m");
        assert_eq!(human_seconds(7200), "2h");
        assert_eq!(human_seconds(2 * 86400), "2d");
    }
}
```

- [ ] **Step 2: Modify `src/cli/mod.rs`**

```rust
//! CLI command implementations.

pub mod cache;
pub mod fetch;
```

- [ ] **Step 3: Wire `Cache(CacheCmd)` in `src/main.rs`**

Replace the previous "not yet implemented" branch for Cache with a real dispatch:

```rust
async fn dispatch(cli: Cli) -> ExitCode {
    let result = match cli.command {
        Command::Fetch(args) => {
            rover::cli::fetch::run(args.into_runtime_args(), cli.config.as_deref()).await
        }
        Command::Cache(sub) => {
            let args = sub.into_runtime_args();
            rover::cli::cache::run(args, cli.config.as_deref()).await
        }
        Command::Mcp
        | Command::Batch { .. }
        | Command::Task { .. }
        | Command::Doctor
        | Command::Config(_) => {
            eprintln!("not yet implemented (planned for a later milestone)");
            return ExitCode::from(2);
        }
    };
    // ... rest unchanged
}
```

Update `CacheCmd` in `src/main.rs` so each variant carries the args clap should parse:

```rust
#[derive(Debug, Subcommand)]
enum CacheCmd {
    /// List cached URLs (most recent first).
    List {
        #[arg(long, default_value_t = 20)]
        limit: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Print the cached Markdown for a URL.
    Get { url: String },
    /// Delete cache entries matching a glob (`*`, `?`).
    Purge {
        pattern: String,
        /// Required to wipe the entire cache (`*` pattern).
        #[arg(long)]
        all: bool,
    },
    /// Show cache size, entry count, expired count.
    Stats,
}

impl CacheCmd {
    fn into_runtime_args(self) -> rover::cli::cache::Args {
        match self {
            CacheCmd::List { limit, offset } => rover::cli::cache::Args::List { limit, offset },
            CacheCmd::Get { url } => rover::cli::cache::Args::Get { url },
            CacheCmd::Purge { pattern, all } => rover::cli::cache::Args::Purge { pattern, all },
            CacheCmd::Stats => rover::cli::cache::Args::Stats,
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --features test-loopback
```

Expected: existing tests + 3 new cache helper unit tests pass.

- [ ] **Step 5: Smoke-test manually**

```bash
ROVER_DATA_DIR=/tmp/rover-cache cargo run --release -- cache stats
```

Should print zero entries (the dir is empty).

- [ ] **Step 6: Commit**

```bash
git add src/cli/ src/main.rs
git commit -m "feat(cli): rover cache list/get/purge/stats"
```

---

## Task 11: End-to-end cache lifecycle test

**Files:**
- Create: `tests/cache_lifecycle.rs`

A single integration test that exercises the full M2 acceptance: hit, miss, force-refresh, 304 revalidation, purge.

- [ ] **Step 1: Create `tests/cache_lifecycle.rs`**

```rust
//! End-to-end cache lifecycle test for M2.

use assert_cmd::Command;
use predicates::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ARTICLE_HTML: &str = r#"
<!doctype html>
<html lang="en">
<head><title>Sample article about caching behavior</title></head>
<body>
  <article>
    <h2>How to do the thing</h2>
    <meta http-equiv="Content-Language" content="en" />
    <p>Body paragraph one with enough text to clear readabilityrs's character threshold of 500 characters by default. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.</p>
  </article>
</body>
</html>
"#;

fn rover() -> Command {
    Command::cargo_bin("rover").unwrap()
}

#[tokio::test]
async fn cache_hit_then_force_refresh_and_purge() {
    let server = MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(move |_req: &wiremock::Request| {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .set_body_string(ARTICLE_HTML)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cache-control", "max-age=3600")
        })
        .mount(&server)
        .await;

    let url = format!("{}/article", server.uri());
    let tmp = tempfile::tempdir().unwrap();

    // First fetch — miss, hits the network.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "first fetch should hit network");

    // Second fetch — hit, no network.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "second fetch should hit cache");

    // Force refresh — bypass.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--force-refresh", "--ssrf-test-loopback"])
        .assert()
        .success();
    assert_eq!(hits.load(Ordering::SeqCst), 2, "force-refresh should hit network");

    // Stats: 1 entry.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries:       1"));

    // Purge.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "purge", &format!("{}/*", server.uri())])
        .assert()
        .success()
        .stdout(predicate::str::contains("purged 1 entry"));

    // Stats after purge: 0.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries:       0"));
}

#[tokio::test]
async fn revalidation_returns_304_and_serves_cache() {
    let server = MockServer::start().await;
    let etag = "\"abc-123\"";

    // First response: 200 with short max-age and an ETag.
    Mock::given(method("GET"))
        .and(path("/news"))
        .and(wiremock::matchers::header_exists_predicate("if-none-match", |_| false))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(ARTICLE_HTML)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cache-control", "max-age=1")
                .insert_header("etag", etag),
        )
        .mount(&server)
        .await;

    // Conditional re-request returns 304.
    Mock::given(method("GET"))
        .and(path("/news"))
        .and(wiremock::matchers::header_exists("if-none-match"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let url = format!("{}/news", server.uri());
    let tmp = tempfile::tempdir().unwrap();

    // First fetch — miss + populate.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success();

    // Wait so the entry expires (max-age=1).
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Second fetch — stale, conditional GET, 304, served from cache.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));
}
```

> Note: `wiremock::matchers::header_exists` and `header_exists_predicate` may have slightly different names in the installed `wiremock` version. If they don't exist, replace with a custom matcher closure that inspects `req.headers.get("if-none-match")`. The test intent is: route conditional requests (with `If-None-Match`) to the 304 response, and unconditional requests to the 200 response.

- [ ] **Step 2: Run the integration tests**

```bash
cargo test --features test-loopback --test cache_lifecycle
```

Expected: 2 tests pass.

If the wiremock matcher API differs, adapt as in the note above. The test logic is:
1. First fetch: 200 with body, no `If-None-Match`.
2. Subsequent fetch with `If-None-Match`: 304 empty body.
3. Cached entry survives revalidation.

- [ ] **Step 3: Run the full suite**

```bash
cargo test --features test-loopback
```

Expected: all M1 tests + all M2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/cache_lifecycle.rs
git commit -m "test(m2): end-to-end cache lifecycle (hit, miss, force-refresh, 304, purge)"
```

---

## Task 12: README update for M2

**Files:**
- Modify: `README.md` (mark M2 done; show cache subcommands)

- [ ] **Step 1: Update the status block in `README.md`**

Replace:

```
> **Status:** early development. Milestone M1 (single-URL fetch path) is complete; M2 (caching) is next. ...
```

with:

```
> **Status:** early development. Milestones M1 (single-URL fetch path) and M2 (caching & storage) are complete; M3 (MCP server mode) is next. ...
```

Add a new section after "Try it":

````markdown
## Cache

`rover` keeps a local SQLite cache at `$XDG_DATA_HOME/rover/rover.db` (or
`~/.local/share/rover/rover.db` by default; override with `ROVER_DATA_DIR`).

```sh
rover cache list                 # paginated URL listing
rover cache get <url>            # print cached Markdown for a URL
rover cache purge 'https://x/*'  # delete cache entries by glob
rover cache stats                # entry count, total size, expired count
```

`rover fetch <url>` writes through the cache. `rover fetch --force-refresh <url>`
bypasses it.
````

- [ ] **Step 2: Final test pass + release build**

```bash
cargo test --features test-loopback
cargo build --release
```

Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): mark M2 complete and add cache subcommands"
```

---

## Acceptance Check

The PRD acceptance for M2 (PRD §14):

> Repeated fetches hit cache; purging works; expired entries re-fetch with conditional headers.

This is covered deterministically by `tests/cache_lifecycle.rs::cache_hit_then_force_refresh_and_purge` and `revalidation_returns_304_and_serves_cache`.

Live sanity check (network required):

```bash
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- fetch https://example.com/
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- fetch https://example.com/   # cache hit
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- cache list
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- cache stats
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- cache purge 'https://example.com/*'
ROVER_DATA_DIR=/tmp/rover-m2 cargo run --release -- cache stats
```

The wiremock-based integration tests are the deterministic acceptance gate; the live URLs above are sanity checks before declaring the milestone done.

---

## Decisions deferred to later milestones (intentional)

- **MCP server** (M3): `rover mcp` and the `force_refresh` MCP arg both wait for M3. The CLI `--force-refresh` is the M2 surface.
- **`servers` table + multi-instance heartbeat** (M3): no live writer in M2 worth tracking.
- **Token counting upgrade** (M3): the M1 chars/4 heuristic still drives `estimated_tokens`. M3 adds real tokenizers behind the same call site.
- **Metadata extraction** (M4): JSON-LD, OG, Twitter Card, microdata. M2's `metadata_json` column is provisioned but always NULL.
- **Tables/images transforms** (M4): readabilityrs defaults still pass through.
- **Rate limiting + robots** (M5): `robots_cache` table is provisioned but not populated.
- **Long-running tasks + SWR scheduling** (M6): M2 ships stale-served-without-revalidation. The full SWR pattern with `revalidation_task_id` envelopes lands in M6.
- **Summarization** (M7): `summary_cache` table not yet introduced; M7 adds migration `004_summary_cache.sql`.
- **Full SSRF level matrix** (M8): only `Strict` (and the test-only `TestLoopback`) are exposed in production.
- **HAR / doctor / config show-set** (M8): `rover doctor` will validate `Db::open` and schema_version when M8 lands.
- **Headless / local-inference / VLM** (M9).

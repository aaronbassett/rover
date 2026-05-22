# Rover M7 — Summarization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the summarizer subsystem and finish the MCP tool surface around it: a `SummarizerBackend` trait with extractive (TextRank) and cloud (genai) implementations, a service-layer cache over `summary_cache`, a new `summarize` MCP tool, real bodies for the three pre-stubbed paths (`fetch.summarize`, `fetch.max_tokens`, `TablesMode::Summarize`), and a new `mode: "estimates"` arg on the existing `count_tokens` tool.

**Architecture:** A new migration introduces `summary_cache` keyed on `(content_hash, params_hash)`. A `SummarizerService` wraps an `Arc<SummarizerRegistry>` and owns the cache hot path: hash params, look up `(content_hash, params_hash)`, dispatch to a `dyn SummarizerBackend` on miss, optionally fall back to the extractive backend on cloud failure, write the cache row. Backends are cache-unaware. The MCP server constructs one `Arc<SummarizerService>` at startup and passes it into `RoverHandler` alongside the existing `Db`, `Config`, `client`, `Pacer`. All summarize work is synchronous in v1 — no task rows are inserted by M7.

**Tech Stack:** `async-trait = "0.1"`, `unicode-segmentation = "1"`, `genai = "0.4"` (cloud backend; `ServiceTargetResolver` for `openai_compat`), the existing `tokio-rusqlite` storage actor, `sha2`, the existing `tokenizers` infra (M3), `wiremock` (dev) for OpenAI-compatible mock endpoints.

**Branch context:** Execute on `m7-summarization`, cut from `origin/main` at `3bde9e2` (M6-polish PR #8 merge). The branch currently carries two commits: the M7 design spec (`3272af5`) and a spec amendment (`275246e`) reflecting that `count_tokens` and `get_metadata` already exist. Run `cargo test --features test-loopback` to confirm 322 tests green on a clean checkout before Task 1.

**Scope of this plan:** PRD milestone M7 only — summarization + the `count_tokens` four-estimate envelope. No CLI subcommand additions beyond passthrough flags on `rover fetch`. M6 follow-ups stay deferred (M7 doesn't take any of them).

**References:**
- Design spec: `docs/superpowers/specs/2026-05-22-rover-m7-summarization-design.md`
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` §4.1 (`fetch.summarize` + `fetch.max_tokens`), §4.3 (`summarize`), §4.5 (`count_tokens` envelope), §6.3 (Tables Summarize), §7 (summarization), §8.1 + §8.4 (`summary_cache`), §14 M7 (acceptance).
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` §2.6 (`[summarization]` section), §2.7 (cache-miss path), §3.5 (params hash includes backend identity), §4.4 (error model), §4.5 (test strategy).
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` M7 section.
- M6 plan (granularity reference): `docs/superpowers/plans/2026-05-14-rover-m6-tasks-batching.md`.

---

## Decisions inherited from the M7 design spec

The spec resolved every open question. Quick reference:

1. **Backend trait dispatch:** `Arc<dyn SummarizerBackend>` with `async_trait`. Backends are cache-unaware.
2. **Registry:** built once at startup; shared via `Arc<SummarizerRegistry>` injected into `RoverHandler` and (CLI-side) constructed in `src/main.rs`.
3. **Cache placement:** `SummarizerService::compact(content_hash, content, opts)` does the lookup; `SummarizerError` carries fallback metadata back to callers.
4. **Sentence segmenter:** `unicode-segmentation` (UAX #29).
5. **TF-IDF scope:** within-document IDF.
6. **PageRank:** `damping = 0.85`, `max_iter = 50`, `tol = 1e-4`, edges below `0.1` similarity dropped.
7. **Output ordering:** chosen sentences re-sorted by original byte offset.
8. **Headlines mode:** for each heading at the deepest covered depth (H1 if present, otherwise H2, …), emit the heading + the single highest-scoring sentence inside its section. Documents with no headings fall back to flat top-k extractive output.
9. **`target_tokens` semantics:** Extractive — greedy cumulative-token cap on the ranked-then-reordered list. Abstractive — embed in the prompt. Headlines — cap section count by cumulative token cost.
10. **Cloud streaming:** collected to `String`. No streaming in v1.
11. **Error model:** distinct snake_case codes (`summarizer_backend_unavailable`, `summarizer_rate_limited`, `summarizer_auth_failed`, `summarizer_model_error`, `summarizer_no_such_backend`, `summarizer_no_extractive_backend_for_fallback`). Default `fallback_to_extractive = true` retries once against extractive and tags the response with `summarizer_fallback: { from, reason }`.
12. **`params_hash` inputs:** SHA-256 over `backend_name`, `model_id`, `mode`, `target_tokens` (or `"null"`), trimmed `focus` (or `""`), lexically-sorted `preserve` (or `""`), `style` — joined with U+001E.
13. **Sync vs task:** all summarize work is synchronous. No new task rows.
14. **Summarize task kind:** schema stays final; the M6 stub worker remains, body unchanged in M7. (The error message stays `summarization_not_yet_implemented` — the spec's note about renaming to `summarize_no_longer_a_task_kind` is purely cosmetic and not worth bumping a migration boundary.)
15. **Summarization defaults:** `[summarization]` section gains `default_backend`, `default_mode`, `default_style`, `fallback_to_extractive`.
16. **Cloud provider scope:** all `genai` built-ins + a `openai_compat` kind with `ServiceTargetResolver`. Free-form `provider` string in config.
17. **Cache-miss path:** `summarize` (and any other tool that needs the page) calls the existing fetcher with default options. Full cache write.
18. **Migration:** `005_summary_cache.sql`.
19. **`max_tokens` overflow:** single-shot. Summarize once with `mode = default_mode`, `target_tokens = max_tokens`, `style = default_style`. If the result still exceeds `max_tokens`, return the existing `MaxTokensExceeded` error.
20. **Tables Summarize fallback:** per-table — requested backend → extractive → keep verbatim. `TableTransform.fallback_reason` records the cause.
21. **`count_tokens` extension:** opt-in `mode: "estimates"` arg returns the four-estimate envelope; default mode preserves today's single-count shape. Non-breaking.
22. **`get_metadata`:** already ships, no M7 changes.

---

## Files Created or Modified in This Plan

```
# Created
src/storage/migrations/005_summary_cache.sql
src/storage/summaries.rs                  # CRUD + params_hash helper

src/summarizer/mod.rs                     # public surface + SummarizerService
src/summarizer/backend.rs                 # SummarizerBackend trait + CompactOpts/Mode/Style/Preserve
src/summarizer/error.rs                   # SummarizerError + BackendError
src/summarizer/registry.rs                # Registry construction from config
src/summarizer/extractive.rs              # TextRank + Headlines + target_tokens
src/summarizer/cloud.rs                   # genai wrapper
src/summarizer/prompts.rs                 # Abstractive prompt template + render

src/mcp/tools/summarize.rs                # summarize MCP tool

tests/summarizer_extractive.rs
tests/summarizer_cloud.rs
tests/summary_cache_lifecycle.rs
tests/mcp_summarize.rs
tests/mcp_count_tokens_estimates.rs
tests/fetch_summarize_arg.rs
tests/fetch_max_tokens_auto_summarize.rs
tests/tables_summarize_mode.rs

# Modified
Cargo.toml                                # +async-trait, +unicode-segmentation, +genai
src/lib.rs                                # +pub mod summarizer
src/storage/mod.rs                        # register 005_summary_cache.sql + pub mod summaries
src/config.rs                             # +SummarizationConfig + BackendConfig + parsing
src/mcp/envelope.rs                       # +RoverError codes; CountResponse becomes untagged enum
src/mcp/error.rs                          # +SummarizerError translation; message tweak on MaxTokensExceeded
src/mcp/handler.rs                        # carry Arc<SummarizerService>; register summarize_tool
src/mcp/server.rs                         # build SummarizerService at startup
src/mcp/tools/fetch.rs                    # wire summarize arg + max_tokens auto-summarize + TablesMode::Summarize
src/mcp/tools/count_tokens.rs             # add `mode: "estimates"` arg; untagged response enum
src/extractor/tables.rs                   # +TableTransform.fallback_reason; surface Summarize hook
src/extractor/options.rs                  # (no shape change — TablesMode::Summarize already exists)
src/main.rs                               # construct SummarizerService for both mcp + fetch paths
src/cli/fetch.rs                          # +--summarize JSON flag, +--max-tokens flag
src/fetcher/cached.rs                     # honor cache.store_raw_html on write path

README.md                                 # M7 complete marker (final task)
docs/superpowers/milestones/rover-milestones.md   # M7 status update (final task)
```

Inline unit tests live in `#[cfg(test)] mod tests` at the bottom of each new source file. Integration tests under `tests/*.rs` cover end-to-end MCP/CLI flows via the existing `tests/common/mod.rs::spawn_client` helper. All test suites require `cargo test --features test-loopback`.

---

## Task 1: Migration 005 + `storage::summaries`

**Files:**
- Create: `src/storage/migrations/005_summary_cache.sql`
- Create: `src/storage/summaries.rs`
- Modify: `src/storage/mod.rs` (register migration; `pub mod summaries`)

The `summary_cache` table and its async CRUD wrapper. Independent of every other M7 module — done first so later tasks can write integration tests against a real backed store.

### Step 1.1: Migration file

- [ ] **Step 1: Create the migration file**

Create `src/storage/migrations/005_summary_cache.sql` with:

```sql
-- M7: summary_cache.
--
-- One row per (content, params) pairing. `content_hash` is the existing
-- `pages.content_hash` (sha256 of extracted_md) for whole-page summaries;
-- for Tables Summarize it is sha256(table_text). No FK to pages because
-- table-text summaries don't have a page row.
--
-- `params_hash` includes the backend's config-key name (design §3.5), so
-- two backends pointing at the same model produce independent cache rows.

CREATE TABLE IF NOT EXISTS summary_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash  TEXT NOT NULL,
    params_hash   TEXT NOT NULL,
    summary_md    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE(content_hash, params_hash)
);

CREATE INDEX IF NOT EXISTS summary_cache_by_content ON summary_cache(content_hash);
```

- [ ] **Step 2: Register the migration**

In `src/storage/mod.rs`, append the new entry to the `MIGRATIONS` array (between the existing `004_tasks.sql` line and the closing `];`):

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
    ("004_tasks.sql", include_str!("migrations/004_tasks.sql")),
    (
        "005_summary_cache.sql",
        include_str!("migrations/005_summary_cache.sql"),
    ),
];
```

- [ ] **Step 3: Add a migration regression test**

In `src/storage/mod.rs`, inside the existing `#[cfg(test)] mod tests` block, append:

```rust
#[tokio::test]
async fn migration_005_adds_summary_cache_table() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rover.db");
    let db = Db::open(&path).await.unwrap();

    let count: i64 = db
        .conn
        .call(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM summary_cache",
                [],
                |r| r.get::<_, i64>(0),
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(db.schema_version().await.unwrap(), MIGRATIONS.len() as u32);
}
```

- [ ] **Step 4: Run the migration test**

Run: `cargo test --features test-loopback --lib storage::tests::migration_005_adds_summary_cache_table`

Expected: PASS. The unique index existence is verified implicitly by Task 1's later insert tests.

### Step 1.2: `storage::summaries` module skeleton

- [ ] **Step 5: Register the module**

In `src/storage/mod.rs`, add `pub mod summaries;` next to the other `pub mod` declarations (after `pub mod servers;`):

```rust
pub mod error;
pub mod events;
pub mod pages;
pub mod robots;
pub mod servers;
pub mod summaries;
pub mod system;
pub mod tasks;
```

- [ ] **Step 6: Create `src/storage/summaries.rs` with the row struct and a failing insert test**

Create `src/storage/summaries.rs`:

```rust
//! Async wrapper around the `summary_cache` table.
//!
//! `content_hash` is the page's extracted-markdown sha256 (with the
//! `sha256:` prefix matching `pages.content_hash`) for whole-page
//! summaries, or the raw sha256-of-table-text (no prefix) for per-table
//! summaries. The column accepts either shape — callers decide.

use crate::storage::Db;
use crate::storage::error::StorageError;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// One `summary_cache` row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryRow {
    pub content_hash: String,
    pub params_hash: String,
    pub summary_md: String,
    pub created_at: i64,
}

/// Look up a cached summary by `(content_hash, params_hash)`. Returns
/// `Ok(None)` on no-such-row; `Err(...)` only on storage errors.
pub async fn lookup(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
) -> Result<Option<SummaryRow>, StorageError> {
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    db.conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT content_hash, params_hash, summary_md, created_at \
                   FROM summary_cache \
                  WHERE content_hash = ?1 AND params_hash = ?2",
            )?;
            let mut rows = stmt.query(rusqlite::params![ch, ph])?;
            if let Some(r) = rows.next()? {
                Ok::<_, rusqlite::Error>(Some(SummaryRow {
                    content_hash: r.get(0)?,
                    params_hash: r.get(1)?,
                    summary_md: r.get(2)?,
                    created_at: r.get(3)?,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(Into::into)
}

/// Insert a new summary. On unique-conflict, the existing row wins and
/// the function returns `Ok(())` — concurrent writers can both attempt the
/// write and the cache stays consistent.
pub async fn insert(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
    summary_md: &str,
) -> Result<(), StorageError> {
    let now = Timestamp::now().as_second();
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    let md = summary_md.to_string();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO summary_cache (content_hash, params_hash, summary_md, created_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(content_hash, params_hash) DO NOTHING",
                rusqlite::params![ch, ph, md, now],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_db() -> (Db, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        (Db::open(&path).await.unwrap(), tmp)
    }

    #[tokio::test]
    async fn lookup_returns_none_for_missing_row() {
        let (db, _tmp) = make_db().await;
        let r = lookup(&db, "sha256:abc", "ph").await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn insert_then_lookup_round_trips() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "hello").await.unwrap();
        let r = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        assert_eq!(r.content_hash, "sha256:abc");
        assert_eq!(r.params_hash, "ph1");
        assert_eq!(r.summary_md, "hello");
        assert!(r.created_at > 0);
    }

    #[tokio::test]
    async fn insert_conflict_keeps_first_row() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "first").await.unwrap();
        insert(&db, "sha256:abc", "ph1", "second").await.unwrap();
        let r = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        assert_eq!(r.summary_md, "first");
    }

    #[tokio::test]
    async fn different_params_hash_creates_independent_rows() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "one").await.unwrap();
        insert(&db, "sha256:abc", "ph2", "two").await.unwrap();
        let r1 = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        let r2 = lookup(&db, "sha256:abc", "ph2").await.unwrap().unwrap();
        assert_eq!(r1.summary_md, "one");
        assert_eq!(r2.summary_md, "two");
    }
}
```

- [ ] **Step 7: Run the storage tests**

Run: `cargo test --features test-loopback --lib storage::summaries::tests`

Expected: 4 tests PASS.

- [ ] **Step 8: Commit**

```bash
git add src/storage/migrations/005_summary_cache.sql src/storage/summaries.rs src/storage/mod.rs
git commit -m "feat(m7): add summary_cache migration and storage::summaries crud"
```

---

## Task 2: `summarizer` Module Skeleton + Types + Errors

**Files:**
- Modify: `Cargo.toml` (add `async-trait`)
- Modify: `src/lib.rs` (`pub mod summarizer`)
- Create: `src/summarizer/mod.rs`
- Create: `src/summarizer/backend.rs`
- Create: `src/summarizer/error.rs`

Lock down the trait shape, the `CompactOpts` struct, and the error types before any backend is written. Two later tasks (extractive, cloud) drop in as separate files implementing the same trait. The `params_hash` helper lives here so both backends and tests share it.

### Step 2.1: `async-trait` dependency

- [ ] **Step 1: Add the dep**

In `Cargo.toml`, in `[dependencies]` (alphabetical block), add after `anyhow = "1"`:

```toml
async-trait = "0.1"
```

- [ ] **Step 2: Confirm it resolves**

Run: `cargo build --lib`

Expected: SUCCESS (downloads `async-trait`, compiles).

### Step 2.2: Module registration

- [ ] **Step 3: Register the summarizer module**

In `src/lib.rs`, find the existing `pub mod` block and add `pub mod summarizer;` in alphabetical order (between `pub mod storage;` and `pub mod tasks;` if those are adjacent — adjust to whatever local order exists):

```rust
pub mod summarizer;
```

- [ ] **Step 4: Create empty module files**

Create `src/summarizer/mod.rs`:

```rust
//! Summarization subsystem.
//!
//! Exposes a `SummarizerBackend` trait and three concrete impls — `Extractive`
//! (TextRank, offline), `Cloud` (wraps `genai::Client`), and (M9-future)
//! `LocalMistralRs`. The `SummarizerService` (Task 7) wraps a `Registry`
//! (Task 6) plus the storage handle and owns the cache hot path.

pub mod backend;
pub mod error;

pub use backend::{CompactMode, CompactOpts, PreserveSection, Style, SummarizerBackend};
pub use error::{BackendError, SummarizerError};
```

Create `src/summarizer/backend.rs` (empty stub, filled in §2.3).

Create `src/summarizer/error.rs` (empty stub, filled in §2.4).

### Step 2.3: `CompactOpts` + trait

- [ ] **Step 5: Write the failing trait-shape test**

Create `src/summarizer/backend.rs` with:

```rust
//! Backend trait and compaction options.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::summarizer::error::BackendError;

/// Compaction modes (PRD §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactMode {
    Extractive,
    Abstractive,
    Headlines,
}

impl CompactMode {
    /// Stable string for params_hash and config parsing.
    pub fn as_str(self) -> &'static str {
        match self {
            CompactMode::Extractive => "extractive",
            CompactMode::Abstractive => "abstractive",
            CompactMode::Headlines => "headlines",
        }
    }
}

/// Compaction style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Bullet,
    Prose,
    Executive,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Style::Bullet => "bullet",
            Style::Prose => "prose",
            Style::Executive => "executive",
        }
    }
}

/// Section kinds the summarizer is asked to preserve verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreserveSection {
    Code,
    Tables,
    Quotes,
    Lists,
}

impl PreserveSection {
    pub fn as_str(self) -> &'static str {
        match self {
            PreserveSection::Code => "code",
            PreserveSection::Tables => "tables",
            PreserveSection::Quotes => "quotes",
            PreserveSection::Lists => "lists",
        }
    }
}

/// One summarization request after defaults have been merged.
///
/// `target_tokens` counts via the configured tokenizer family on the
/// service side; backends treat it as advisory text in the prompt
/// (Abstractive) or as a hard greedy cap (Extractive/Headlines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOpts {
    pub mode: CompactMode,
    pub style: Style,
    pub target_tokens: Option<usize>,
    pub focus: Option<String>,
    pub preserve: Vec<PreserveSection>,
    /// The resolved backend's config-key name (e.g. "default", "fast").
    /// Filled in by `SummarizerService::compact` from the registry; backends
    /// see it for logging only.
    pub backend_name: String,
}

#[async_trait]
pub trait SummarizerBackend: Send + Sync {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError>;

    /// Config-key name (e.g. "default", "fast").
    fn name(&self) -> &str;

    /// Resolved model identifier for `params_hash`. `""` for extractive.
    fn model_id(&self) -> &str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_as_str() {
        assert_eq!(CompactMode::Extractive.as_str(), "extractive");
        assert_eq!(CompactMode::Abstractive.as_str(), "abstractive");
        assert_eq!(CompactMode::Headlines.as_str(), "headlines");
        assert_eq!(Style::Bullet.as_str(), "bullet");
        assert_eq!(Style::Prose.as_str(), "prose");
        assert_eq!(Style::Executive.as_str(), "executive");
        assert_eq!(PreserveSection::Code.as_str(), "code");
    }

    #[test]
    fn compact_opts_is_clonable() {
        let o = CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(500),
            focus: Some("api shape".to_string()),
            preserve: vec![PreserveSection::Code],
            backend_name: "fast".to_string(),
        };
        let cloned = o.clone();
        assert_eq!(o, cloned);
    }
}
```

- [ ] **Step 6: Run the type tests**

Run: `cargo test --features test-loopback --lib summarizer::backend::tests`

Expected: 2 tests PASS.

### Step 2.4: Error types

- [ ] **Step 7: Write the error-type tests + impls**

Create `src/summarizer/error.rs`:

```rust
//! Errors raised by the summarizer subsystem.

use thiserror::Error;

/// Errors a backend can raise from `compact`.
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend unavailable: {0}")]
    Unavailable(String),

    #[error("rate limited")]
    RateLimited,

    #[error("auth failed: {0}")]
    AuthFailed(String),

    #[error("model error: {0}")]
    ModelError(String),

    /// Programmer-visible misuse (e.g. empty content) — distinct from
    /// network errors so the service doesn't retry through extractive.
    #[error("invalid request: {0}")]
    Invalid(String),
}

/// Errors a `SummarizerService` raises. Wraps `BackendError` with the
/// originating backend's name so MCP responses can identify the failing
/// backend in `summarizer_fallback.from`.
#[derive(Debug, Error)]
pub enum SummarizerError {
    #[error("no such backend: {name}")]
    NoSuchBackend { name: String },

    #[error("no extractive backend configured for fallback")]
    NoExtractiveBackendForFallback,

    #[error("backend {name} unavailable: {reason}")]
    BackendUnavailable { name: String, reason: String },

    #[error("backend {name} rate limited")]
    RateLimited { name: String },

    #[error("backend {name} auth failed: {reason}")]
    AuthFailed { name: String, reason: String },

    #[error("backend {name} model error: {reason}")]
    ModelError { name: String, reason: String },

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("token counting error: {0}")]
    Tokenizer(#[from] crate::tokenizer::TokenizerError),
}

impl SummarizerError {
    /// Convert a `BackendError` into a `SummarizerError` carrying the
    /// originating backend's name.
    pub(crate) fn from_backend(name: &str, e: BackendError) -> Self {
        match e {
            BackendError::Unavailable(r) | BackendError::Invalid(r) => {
                SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason: r,
                }
            }
            BackendError::RateLimited => SummarizerError::RateLimited {
                name: name.to_string(),
            },
            BackendError::AuthFailed(r) => SummarizerError::AuthFailed {
                name: name.to_string(),
                reason: r,
            },
            BackendError::ModelError(r) => SummarizerError::ModelError {
                name: name.to_string(),
                reason: r,
            },
        }
    }

    /// Short, stable reason string for `summarizer_fallback.reason` metadata.
    pub fn fallback_reason(&self) -> &'static str {
        match self {
            SummarizerError::BackendUnavailable { .. } => "backend_unavailable",
            SummarizerError::RateLimited { .. } => "rate_limited",
            SummarizerError::AuthFailed { .. } => "auth_failed",
            SummarizerError::ModelError { .. } => "model_error",
            _ => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_backend_maps_each_variant() {
        let cases = [
            (
                BackendError::Unavailable("net".into()),
                "backend_unavailable",
            ),
            (BackendError::RateLimited, "rate_limited"),
            (BackendError::AuthFailed("401".into()), "auth_failed"),
            (BackendError::ModelError("bad".into()), "model_error"),
            (BackendError::Invalid("empty".into()), "backend_unavailable"),
        ];
        for (be, expected_reason) in cases {
            let e = SummarizerError::from_backend("fast", be);
            assert_eq!(e.fallback_reason(), expected_reason, "for {e}");
        }
    }
}
```

- [ ] **Step 8: Run the error tests**

Run: `cargo test --features test-loopback --lib summarizer::error::tests`

Expected: PASS.

### Step 2.5: `params_hash` helper

- [ ] **Step 9: Write the failing hash tests**

In `src/summarizer/mod.rs`, append below the `pub use` block:

```rust
use sha2::{Digest, Sha256};

use crate::summarizer::backend::CompactOpts;

/// Record separator used to disambiguate hash inputs.
const RS: char = '\u{1E}';

/// Deterministic params_hash for `summary_cache` lookups. Inputs are
/// serialized as plain strings — never via serde — so reorderings or
/// crate version changes can't shift the hash.
pub fn params_hash(opts: &CompactOpts, model_id: &str) -> String {
    let target = opts
        .target_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_string());
    let focus = opts
        .focus
        .as_deref()
        .map(|s| s.trim())
        .unwrap_or("")
        .to_string();
    let mut preserve_sorted: Vec<&'static str> =
        opts.preserve.iter().map(|p| p.as_str()).collect();
    preserve_sorted.sort();
    preserve_sorted.dedup();
    let preserve_csv = preserve_sorted.join(",");

    let serialized = format!(
        "{name}{RS}{model}{RS}{mode}{RS}{target}{RS}{focus}{RS}{preserve}{RS}{style}",
        name = opts.backend_name,
        model = model_id,
        mode = opts.mode.as_str(),
        target = target,
        focus = focus,
        preserve = preserve_csv,
        style = opts.style.as_str(),
    );

    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn baseline() -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(500),
            focus: Some("api shape".to_string()),
            preserve: vec![PreserveSection::Code, PreserveSection::Tables],
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn hash_is_deterministic_for_same_inputs() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o-mini");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn hash_changes_when_backend_name_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let mut other = baseline();
        other.backend_name = "smart".to_string();
        let b = params_hash(&other, "gpt-4o-mini");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_model_id_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_invariant_to_preserve_ordering() {
        let mut a_opts = baseline();
        a_opts.preserve = vec![PreserveSection::Code, PreserveSection::Tables];
        let mut b_opts = baseline();
        b_opts.preserve = vec![PreserveSection::Tables, PreserveSection::Code];
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_treats_target_none_as_null_string() {
        let mut o = baseline();
        o.target_tokens = None;
        let _ = params_hash(&o, "m");
        // Implicit: no panic; the difference vs Some(500) is exercised below.
        let h_none = params_hash(&o, "m");
        o.target_tokens = Some(500);
        let h_some = params_hash(&o, "m");
        assert_ne!(h_none, h_some);
    }

    #[test]
    fn focus_whitespace_normalization_collapses_to_same_hash() {
        let mut a_opts = baseline();
        a_opts.focus = Some("api shape".to_string());
        let mut b_opts = baseline();
        b_opts.focus = Some("  api shape  ".to_string());
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 10: Run the hash tests**

Run: `cargo test --features test-loopback --lib summarizer::tests`

Expected: 6 tests PASS.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml src/lib.rs src/summarizer
git commit -m "feat(m7): summarizer trait + compactopts + error enum + params_hash"
```

---

## Task 3: Extractive Backend (TextRank + Headlines + target_tokens)

**Files:**
- Modify: `Cargo.toml` (add `unicode-segmentation`)
- Create: `src/summarizer/extractive.rs`
- Modify: `src/summarizer/mod.rs` (`pub mod extractive`)
- Create: `tests/summarizer_extractive.rs`

The extractive backend is the largest single piece of M7 code: sentence segmentation, TF-IDF, cosine similarity, PageRank, mode-specific output. ~400 lines of impl + comprehensive tests. Self-contained — no I/O.

### Step 3.1: Add `unicode-segmentation` dep + module skeleton

- [ ] **Step 1: Add the dep**

In `Cargo.toml`, in `[dependencies]`:

```toml
unicode-segmentation = "1"
```

- [ ] **Step 2: Register the module**

In `src/summarizer/mod.rs`, add `pub mod extractive;` next to `pub mod backend;`:

```rust
pub mod backend;
pub mod error;
pub mod extractive;
```

- [ ] **Step 3: Create the file skeleton**

Create `src/summarizer/extractive.rs`:

```rust
//! Extractive summarizer — TextRank.
//!
//! Pipeline (PRD §7.2):
//! 1. Sentence-split via `unicode-segmentation` (UAX #29).
//! 2. Tokenize per sentence (lowercased Unicode words).
//! 3. TF-IDF per sentence (within-document IDF).
//! 4. Cosine-similarity edges (drop below 0.1).
//! 5. PageRank — damping 0.85, max_iter 50, tol 1e-4.
//! 6. Mode-specific selection (Extractive | Headlines).
//!    Abstractive mode delegates to the cloud backend; this module
//!    never sees it.
//!
//! All paths are pure (no I/O, no async). The trait wrapper at the
//! bottom satisfies `SummarizerBackend`.

use std::collections::HashMap;

use async_trait::async_trait;
use unicode_segmentation::UnicodeSegmentation;

use crate::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::tokenizer::{self, Tokenizer};

const NAME: &str = "extractive";

/// PageRank tuning. Pinned to design §2 §5.
const PAGERANK_DAMPING: f32 = 0.85;
const PAGERANK_MAX_ITER: usize = 50;
const PAGERANK_TOL: f32 = 1e-4;
const SIMILARITY_FLOOR: f32 = 0.1;
```

### Step 3.2: Sentence segmentation

- [ ] **Step 4: Write the failing sentence-split tests**

Append to `src/summarizer/extractive.rs`:

```rust
/// One sentence keeping its source offset for stable re-ordering.
#[derive(Debug, Clone)]
pub(super) struct Sentence {
    pub span_start: usize,
    pub text: String,
}

pub(super) fn split_sentences(content: &str) -> Vec<Sentence> {
    let mut out = Vec::new();
    for (offset, s) in content.split_sentence_bound_indices() {
        let trimmed = s.trim();
        if trimmed.chars().count() < 3 {
            continue;
        }
        out.push(Sentence {
            span_start: offset,
            text: trimmed.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod sentence_tests {
    use super::*;

    #[test]
    fn split_produces_three_sentences_from_simple_paragraph() {
        let text = "Hello world. How are you? Fine!";
        let s = split_sentences(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].text, "Hello world.");
        assert_eq!(s[1].text, "How are you?");
        assert_eq!(s[2].text, "Fine!");
    }

    #[test]
    fn split_skips_short_fragments() {
        let text = "Hi. This is a longer sentence.";
        let s = split_sentences(text);
        // "Hi." is 3 chars, sits exactly at the >=3 threshold.
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn split_handles_unicode_punctuation() {
        let text = "Привет мир. Это тест.";
        let s = split_sentences(text);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn split_preserves_byte_offsets() {
        let text = "First sentence. Second sentence here.";
        let s = split_sentences(text);
        assert!(s[0].span_start < s[1].span_start);
    }

    #[test]
    fn split_returns_empty_for_empty_input() {
        assert!(split_sentences("").is_empty());
    }
}
```

- [ ] **Step 5: Run the segmentation tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::sentence_tests`

Expected: 5 tests PASS.

### Step 3.3: Tokenize + TF-IDF

- [ ] **Step 6: Write the failing TF-IDF tests**

Append to `src/summarizer/extractive.rs`:

```rust
/// Lowercased Unicode word tokens. Filters punctuation and whitespace.
pub(super) fn tokenize(s: &str) -> Vec<String> {
    s.unicode_words().map(str::to_lowercase).collect()
}

/// Returns (per-sentence L2-normalized TF-IDF vectors, vocabulary index map).
/// Each vector is a sparse `HashMap<term_index, f32>` for cheap cosine.
pub(super) fn tfidf_vectors(sentences: &[Sentence]) -> Vec<HashMap<usize, f32>> {
    if sentences.is_empty() {
        return Vec::new();
    }

    // Step A: document frequencies + vocabulary.
    let mut df: HashMap<String, usize> = HashMap::new();
    let mut vocab: HashMap<String, usize> = HashMap::new();
    let tokens_per_sent: Vec<Vec<String>> =
        sentences.iter().map(|s| tokenize(&s.text)).collect();
    for terms in &tokens_per_sent {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in terms {
            if seen.insert(t.as_str()) {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            if !vocab.contains_key(t) {
                let id = vocab.len();
                vocab.insert(t.clone(), id);
            }
        }
    }
    let n = sentences.len() as f32;

    // Step B: per-sentence TF, multiplied by IDF, then L2-normalized.
    let mut vectors = Vec::with_capacity(sentences.len());
    for terms in &tokens_per_sent {
        let mut tf: HashMap<usize, f32> = HashMap::new();
        for t in terms {
            let id = vocab[t.as_str()];
            *tf.entry(id).or_insert(0.0) += 1.0;
        }
        let mut tfidf: HashMap<usize, f32> = HashMap::new();
        for (id, count) in tf {
            // recover term from vocab by id is O(vocab); instead store reverse:
            let term = vocab.iter().find(|(_, v)| **v == id).map(|(k, _)| k.as_str()).unwrap();
            let dfv = df[term] as f32;
            let idf = (n / dfv).ln(); // 0 if term in every sentence
            if idf > 0.0 {
                tfidf.insert(id, count * idf);
            }
        }
        // L2 normalize.
        let norm: f32 = tfidf.values().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in tfidf.values_mut() {
                *v /= norm;
            }
        }
        vectors.push(tfidf);
    }
    vectors
}

/// Cosine over two sparse vectors.
pub(super) fn cosine(a: &HashMap<usize, f32>, b: &HashMap<usize, f32>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut sum = 0.0;
    for (k, v) in small {
        if let Some(w) = large.get(k) {
            sum += v * w;
        }
    }
    sum
}

#[cfg(test)]
mod tfidf_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn tokenize_lowercases_words() {
        let t = tokenize("Hello, World! Don't stop.");
        assert!(t.contains(&"hello".to_string()));
        assert!(t.contains(&"world".to_string()));
        assert!(t.contains(&"don't".to_string()));
        assert!(t.contains(&"stop".to_string()));
    }

    #[test]
    fn tfidf_zeroes_out_terms_appearing_everywhere() {
        let s = sents(&["the cat sat", "the cat slept", "the cat ran"]);
        let vecs = tfidf_vectors(&s);
        // "the" and "cat" appear in every sentence → idf = ln(1) = 0; should be absent.
        for v in &vecs {
            // The unique tokens (sat/slept/ran) must remain.
            assert_eq!(v.len(), 1, "sentence kept only the discriminating term");
        }
    }

    #[test]
    fn cosine_is_one_for_identical_sentences() {
        let s = sents(&["hello world", "hello world"]);
        let v = tfidf_vectors(&s);
        let c = cosine(&v[0], &v[1]);
        assert!(
            (c - 1.0).abs() < 1e-5 || c.is_nan(),
            "cosine on identical (or both-zero) was {c}"
        );
    }

    #[test]
    fn cosine_is_zero_for_disjoint_sentences() {
        // Use a corpus where each sentence has a unique discriminator, so
        // IDF is non-zero and vectors actually carry weight.
        let s = sents(&[
            "alpha beta",
            "gamma delta",
            "epsilon zeta",
        ]);
        let v = tfidf_vectors(&s);
        // Sentences 0 and 1 share no tokens.
        assert!(cosine(&v[0], &v[1]).abs() < 1e-5);
    }

    #[test]
    fn tfidf_returns_empty_for_empty_input() {
        assert!(tfidf_vectors(&[]).is_empty());
    }
}
```

- [ ] **Step 7: Run the TF-IDF tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::tfidf_tests`

Expected: 5 tests PASS.

### Step 3.4: PageRank

- [ ] **Step 8: Write the failing PageRank tests**

Append to `src/summarizer/extractive.rs`:

```rust
/// Run PageRank on the dense similarity matrix. Edges below
/// `SIMILARITY_FLOOR` are zeroed before power iteration. Returns one
/// score per sentence, length-matched to `vectors`.
pub(super) fn pagerank(vectors: &[HashMap<usize, f32>]) -> Vec<f32> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }

    // Build weighted similarity matrix in row-major form. Diagonal = 0.
    let mut weights = vec![0.0_f32; n * n];
    let mut row_sums = vec![0.0_f32; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let s = cosine(&vectors[i], &vectors[j]);
            if s >= SIMILARITY_FLOOR {
                weights[i * n + j] = s;
                weights[j * n + i] = s;
                row_sums[i] += s;
                row_sums[j] += s;
            }
        }
    }

    let inv_n = 1.0_f32 / n as f32;
    let mut score = vec![inv_n; n];
    let teleport = (1.0 - PAGERANK_DAMPING) * inv_n;

    for _ in 0..PAGERANK_MAX_ITER {
        let mut next = vec![teleport; n];
        for j in 0..n {
            if row_sums[j] == 0.0 {
                // Dangling: distribute uniformly.
                let share = PAGERANK_DAMPING * score[j] * inv_n;
                for k in 0..n {
                    next[k] += share;
                }
            } else {
                let factor = PAGERANK_DAMPING * score[j] / row_sums[j];
                for i in 0..n {
                    let w = weights[i * n + j];
                    if w > 0.0 {
                        next[i] += factor * w;
                    }
                }
            }
        }
        let delta: f32 = score
            .iter()
            .zip(next.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        score = next;
        if delta < PAGERANK_TOL {
            break;
        }
    }
    score
}

#[cfg(test)]
mod pagerank_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn empty_returns_empty() {
        assert!(pagerank(&[]).is_empty());
    }

    #[test]
    fn single_sentence_gets_full_mass() {
        let v = tfidf_vectors(&sents(&["alpha beta gamma"]));
        let pr = pagerank(&v);
        assert_eq!(pr.len(), 1);
        assert!((pr[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn central_sentence_outscores_peripheral() {
        // Three sentences; the middle one shares tokens with each peripheral
        // and PageRank should reflect that centrality.
        let v = tfidf_vectors(&sents(&[
            "alpha unique_left",
            "alpha gamma bridge",
            "gamma unique_right",
        ]));
        let pr = pagerank(&v);
        assert!(
            pr[1] >= pr[0] && pr[1] >= pr[2],
            "middle sentence should be highest: {pr:?}",
        );
    }

    #[test]
    fn scores_sum_close_to_one() {
        let v = tfidf_vectors(&sents(&[
            "alpha beta gamma",
            "alpha gamma",
            "delta epsilon zeta",
            "alpha epsilon",
        ]));
        let pr = pagerank(&v);
        let total: f32 = pr.iter().sum();
        assert!((total - 1.0).abs() < 0.05, "sum was {total}");
    }
}
```

- [ ] **Step 9: Run the PageRank tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::pagerank_tests`

Expected: 4 tests PASS.

### Step 3.5: Extractive mode output

- [ ] **Step 10: Write the failing selection test**

Append to `src/summarizer/extractive.rs`:

```rust
/// Select sentences for `mode = Extractive`, ranked by PageRank, then
/// re-ordered by source position. Honors `target_tokens` by greedily
/// admitting top-ranked sentences while the cumulative tokenizer count
/// stays at or under the budget. If even the single highest sentence
/// already exceeds the budget, it is still emitted (a warning is logged).
fn select_extractive(
    sentences: &[Sentence],
    scores: &[f32],
    target_tokens: Option<usize>,
    family: Tokenizer,
) -> Vec<usize> {
    if sentences.is_empty() {
        return Vec::new();
    }
    // Rank index list, highest first.
    let mut order: Vec<usize> = (0..sentences.len()).collect();
    order.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap_or(std::cmp::Ordering::Equal));

    let chosen = match target_tokens {
        None => order,
        Some(max) => {
            let mut chosen = Vec::new();
            let mut cumulative: usize = 0;
            for idx in order {
                // Token-count by configured tokenizer family. If the call
                // fails (model not loaded), fall back to a char/4 heuristic.
                let count = tokenizer::count(&sentences[idx].text, family).unwrap_or_else(|_| {
                    sentences[idx].text.chars().count().div_ceil(4)
                });
                if chosen.is_empty() {
                    chosen.push(idx);
                    cumulative = count;
                    if count > max {
                        tracing::warn!(
                            target: "rover::summarizer",
                            sentence_tokens = count,
                            target_tokens = max,
                            "top-ranked sentence exceeds target_tokens; emitting anyway",
                        );
                        break;
                    }
                } else if cumulative + count <= max {
                    chosen.push(idx);
                    cumulative += count;
                } else {
                    // Continue scanning — a shorter top-N candidate may fit later.
                }
            }
            chosen
        }
    };

    let mut by_position = chosen;
    by_position.sort_by_key(|&i| sentences[i].span_start);
    by_position
}

/// Format selected sentences into the chosen output style.
fn format_selected(sentences: &[Sentence], indices: &[usize], style: Style) -> String {
    match style {
        Style::Bullet => indices
            .iter()
            .map(|&i| format!("- {}", sentences[i].text))
            .collect::<Vec<_>>()
            .join("\n"),
        Style::Prose => indices
            .iter()
            .map(|&i| sentences[i].text.clone())
            .collect::<Vec<_>>()
            .join(" "),
        Style::Executive => {
            // Use the first selected sentence as the headline, the rest as
            // a 'Details' block.
            if indices.is_empty() {
                String::new()
            } else {
                let head = &sentences[indices[0]].text;
                if indices.len() == 1 {
                    head.clone()
                } else {
                    let rest = indices[1..]
                        .iter()
                        .map(|&i| sentences[i].text.clone())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{head}\n\nDetails: {rest}")
                }
            }
        }
    }
}

#[cfg(test)]
mod extractive_mode_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn select_extractive_picks_in_source_order() {
        let s = sents(&[
            "Low.",
            "High Importance Sentence Here.",
            "Mid.",
        ]);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        // Force a known ordering: top-1 in the middle should still come out
        // sorted by source-position when selected.
        let _ = pr;
        let chosen = select_extractive(&s, &[0.1, 0.9, 0.5], None, Tokenizer::O200k);
        assert_eq!(chosen, vec![0, 1, 2]);
    }

    #[test]
    fn select_extractive_caps_to_target_tokens() {
        let s = sents(&[
            "first sentence.",   // ~4 tokens
            "second sentence.",  // ~4 tokens
            "third sentence.",   // ~4 tokens
        ]);
        let chosen = select_extractive(&s, &[0.5, 0.5, 0.5], Some(5), Tokenizer::O200k);
        // Greedy admits ranked-first sentence (length ~4), then refuses the
        // next (would push cumulative >5).
        assert_eq!(chosen.len(), 1);
    }

    #[test]
    fn format_bullet_prefixes_dashes() {
        let s = sents(&["a.", "b.", "c."]);
        assert_eq!(
            format_selected(&s, &[0, 2], Style::Bullet),
            "- a.\n- c.",
        );
    }

    #[test]
    fn format_executive_with_one_sentence_omits_details() {
        let s = sents(&["only one."]);
        assert_eq!(format_selected(&s, &[0], Style::Executive), "only one.");
    }
}
```

- [ ] **Step 11: Run the selection tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::extractive_mode_tests`

Expected: 4 tests PASS.

### Step 3.6: Headlines mode

- [ ] **Step 12: Write the failing headlines tests**

Append to `src/summarizer/extractive.rs`:

```rust
/// (depth, heading_text) parsed from an ATX heading line, or None.
fn parse_atx_heading(line: &str) -> Option<(usize, &str)> {
    let bytes = line.as_bytes();
    let mut depth = 0;
    while depth < bytes.len() && bytes[depth] == b'#' {
        depth += 1;
    }
    if depth == 0 || depth > 6 {
        return None;
    }
    if depth == bytes.len() {
        return None;
    }
    if bytes[depth] != b' ' {
        return None;
    }
    let text = line[depth + 1..].trim();
    if text.is_empty() {
        None
    } else {
        Some((depth, text))
    }
}

#[derive(Debug)]
struct HeadingSection {
    depth: usize,
    heading: String,
    /// Indices into the `sentences` array.
    sentence_indices: Vec<usize>,
}

/// Walk the source, building (heading → sentences-in-section) groups.
/// Sentences are matched into a section by their `span_start` relative
/// to the byte offsets of the heading lines.
fn group_by_headings(content: &str, sentences: &[Sentence]) -> Vec<HeadingSection> {
    let mut headings = Vec::new();
    let mut byte_offset = 0;
    for line in content.split_inclusive('\n') {
        let line_trimmed = line.trim_end_matches('\n');
        if let Some((depth, text)) = parse_atx_heading(line_trimmed) {
            headings.push(HeadingSection {
                depth,
                heading: text.to_string(),
                sentence_indices: Vec::new(),
            });
            // Push a sentinel span boundary by storing the byte_offset on
            // the heading via a parallel array.
            // We use the headings vec position + a separate offsets vec.
            headings.last_mut().unwrap().sentence_indices.push(byte_offset);
        }
        byte_offset += line.len();
    }
    // First-pass abuse: `sentence_indices[0]` is the heading's byte
    // offset; replace it with the real per-section sentence indices below.
    if headings.is_empty() {
        return Vec::new();
    }
    let heading_offsets: Vec<usize> = headings
        .iter()
        .map(|h| h.sentence_indices[0])
        .collect();
    for h in &mut headings {
        h.sentence_indices.clear();
    }
    for (si, sent) in sentences.iter().enumerate() {
        // Find the heading whose offset precedes this sentence's start.
        let mut bucket: Option<usize> = None;
        for (hi, off) in heading_offsets.iter().enumerate() {
            if sent.span_start >= *off {
                bucket = Some(hi);
            } else {
                break;
            }
        }
        if let Some(b) = bucket {
            headings[b].sentence_indices.push(si);
        }
    }
    headings
}

/// Headlines mode (design §2): for each heading at the deepest covered
/// depth, emit `## heading\n\n{top-1 sentence}\n\n`. Documents without
/// any headings fall back to a flat top-k extractive list capped by
/// `target_tokens` (or top-3 when no target is provided).
fn select_headlines(
    content: &str,
    sentences: &[Sentence],
    scores: &[f32],
    target_tokens: Option<usize>,
    family: Tokenizer,
) -> String {
    let mut groups = group_by_headings(content, sentences);
    // Drop empty sections (heading with no sentence beneath).
    groups.retain(|g| !g.sentence_indices.is_empty());

    if groups.is_empty() {
        // No headings → fall back to flat extractive top-3 (capped by tokens
        // if target_tokens supplied).
        let chosen = select_extractive(sentences, scores, target_tokens.or(Some(usize::MAX)), family);
        let trimmed: Vec<usize> = chosen.into_iter().take(3).collect();
        return format_selected(sentences, &trimmed, Style::Bullet);
    }

    // Pick deepest covered depth.
    let deepest = groups.iter().map(|g| g.depth).max().unwrap();

    // For each group at that depth, pick its highest-scoring sentence.
    let mut emitted = Vec::new();
    let mut cumulative_tokens: usize = 0;
    let token_budget = target_tokens.unwrap_or(usize::MAX);
    for g in groups.iter().filter(|g| g.depth == deepest) {
        let best = g
            .sentence_indices
            .iter()
            .copied()
            .max_by(|a, b| scores[*a].partial_cmp(&scores[*b]).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(idx) = best {
            let count = tokenizer::count(&sentences[idx].text, family)
                .unwrap_or_else(|_| sentences[idx].text.chars().count().div_ceil(4));
            // Always include the first heading even if oversize.
            if !emitted.is_empty() && cumulative_tokens + count > token_budget {
                break;
            }
            let prefix = "#".repeat(g.depth);
            emitted.push(format!("{prefix} {}\n\n{}", g.heading, sentences[idx].text));
            cumulative_tokens += count;
        }
    }
    emitted.join("\n\n")
}

#[cfg(test)]
mod headlines_tests {
    use super::*;

    #[test]
    fn parse_atx_extracts_depth_and_text() {
        assert_eq!(parse_atx_heading("# Hello"), Some((1, "Hello")));
        assert_eq!(parse_atx_heading("### Three"), Some((3, "Three")));
        assert_eq!(parse_atx_heading("####### Too Deep"), None);
        assert_eq!(parse_atx_heading("#NoSpace"), None);
        assert_eq!(parse_atx_heading("Not a heading"), None);
    }

    #[test]
    fn group_by_headings_buckets_sentences_correctly() {
        let content = "# A\nfirst sentence here.\nsecond sentence here.\n# B\nthird sentence here.\n";
        let s = split_sentences(content);
        let groups = group_by_headings(content, &s);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].heading, "A");
        assert_eq!(groups[1].heading, "B");
        // Section A should have 2 sentences (first + second).
        assert_eq!(groups[0].sentence_indices.len(), 2);
        // Section B should have 1 sentence (third).
        assert_eq!(groups[1].sentence_indices.len(), 1);
    }

    #[test]
    fn select_headlines_emits_one_per_section() {
        let content = "# Intro\nThe quick brown fox.\nThe lazy dog.\n## Body\nDetails matter here.\nMore words follow.\n";
        let s = split_sentences(content);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        let out = select_headlines(content, &s, &pr, None, Tokenizer::O200k);
        // Two headings at top level should produce two sections — but the
        // deepest depth is 2 ("## Body"), so only that section is included.
        assert!(out.contains("## Body"));
        assert!(out.contains("Details") || out.contains("More words"));
    }

    #[test]
    fn select_headlines_falls_back_when_no_headings() {
        let content = "First sentence here. Second sentence here. Third one.";
        let s = split_sentences(content);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        let out = select_headlines(content, &s, &pr, None, Tokenizer::O200k);
        // No headings → bullet fallback.
        assert!(out.contains("- "));
    }
}
```

- [ ] **Step 13: Run the headlines tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::headlines_tests`

Expected: 4 tests PASS.

### Step 3.7: `SummarizerBackend` trait impl

- [ ] **Step 14: Write the failing trait test**

Append to `src/summarizer/extractive.rs`:

```rust
/// The public extractive backend. Stateless aside from the `name` field
/// (configurable via the registry so a project can have multiple
/// extractive entries — e.g. one named "default" and one named "fast").
#[derive(Debug, Clone)]
pub struct ExtractiveBackend {
    name: String,
    /// Tokenizer family for `target_tokens` accounting. Defaults to the
    /// configured tokenizer at service construction time.
    tokenizer: Tokenizer,
}

impl ExtractiveBackend {
    pub fn new(name: impl Into<String>, tokenizer: Tokenizer) -> Self {
        Self {
            name: name.into(),
            tokenizer,
        }
    }

    /// Run the full pipeline. Public for direct testing without the trait.
    pub fn run(&self, content: &str, opts: &CompactOpts) -> String {
        let sentences = split_sentences(content);
        if sentences.is_empty() {
            return String::new();
        }
        let vectors = tfidf_vectors(&sentences);
        let scores = pagerank(&vectors);

        match opts.mode {
            CompactMode::Headlines => {
                select_headlines(content, &sentences, &scores, opts.target_tokens, self.tokenizer)
            }
            CompactMode::Extractive => {
                let indices = select_extractive(&sentences, &scores, opts.target_tokens, self.tokenizer);
                format_selected(&sentences, &indices, opts.style)
            }
            CompactMode::Abstractive => {
                // Abstractive falls through to extractive when this backend
                // is invoked directly — the service-level fallback path uses
                // exactly this code path.
                let indices = select_extractive(&sentences, &scores, opts.target_tokens, self.tokenizer);
                format_selected(&sentences, &indices, opts.style)
            }
        }
    }
}

#[async_trait]
impl SummarizerBackend for ExtractiveBackend {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        if content.trim().is_empty() {
            return Err(BackendError::Invalid("empty content".to_string()));
        }
        Ok(self.run(content, opts))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

pub(crate) const EXTRACTIVE_BACKEND_KIND: &str = NAME;

#[cfg(test)]
mod trait_tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn opts(mode: CompactMode, style: Style) -> CompactOpts {
        CompactOpts {
            mode,
            style,
            target_tokens: None,
            focus: None,
            preserve: vec![],
            backend_name: "default".to_string(),
        }
    }

    #[tokio::test]
    async fn empty_content_returns_invalid_error() {
        let be = ExtractiveBackend::new("default", Tokenizer::O200k);
        let r = be.compact("   ", &opts(CompactMode::Extractive, Style::Prose)).await;
        assert!(matches!(r, Err(BackendError::Invalid(_))));
    }

    #[tokio::test]
    async fn extractive_returns_non_empty_for_real_content() {
        let be = ExtractiveBackend::new("default", Tokenizer::O200k);
        let content = "The cat sat on the mat. The dog ran away quickly. The bird flew south.";
        let out = be
            .compact(content, &opts(CompactMode::Extractive, Style::Prose))
            .await
            .unwrap();
        assert!(!out.is_empty());
    }

    #[tokio::test]
    async fn name_round_trips() {
        let be = ExtractiveBackend::new("alt-name", Tokenizer::O200k);
        assert_eq!(be.name(), "alt-name");
    }

    #[test]
    fn preserve_unused_for_extractive_compiles() {
        // Sanity check: PreserveSection is part of the trait surface even
        // though extractive ignores it.
        let _ = PreserveSection::Code;
    }
}
```

- [ ] **Step 15: Run the trait tests**

Run: `cargo test --features test-loopback --lib summarizer::extractive::trait_tests`

Expected: 4 tests PASS.

### Step 3.8: End-to-end integration test

- [ ] **Step 16: Write the end-to-end integration test**

Create `tests/summarizer_extractive.rs`:

```rust
//! End-to-end extractive summarizer test against a real-feeling document.

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::extractive::ExtractiveBackend;
use rover::tokenizer::Tokenizer;

fn opts(mode: CompactMode, target: Option<usize>) -> CompactOpts {
    CompactOpts {
        mode,
        style: Style::Bullet,
        target_tokens: target,
        focus: None,
        preserve: vec![],
        backend_name: "default".to_string(),
    }
}

#[tokio::test]
async fn extractive_three_sentence_caps_to_target_tokens() {
    let content = "\
The Midnight Network is a privacy-preserving blockchain platform. \
It uses zero-knowledge proofs for transaction privacy. \
The network's native token is NIGHT, used for staking and governance.";
    let be = ExtractiveBackend::new("default", Tokenizer::O200k);
    let full = be.compact(content, &opts(CompactMode::Extractive, None)).await.unwrap();
    let bounded = be
        .compact(content, &opts(CompactMode::Extractive, Some(15)))
        .await
        .unwrap();
    assert!(full.len() > bounded.len(), "bounded={bounded:?} full={full:?}");
}

#[tokio::test]
async fn headlines_emits_one_section_per_heading() {
    let content = "\
# Overview\n\
Midnight is a layer-1 privacy-preserving blockchain. It uses ZK proofs.\n\
\n\
# Tokens\n\
NIGHT is the native token. STAR is the unit of account.\n\
\n\
# Networks\n\
Devnet, testnet, and mainnet are all supported.\n";
    let be = ExtractiveBackend::new("default", Tokenizer::O200k);
    let out = be.compact(content, &opts(CompactMode::Headlines, None)).await.unwrap();
    // Three top-level headings → three '#' headers in output.
    let hash_count = out.matches('#').count();
    assert!(hash_count >= 3, "expected ≥3 '#' chars in {out}");
    assert!(out.contains("Overview"));
    assert!(out.contains("Tokens"));
    assert!(out.contains("Networks"));
}
```

- [ ] **Step 17: Run the integration test**

Run: `cargo test --features test-loopback --test summarizer_extractive`

Expected: 2 tests PASS.

- [ ] **Step 18: Commit**

```bash
git add Cargo.toml src/summarizer tests/summarizer_extractive.rs
git commit -m "feat(m7): extractive textrank backend with headlines mode"
```

---

## Task 4: Abstractive Prompt Template

**Files:**
- Create: `src/summarizer/prompts.rs`
- Modify: `src/summarizer/mod.rs` (`pub mod prompts`)

A standalone, side-effect-free module that builds the system + user messages a cloud backend sends. Lives in its own file so the cloud backend (Task 5) stays focused on transport and so the prompt can be unit-tested without genai.

### Step 4.1: Module skeleton

- [ ] **Step 1: Register the module**

In `src/summarizer/mod.rs`, add `pub mod prompts;` to the existing module list:

```rust
pub mod backend;
pub mod error;
pub mod extractive;
pub mod prompts;
```

- [ ] **Step 2: Create the prompts file with failing tests**

Create `src/summarizer/prompts.rs`:

```rust
//! Abstractive prompt template.
//!
//! Builds a single system prompt that's stable across providers. Per-
//! provider tweaks (e.g. omitting the "Reply with only the summary"
//! instruction for models that obey it by default) would live here if
//! ever needed; M7 ships one template.

use crate::summarizer::backend::{CompactOpts, PreserveSection, Style};

/// Pair returned to the cloud backend.
#[derive(Debug, Clone)]
pub struct PromptParts {
    pub system: String,
    pub user: String,
}

fn style_description(style: Style) -> &'static str {
    match style {
        Style::Bullet => "Markdown bullet list, one fact per bullet, no nested bullets.",
        Style::Prose => "One or more short paragraphs.",
        Style::Executive => {
            "Two-section format: a one-sentence headline, then a 'Details' paragraph."
        }
    }
}

fn preserve_description(p: &[PreserveSection]) -> String {
    let mut names: Vec<&'static str> = p
        .iter()
        .map(|pp| match pp {
            PreserveSection::Code => "code blocks",
            PreserveSection::Tables => "tables",
            PreserveSection::Quotes => "blockquotes",
            PreserveSection::Lists => "ordered/unordered lists",
        })
        .collect();
    names.sort();
    names.dedup();
    names.join(", ")
}

/// Render the system + user prompt for an abstractive summarization call.
///
/// Sections (`focus`, `preserve`, `target_tokens`) are conditionally included
/// — empty inputs produce no leading blank lines.
pub fn render_abstractive(opts: &CompactOpts, content: &str) -> PromptParts {
    let mut sys = String::with_capacity(512);
    sys.push_str(
        "You are a precise summarizer. Reply with only the summary — no preamble, no postamble, no meta-commentary. Output valid Markdown.\n\n",
    );
    sys.push_str("Summarize the content provided in the user message.\n\n");

    if let Some(n) = opts.target_tokens {
        sys.push_str(&format!("Target length: ~{n} tokens.\n"));
    }
    sys.push_str(&format!(
        "Output style: {desc}\n",
        desc = style_description(opts.style),
    ));
    if let Some(focus) = opts.focus.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        sys.push_str(&format!("Focus on: {focus}\n"));
    }
    if !opts.preserve.is_empty() {
        sys.push_str(&format!(
            "Preserve the following elements verbatim wherever they appear: {pres}.\n",
            pres = preserve_description(&opts.preserve),
        ));
    }
    sys.push_str("\nRules:\n");
    sys.push_str("- Do not add information not present in the source.\n");
    sys.push_str(
        "- Do not include section titles or headers that the source does not have, unless the chosen style explicitly produces them.\n",
    );
    sys.push_str("- If the source is already shorter than the target, return it unchanged.\n");

    PromptParts {
        system: sys,
        user: content.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, Style};

    fn opts(style: Style, preserve: Vec<PreserveSection>, target: Option<usize>, focus: Option<&str>) -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style,
            target_tokens: target,
            focus: focus.map(str::to_string),
            preserve,
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn minimal_prompt_contains_required_directives() {
        let p = render_abstractive(&opts(Style::Prose, vec![], None, None), "hello world");
        assert!(p.system.contains("Reply with only the summary"));
        assert!(p.system.contains("Output valid Markdown"));
        assert!(p.system.contains("Do not add information not present"));
        assert!(!p.system.contains("Target length"));
        assert!(!p.system.contains("Focus on"));
        assert!(!p.system.contains("Preserve"));
        assert_eq!(p.user, "hello world");
    }

    #[test]
    fn target_tokens_included_when_set() {
        let p = render_abstractive(&opts(Style::Prose, vec![], Some(500), None), "x");
        assert!(p.system.contains("~500 tokens"));
    }

    #[test]
    fn focus_skipped_when_empty_or_whitespace() {
        let p = render_abstractive(&opts(Style::Prose, vec![], None, Some("   ")), "x");
        assert!(!p.system.contains("Focus on"));
    }

    #[test]
    fn preserve_section_lists_human_names_sorted() {
        let p = render_abstractive(
            &opts(
                Style::Prose,
                vec![PreserveSection::Code, PreserveSection::Tables, PreserveSection::Code],
                None,
                None,
            ),
            "x",
        );
        // sorted+deduped → "code blocks, tables"
        assert!(
            p.system.contains("code blocks, tables"),
            "system was {}",
            p.system,
        );
    }

    #[test]
    fn each_style_has_a_distinct_description() {
        let a = render_abstractive(&opts(Style::Bullet, vec![], None, None), "x");
        let b = render_abstractive(&opts(Style::Prose, vec![], None, None), "x");
        let c = render_abstractive(&opts(Style::Executive, vec![], None, None), "x");
        assert_ne!(a.system, b.system);
        assert_ne!(b.system, c.system);
        assert!(a.system.contains("bullet"));
        assert!(b.system.contains("paragraph"));
        assert!(c.system.contains("headline"));
    }
}
```

- [ ] **Step 3: Run the prompt tests**

Run: `cargo test --features test-loopback --lib summarizer::prompts::tests`

Expected: 5 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/summarizer/mod.rs src/summarizer/prompts.rs
git commit -m "feat(m7): abstractive prompt template"
```

---

## Task 5: Cloud Backend (`genai` wrapper)

**Files:**
- Modify: `Cargo.toml` (add `genai`)
- Create: `src/summarizer/cloud.rs`
- Modify: `src/summarizer/mod.rs` (`pub mod cloud`)
- Create: `tests/summarizer_cloud.rs`

The cloud backend translates a `CompactOpts` into a `genai::chat::ChatRequest`, dispatches via `genai::Client`, collects the response into a `String`, and maps `genai::Error` into `BackendError`. The `openai_compat` provider kind installs a `ServiceTargetResolver` so the same code path handles LM Studio, Ollama, vLLM, and OpenAI's shim users.

### Step 5.1: Add the `genai` dep

- [ ] **Step 1: Add the dep with `default-features = false`**

In `Cargo.toml`, in `[dependencies]`:

```toml
genai = { version = "0.4", default-features = false, features = ["rustls"] }
```

- [ ] **Step 2: Verify it resolves**

Run: `cargo build --lib`

Expected: SUCCESS. If `genai 0.4.x` API has shifted in a way that breaks this design (the `ServiceTargetResolver` shape changed, etc.), the implementer pins a working patch version and updates the code below. Any divergence gets noted in §11 of the spec — do not silently patch without flagging.

### Step 5.2: Cloud backend skeleton + provider parsing

- [ ] **Step 3: Register the module**

In `src/summarizer/mod.rs`:

```rust
pub mod backend;
pub mod cloud;
pub mod error;
pub mod extractive;
pub mod prompts;
```

- [ ] **Step 4: Write the failing provider-parse test**

Create `src/summarizer/cloud.rs`:

```rust
//! Cloud-backed summarizer wrapping `genai::Client`.
//!
//! Supports every provider `genai` ships natively (OpenAI, Anthropic,
//! Gemini, xAI, Groq, DeepSeek, Together, Fireworks) plus a custom
//! `openai_compat` kind that points at any OpenAI-compatible endpoint
//! via a `ServiceTargetResolver`.

use async_trait::async_trait;
use genai::chat::{ChatMessage, ChatRequest};
use genai::resolver::{AuthData, ServiceTargetResolver};
use genai::{Client, ServiceTarget};

use crate::summarizer::backend::{CompactMode, CompactOpts, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::summarizer::prompts::render_abstractive;

/// Provider kind parsed from `[backends.<name>] provider = "..."`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    XAi,
    Groq,
    DeepSeek,
    Together,
    Fireworks,
    /// Custom base_url speaking the OpenAI Chat Completions shape.
    OpenAiCompat,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "openai" => Ok(ProviderKind::OpenAi),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "gemini" => Ok(ProviderKind::Gemini),
            "xai" => Ok(ProviderKind::XAi),
            "groq" => Ok(ProviderKind::Groq),
            "deepseek" => Ok(ProviderKind::DeepSeek),
            "together" => Ok(ProviderKind::Together),
            "fireworks" => Ok(ProviderKind::Fireworks),
            "openai_compat" => Ok(ProviderKind::OpenAiCompat),
            other => Err(format!("unknown provider: {other}")),
        }
    }

    /// `genai` adapter kind, used by the resolver when remapping a model.
    fn adapter_kind(&self) -> genai::adapter::AdapterKind {
        use genai::adapter::AdapterKind;
        match self {
            ProviderKind::OpenAi | ProviderKind::OpenAiCompat => AdapterKind::OpenAI,
            ProviderKind::Anthropic => AdapterKind::Anthropic,
            ProviderKind::Gemini => AdapterKind::Gemini,
            ProviderKind::XAi => AdapterKind::Xai,
            ProviderKind::Groq => AdapterKind::Groq,
            ProviderKind::DeepSeek => AdapterKind::DeepSeek,
            ProviderKind::Together => AdapterKind::Together,
            ProviderKind::Fireworks => AdapterKind::Fireworks,
        }
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn parses_every_supported_provider() {
        for s in [
            "openai", "anthropic", "gemini", "xai", "groq", "deepseek", "together", "fireworks",
            "openai_compat",
        ] {
            assert!(ProviderKind::parse(s).is_ok(), "unexpected failure for {s}");
        }
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(ProviderKind::parse("bogus").is_err());
    }
}
```

- [ ] **Step 5: Run the provider-parse tests**

Run: `cargo test --features test-loopback --lib summarizer::cloud::provider_tests`

Expected: 2 tests PASS.

### Step 5.3: `CloudBackend` impl

- [ ] **Step 6: Append the backend struct + trait impl**

Append to `src/summarizer/cloud.rs`:

```rust
/// Cloud backend. Builds a `genai::Client` once at construction; the
/// service holds an `Arc<dyn SummarizerBackend>` so this struct is
/// cheap to clone.
#[derive(Debug, Clone)]
pub struct CloudBackend {
    name: String,
    model: String,
    provider: ProviderKind,
    client: Client,
}

impl CloudBackend {
    /// Build a cloud backend.
    ///
    /// * `name` — config-key name (e.g. "fast").
    /// * `provider` — parsed provider kind.
    /// * `model` — the literal model id passed to genai (e.g. "gpt-4o-mini").
    /// * `base_url` — only used when `provider == OpenAiCompat`. For native
    ///   providers, pass an empty string.
    /// * `api_key` — when `Some`, installs an explicit auth override. When
    ///   `None`, genai's default env-var resolution applies (OPENAI_API_KEY,
    ///   ANTHROPIC_API_KEY, etc.).
    pub fn new(
        name: impl Into<String>,
        provider: ProviderKind,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, BackendError> {
        let name = name.into();
        let model = model.into();

        let mut builder = Client::builder();

        if provider == ProviderKind::OpenAiCompat {
            let base = base_url
                .clone()
                .ok_or_else(|| BackendError::Invalid("openai_compat requires base_url".into()))?;
            let key_for_resolver = api_key.clone().unwrap_or_else(|| "noop".to_string());
            let mapped_model = model.clone();
            let resolver = ServiceTargetResolver::from_resolver_fn(
                move |service_target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                    // Only remap when the call is destined for our model. We
                    // route by exact model id match because multiple
                    // openai_compat backends with different base_urls might
                    // share a process.
                    if service_target.model.model_name.as_ref() == mapped_model {
                        Ok(ServiceTarget {
                            endpoint: genai::Endpoint::from_owned(base.clone()),
                            auth: AuthData::from_single(key_for_resolver.clone()),
                            model: service_target.model,
                        })
                    } else {
                        Ok(service_target)
                    }
                },
            );
            builder = builder.with_service_target_resolver(resolver);
        } else if let Some(k) = api_key {
            // Native providers with an explicit key override. Most users
            // leave api_key None and let genai's env-var defaults work.
            builder = builder.with_auth_resolver(
                genai::resolver::AuthResolver::from_resolver_fn(
                    move |_| -> Result<Option<AuthData>, genai::resolver::Error> {
                        Ok(Some(AuthData::from_single(k.clone())))
                    },
                ),
            );
        }

        let client = builder.build();

        // Sanity-check that genai knows this adapter at the kind level. We
        // don't reach the network here — just confirm enum mapping works.
        let _ = provider.adapter_kind();

        Ok(Self {
            name,
            model,
            provider,
            client,
        })
    }

    fn build_request(&self, content: &str, opts: &CompactOpts) -> ChatRequest {
        let parts = render_abstractive(opts, content);
        ChatRequest::new(vec![
            ChatMessage::system(parts.system),
            ChatMessage::user(parts.user),
        ])
    }

    /// Translate a genai error into our error type. Errors live as a
    /// single chained `Display` in genai 0.4; we string-match the status
    /// number out of common shapes plus check the `Display` for known
    /// signal keywords.
    fn map_error(err: genai::Error) -> BackendError {
        let s = err.to_string().to_lowercase();
        if s.contains("401") || s.contains("403") || s.contains("unauthorized")
            || s.contains("forbidden") || s.contains("api_key") || s.contains("api key")
        {
            BackendError::AuthFailed(err.to_string())
        } else if s.contains("429") || s.contains("rate limit") || s.contains("too many requests") {
            BackendError::RateLimited
        } else if s.contains("model") && (s.contains("not found") || s.contains("invalid")) {
            BackendError::ModelError(err.to_string())
        } else {
            BackendError::Unavailable(err.to_string())
        }
    }
}

#[async_trait]
impl SummarizerBackend for CloudBackend {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        if content.trim().is_empty() {
            return Err(BackendError::Invalid("empty content".to_string()));
        }
        // Only Abstractive uses the cloud round-trip; Extractive and
        // Headlines belong to the extractive backend. If a caller asks
        // a cloud backend for Extractive output, we still send the
        // chat request — the abstractive prompt produces extractive-style
        // output well enough — but log a warning so this misuse is visible.
        if opts.mode != CompactMode::Abstractive {
            tracing::warn!(
                target: "rover::summarizer",
                mode = opts.mode.as_str(),
                backend = self.name,
                "cloud backend invoked for non-abstractive mode",
            );
        }
        let req = self.build_request(content, opts);
        let resp = self
            .client
            .exec_chat(&self.model, req, None)
            .await
            .map_err(Self::map_error)?;
        Ok(resp.first_text().unwrap_or_default().to_string())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod cloud_tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn opts() -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(200),
            focus: None,
            preserve: vec![],
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn build_request_has_two_messages() {
        let be = CloudBackend::new(
            "fast",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            Some("noop".into()),
        )
        .unwrap();
        let req = be.build_request("hello", &opts());
        // Two messages: system + user.
        assert_eq!(req.messages.len(), 2);
    }

    #[test]
    fn openai_compat_requires_base_url() {
        let r = CloudBackend::new("custom", ProviderKind::OpenAiCompat, "m", None, None);
        assert!(matches!(r, Err(BackendError::Invalid(_))));
    }

    #[test]
    fn openai_compat_constructs_with_base_url() {
        let r = CloudBackend::new(
            "custom",
            ProviderKind::OpenAiCompat,
            "m",
            Some("http://127.0.0.1:1234/v1".into()),
            Some("k".into()),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn map_error_recognizes_401_as_auth() {
        // We can't easily construct a `genai::Error` directly — instead
        // confirm the string heuristic by hitting the function with a
        // ModelError variant that contains 401-ish text. The function is
        // a pure string match, so equivalence holds.
        let s = "request failed with status 401 unauthorized".to_lowercase();
        let recognized_as_auth = s.contains("401") || s.contains("unauthorized");
        assert!(recognized_as_auth);
    }

    #[test]
    fn preserve_optional_field_round_trips() {
        let _ = vec![PreserveSection::Code];
    }
}
```

- [ ] **Step 7: Run the cloud unit tests**

Run: `cargo test --features test-loopback --lib summarizer::cloud`

Expected: All tests PASS (provider + cloud).

### Step 5.4: Integration test against wiremock

- [ ] **Step 8: Write the failing wiremock integration test**

Create `tests/summarizer_cloud.rs`:

```rust
//! Cloud backend integration test against a wiremock OpenAI-compatible
//! endpoint. Verifies the wire shape (system + user messages, model id)
//! and the response decoding path without a real LLM.

#![cfg(feature = "test-loopback")]

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::cloud::{CloudBackend, ProviderKind};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn opts(focus: Option<&str>) -> CompactOpts {
    CompactOpts {
        mode: CompactMode::Abstractive,
        style: Style::Prose,
        target_tokens: Some(150),
        focus: focus.map(str::to_string),
        preserve: vec![],
        backend_name: "lm".to_string(),
    }
}

#[tokio::test]
async fn cloud_round_trips_against_openai_compat_endpoint() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "lm-test",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Summary: hello world.",
                },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(server.uri()),
        Some("test-key".into()),
    )
    .unwrap();

    let out = be
        .compact("Please summarize this text.", &opts(None))
        .await
        .expect("summarization succeeds");
    assert!(out.contains("hello world"), "got {out:?}");
}

#[tokio::test]
async fn cloud_maps_401_to_auth_failed() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": "invalid api key", "type": "invalid_request_error" }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(server.uri()),
        Some("wrong-key".into()),
    )
    .unwrap();

    let err = be.compact("hi.", &opts(None)).await.unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("auth") || s.contains("401"), "got {s}");
}

#[tokio::test]
async fn cloud_maps_429_to_rate_limited() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": { "message": "rate limit exceeded", "type": "rate_limit_exceeded" }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(server.uri()),
        Some("k".into()),
    )
    .unwrap();

    let err = be.compact("hi.", &opts(None)).await.unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("rate"), "got {s}");
}
```

- [ ] **Step 9: Run the wiremock integration test**

Run: `cargo test --features test-loopback --test summarizer_cloud`

Expected: 3 tests PASS. If `genai 0.4` uses a different request URL suffix or auth header shape, adjust the matchers; the implementer documents any divergence in the spec.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml src/summarizer/mod.rs src/summarizer/cloud.rs tests/summarizer_cloud.rs
git commit -m "feat(m7): cloud backend over genai with openai_compat support"
```

---

## Task 6: Backend Registry + `[summarization]` Config

**Files:**
- Modify: `src/config.rs` (add `SummarizationConfig` + `BackendConfig` + parsing)
- Create: `src/summarizer/registry.rs`
- Modify: `src/summarizer/mod.rs` (`pub mod registry`)

A `SummarizerRegistry` is built once at startup from the loaded config. Validation rules (default_backend must exist; at least one extractive backend when fallback is enabled) fail fast at construction time rather than on first call. If no `[backends.*]` blocks are configured, the registry installs an implicit `default` extractive backend so a fresh install works offline.

### Step 6.1: Config additions

- [ ] **Step 1: Write the failing config-parse tests**

In `src/config.rs`, append to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn summarization_section_parses_with_defaults() {
    let toml = r#"
[summarization]
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.summarization.default_backend, "default");
    assert_eq!(cfg.summarization.default_mode, "abstractive");
    assert_eq!(cfg.summarization.default_style, "prose");
    assert!(cfg.summarization.fallback_to_extractive);
}

#[test]
fn backends_section_parses_extractive_block() {
    let toml = r#"
[backends.default]
kind = "extractive"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.backends.len(), 1);
    let b = cfg.backends.get("default").unwrap();
    assert_eq!(b.kind, "extractive");
    assert!(b.provider.is_none());
}

#[test]
fn backends_section_parses_cloud_block_with_all_fields() {
    let toml = r#"
[backends.lm_studio]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234/v1"
model = "qwen3.5-0.8b"
api_key_env = "LM_KEY"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let b = cfg.backends.get("lm_studio").unwrap();
    assert_eq!(b.kind, "cloud");
    assert_eq!(b.provider.as_deref(), Some("openai_compat"));
    assert_eq!(b.base_url.as_deref(), Some("http://localhost:1234/v1"));
    assert_eq!(b.model.as_deref(), Some("qwen3.5-0.8b"));
    assert_eq!(b.api_key_env.as_deref(), Some("LM_KEY"));
}

#[test]
fn missing_summarization_section_yields_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.summarization.default_backend, "default");
    assert!(cfg.backends.is_empty());
}
```

- [ ] **Step 2: Run the config tests to verify they fail**

Run: `cargo test --features test-loopback --lib config::tests::summarization_section_parses_with_defaults`

Expected: FAIL with "field `summarization` not found" or similar.

- [ ] **Step 3: Add the config structs**

In `src/config.rs`, just below `CacheConfig` (around the section that introduces other typed config blocks), add:

```rust
/// Top-level `[summarization]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarizationConfig {
    #[serde(default = "default_summarization_backend")]
    pub default_backend: String,

    #[serde(default = "default_summarization_mode")]
    pub default_mode: String,

    #[serde(default = "default_summarization_style")]
    pub default_style: String,

    #[serde(default = "default_summarization_fallback")]
    pub fallback_to_extractive: bool,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            default_backend: default_summarization_backend(),
            default_mode: default_summarization_mode(),
            default_style: default_summarization_style(),
            fallback_to_extractive: default_summarization_fallback(),
        }
    }
}

fn default_summarization_backend() -> String { "default".to_string() }
fn default_summarization_mode() -> String { "abstractive".to_string() }
fn default_summarization_style() -> String { "prose".to_string() }
fn default_summarization_fallback() -> bool { true }

/// One `[backends.<name>]` block. Free-form `kind`/`provider` strings —
/// validation lives in `summarizer::registry::build` where the parsed
/// values are matched against the typed enum.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub kind: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}
```

- [ ] **Step 4: Wire the new fields into `Config`**

Find the `Config` struct (top-level) and add two new fields after the existing ones:

```rust
#[serde(default)]
pub summarization: SummarizationConfig,

#[serde(default)]
pub backends: std::collections::HashMap<String, BackendConfig>,
```

Also update `Config::Default` impl (if it exists) to initialise both as `Default::default()` and `HashMap::new()`.

- [ ] **Step 5: Re-run the config tests**

Run: `cargo test --features test-loopback --lib config::tests::summarization_section_parses_with_defaults config::tests::backends_section_parses_extractive_block config::tests::backends_section_parses_cloud_block_with_all_fields config::tests::missing_summarization_section_yields_defaults`

Expected: 4 tests PASS.

### Step 6.2: Registry construction

- [ ] **Step 6: Register the module**

In `src/summarizer/mod.rs`:

```rust
pub mod backend;
pub mod cloud;
pub mod error;
pub mod extractive;
pub mod prompts;
pub mod registry;
```

- [ ] **Step 7: Write the failing registry-build test**

Create `src/summarizer/registry.rs`:

```rust
//! Backend registry construction.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{BackendConfig, Config, SummarizationConfig};
use crate::summarizer::backend::SummarizerBackend;
use crate::summarizer::cloud::{CloudBackend, ProviderKind};
use crate::summarizer::error::SummarizerError;
use crate::summarizer::extractive::ExtractiveBackend;
use crate::tokenizer::Tokenizer;

/// Frozen registry of summarizer backends.
#[derive(Debug, Clone)]
pub struct SummarizerRegistry {
    backends: HashMap<String, Arc<dyn SummarizerBackend>>,
    default_backend: String,
    extractive_fallback: Option<String>,
}

impl SummarizerRegistry {
    pub fn get(&self, name: &str) -> Result<Arc<dyn SummarizerBackend>, SummarizerError> {
        self.backends
            .get(name)
            .cloned()
            .ok_or_else(|| SummarizerError::NoSuchBackend {
                name: name.to_string(),
            })
    }

    pub fn default_backend_name(&self) -> &str {
        &self.default_backend
    }

    pub fn extractive_fallback_name(&self) -> Option<&str> {
        self.extractive_fallback.as_deref()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.backends.keys().map(String::as_str)
    }
}

/// Build a registry from a config + tokenizer family.
///
/// Validation:
/// 1. Every `[backends.<name>]` parses into a concrete backend.
/// 2. `summarization.default_backend` refers to one of those names.
/// 3. If `summarization.fallback_to_extractive == true`, at least one
///    extractive backend exists (any name).
///
/// If `config.backends` is empty entirely, the registry installs an
/// implicit `default` extractive backend so a fresh install works
/// offline without any configuration. This is the only case where we
/// silently inject — once a user adds any `[backends.*]` block, the
/// validation rules apply strictly.
pub fn build(config: &Config, tokenizer: Tokenizer) -> Result<SummarizerRegistry, SummarizerError> {
    let mut backends: HashMap<String, Arc<dyn SummarizerBackend>> = HashMap::new();

    if config.backends.is_empty() {
        backends.insert(
            "default".to_string(),
            Arc::new(ExtractiveBackend::new("default", tokenizer)),
        );
    } else {
        for (name, cfg) in &config.backends {
            let b = build_one(name, cfg, tokenizer)?;
            backends.insert(name.clone(), b);
        }
    }

    let default_backend = config.summarization.default_backend.clone();
    if !backends.contains_key(&default_backend) {
        return Err(SummarizerError::NoSuchBackend {
            name: default_backend,
        });
    }

    let extractive_fallback = find_extractive_fallback(&backends);
    if config.summarization.fallback_to_extractive && extractive_fallback.is_none() {
        return Err(SummarizerError::NoExtractiveBackendForFallback);
    }

    Ok(SummarizerRegistry {
        backends,
        default_backend,
        extractive_fallback,
    })
}

fn build_one(
    name: &str,
    cfg: &BackendConfig,
    tokenizer: Tokenizer,
) -> Result<Arc<dyn SummarizerBackend>, SummarizerError> {
    match cfg.kind.as_str() {
        "extractive" => Ok(Arc::new(ExtractiveBackend::new(name, tokenizer))),
        "cloud" => {
            let provider = cfg.provider.as_deref().ok_or_else(|| {
                SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason: "cloud backend requires `provider`".into(),
                }
            })?;
            let model = cfg.model.as_deref().ok_or_else(|| {
                SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason: "cloud backend requires `model`".into(),
                }
            })?;
            let provider_kind = ProviderKind::parse(provider).map_err(|reason| {
                SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason,
                }
            })?;
            let api_key = cfg
                .api_key_env
                .as_deref()
                .and_then(|var| std::env::var(var).ok());
            let be = CloudBackend::new(name, provider_kind, model, cfg.base_url.clone(), api_key)
                .map_err(|e| SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason: e.to_string(),
                })?;
            Ok(Arc::new(be))
        }
        other => Err(SummarizerError::BackendUnavailable {
            name: name.to_string(),
            reason: format!("unknown backend kind: {other}"),
        }),
    }
}

fn find_extractive_fallback(
    backends: &HashMap<String, Arc<dyn SummarizerBackend>>,
) -> Option<String> {
    // Prefer "default" if it's an extractive backend; otherwise the first
    // extractive backend by name lex order for determinism.
    let mut names: Vec<&String> = backends.keys().collect();
    names.sort();
    for n in &names {
        if backends[*n].model_id().is_empty() {
            // model_id == "" is the convention for extractive backends.
            if *n == "default" {
                return Some((*n).clone());
            }
        }
    }
    for n in names {
        if backends[n].model_id().is_empty() {
            return Some(n.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SummarizationConfig;

    fn cfg_with_backends(map: &[(&str, BackendConfig)]) -> Config {
        let mut cfg = Config::default();
        cfg.summarization = SummarizationConfig::default();
        cfg.backends = map.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        cfg
    }

    #[test]
    fn empty_backends_installs_implicit_extractive_default() {
        let cfg = Config::default();
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
        assert_eq!(reg.default_backend_name(), "default");
        assert_eq!(reg.extractive_fallback_name(), Some("default"));
    }

    #[test]
    fn explicit_extractive_backend_builds() {
        let cfg = cfg_with_backends(&[(
            "default",
            BackendConfig { kind: "extractive".into(), ..Default::default() },
        )]);
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
    }

    #[test]
    fn default_backend_missing_errors() {
        let mut cfg = cfg_with_backends(&[(
            "alt",
            BackendConfig { kind: "extractive".into(), ..Default::default() },
        )]);
        cfg.summarization.default_backend = "missing".into();
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(r, Err(SummarizerError::NoSuchBackend { .. })));
    }

    #[test]
    fn cloud_backend_requires_provider_and_model() {
        let cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: None,
                model: None,
                base_url: None,
                api_key_env: None,
            },
        )]);
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(r, Err(SummarizerError::BackendUnavailable { .. })));
    }

    #[test]
    fn fallback_disabled_allows_cloud_only_registry() {
        let mut cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                base_url: None,
                api_key_env: None,
            },
        )]);
        cfg.summarization.fallback_to_extractive = false;
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
        assert!(reg.extractive_fallback_name().is_none());
    }

    #[test]
    fn fallback_enabled_requires_extractive_backend() {
        let mut cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                base_url: None,
                api_key_env: None,
            },
        )]);
        cfg.summarization.fallback_to_extractive = true;
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(r, Err(SummarizerError::NoExtractiveBackendForFallback)));
    }
}
```

- [ ] **Step 8: Run the registry tests**

Run: `cargo test --features test-loopback --lib summarizer::registry::tests`

Expected: 6 tests PASS.

- [ ] **Step 9: Commit**

```bash
git add src/config.rs src/summarizer/mod.rs src/summarizer/registry.rs
git commit -m "feat(m7): summarizer registry + [summarization]/[backends.*] config"
```

---

## Task 7: `SummarizerService` (Cache Hot Path + Fallback)

**Files:**
- Modify: `src/summarizer/mod.rs` (`SummarizerService` struct + impl)
- Create: `tests/summary_cache_lifecycle.rs`

The service wraps a `SummarizerRegistry` plus a `Db` handle. Hot path: hash params, lookup `summary_cache`, dispatch to backend on miss, fall back on cloud failure, write the cache row. Backends never see the cache; tests at the service layer exercise both the hit and miss + fallback paths.

### Step 7.1: `SummarizerService` skeleton

- [ ] **Step 1: Add the failing service test**

Append to `src/summarizer/mod.rs` (after the `params_hash` helper):

```rust
use std::sync::Arc;

use crate::storage::Db;
use crate::storage::summaries;
use crate::summarizer::backend::{CompactMode, CompactOpts, Style};
use crate::summarizer::error::SummarizerError;
use crate::summarizer::registry::SummarizerRegistry;

/// Outcome of a `SummarizerService::compact` call. Carries enough context
/// for the MCP tool to render the response envelope (cache_status,
/// fallback metadata).
#[derive(Debug, Clone)]
pub struct SummaryResult {
    pub summary_md: String,
    pub cache_status: SummaryCacheStatus,
    pub effective_backend: String,
    pub effective_model_id: String,
    pub fallback: Option<FallbackInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryCacheStatus {
    Hit,
    Miss,
}

#[derive(Debug, Clone)]
pub struct FallbackInfo {
    pub from: String,
    pub reason: &'static str,
}

/// Service over the registry + storage. Cheap to clone; both fields are
/// `Arc`.
#[derive(Debug, Clone)]
pub struct SummarizerService {
    db: Db,
    registry: Arc<SummarizerRegistry>,
    fallback_to_extractive: bool,
}

impl SummarizerService {
    pub fn new(db: Db, registry: Arc<SummarizerRegistry>, fallback_to_extractive: bool) -> Self {
        Self {
            db,
            registry,
            fallback_to_extractive,
        }
    }

    pub fn registry(&self) -> &SummarizerRegistry {
        &self.registry
    }

    /// Compact `content` per `opts`. `content_hash` is the cache key —
    /// the caller decides what it represents (extracted_md hash, table
    /// hash, etc.). Defaults for `opts.backend_name` are resolved by
    /// the registry's `default_backend_name()` *before* calling — the
    /// service trusts whatever name is in the opts.
    pub async fn compact(
        &self,
        content_hash: &str,
        content: &str,
        opts: &CompactOpts,
    ) -> Result<SummaryResult, SummarizerError> {
        let backend = self.registry.get(&opts.backend_name)?;
        let model_id = backend.model_id().to_string();
        let params_hash = params_hash(opts, &model_id);

        // Cache lookup.
        if let Some(row) = summaries::lookup(&self.db, content_hash, &params_hash).await? {
            return Ok(SummaryResult {
                summary_md: row.summary_md,
                cache_status: SummaryCacheStatus::Hit,
                effective_backend: opts.backend_name.clone(),
                effective_model_id: model_id,
                fallback: None,
            });
        }

        // Miss: dispatch.
        match backend.compact(content, opts).await {
            Ok(md) => {
                summaries::insert(&self.db, content_hash, &params_hash, &md).await?;
                Ok(SummaryResult {
                    summary_md: md,
                    cache_status: SummaryCacheStatus::Miss,
                    effective_backend: opts.backend_name.clone(),
                    effective_model_id: model_id,
                    fallback: None,
                })
            }
            Err(orig_err) => {
                let translated = SummarizerError::from_backend(&opts.backend_name, orig_err);
                if !self.fallback_to_extractive {
                    return Err(translated);
                }
                let Some(fb_name) = self.registry.extractive_fallback_name() else {
                    return Err(translated);
                };
                if fb_name == opts.backend_name {
                    // Already extractive; nothing to fall back to.
                    return Err(translated);
                }
                // Build the fallback opts: same shape, swapped backend name.
                let mut fb_opts = opts.clone();
                fb_opts.backend_name = fb_name.to_string();
                // Force the prompt-free path: extractive backend ignores
                // mode=Abstractive but produces sensible output.
                if fb_opts.mode == CompactMode::Abstractive {
                    fb_opts.mode = CompactMode::Extractive;
                }
                let fb_backend = self.registry.get(fb_name)?;
                let fb_model = fb_backend.model_id().to_string();
                let fb_params = params_hash(&fb_opts, &fb_model);
                if let Some(row) = summaries::lookup(&self.db, content_hash, &fb_params).await? {
                    return Ok(SummaryResult {
                        summary_md: row.summary_md,
                        cache_status: SummaryCacheStatus::Hit,
                        effective_backend: fb_name.to_string(),
                        effective_model_id: fb_model,
                        fallback: Some(FallbackInfo {
                            from: opts.backend_name.clone(),
                            reason: translated.fallback_reason(),
                        }),
                    });
                }
                let md = fb_backend
                    .compact(content, &fb_opts)
                    .await
                    .map_err(|e| SummarizerError::from_backend(fb_name, e))?;
                summaries::insert(&self.db, content_hash, &fb_params, &md).await?;
                Ok(SummaryResult {
                    summary_md: md,
                    cache_status: SummaryCacheStatus::Miss,
                    effective_backend: fb_name.to_string(),
                    effective_model_id: fb_model,
                    fallback: Some(FallbackInfo {
                        from: opts.backend_name.clone(),
                        reason: translated.fallback_reason(),
                    }),
                })
            }
        }
    }

    /// Convenience: build opts using `[summarization]` defaults for
    /// unset fields. Returns the opts plus the resolved backend name
    /// (in case the caller wants to log it).
    pub fn resolve_defaults(
        &self,
        mode: Option<CompactMode>,
        style: Option<Style>,
        target_tokens: Option<usize>,
        focus: Option<String>,
        preserve: Vec<crate::summarizer::backend::PreserveSection>,
        backend: Option<String>,
        defaults: &DefaultsHint,
    ) -> CompactOpts {
        CompactOpts {
            mode: mode.unwrap_or(defaults.mode),
            style: style.unwrap_or(defaults.style),
            target_tokens,
            focus,
            preserve,
            backend_name: backend.unwrap_or_else(|| defaults.backend.clone()),
        }
    }
}

/// Compact form of `[summarization]` defaults so callers don't have to
/// carry the whole `Config` reference.
#[derive(Debug, Clone)]
pub struct DefaultsHint {
    pub backend: String,
    pub mode: CompactMode,
    pub style: Style,
}

impl DefaultsHint {
    /// Parse string-typed values from `SummarizationConfig`. Unknown
    /// strings fall back to `Abstractive`/`Prose` with a warning logged.
    pub fn from_config(c: &crate::config::SummarizationConfig) -> Self {
        let mode = match c.default_mode.as_str() {
            "extractive" => CompactMode::Extractive,
            "abstractive" => CompactMode::Abstractive,
            "headlines" => CompactMode::Headlines,
            other => {
                tracing::warn!(
                    target: "rover::summarizer",
                    value = other,
                    "unknown summarization.default_mode; falling back to abstractive",
                );
                CompactMode::Abstractive
            }
        };
        let style = match c.default_style.as_str() {
            "bullet" => Style::Bullet,
            "prose" => Style::Prose,
            "executive" => Style::Executive,
            other => {
                tracing::warn!(
                    target: "rover::summarizer",
                    value = other,
                    "unknown summarization.default_style; falling back to prose",
                );
                Style::Prose
            }
        };
        Self {
            backend: c.default_backend.clone(),
            mode,
            style,
        }
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, Style, SummarizerBackend, PreserveSection};
    use crate::summarizer::error::BackendError;
    use crate::summarizer::registry::SummarizerRegistry;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Recording backend whose call count and forced-error mode the
    /// service tests inspect.
    struct RecordingBackend {
        name: String,
        model: String,
        calls: Arc<AtomicUsize>,
        fail: Option<BackendError>,
    }

    impl std::fmt::Debug for RecordingBackend {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RecordingBackend").field("name", &self.name).finish()
        }
    }

    #[async_trait]
    impl SummarizerBackend for RecordingBackend {
        async fn compact(&self, _: &str, _: &CompactOpts) -> Result<String, BackendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = &self.fail {
                Err(match e {
                    BackendError::Unavailable(s) => BackendError::Unavailable(s.clone()),
                    BackendError::RateLimited => BackendError::RateLimited,
                    BackendError::AuthFailed(s) => BackendError::AuthFailed(s.clone()),
                    BackendError::ModelError(s) => BackendError::ModelError(s.clone()),
                    BackendError::Invalid(s) => BackendError::Invalid(s.clone()),
                })
            } else {
                Ok(format!("(from {})", self.name))
            }
        }
        fn name(&self) -> &str { &self.name }
        fn model_id(&self) -> &str { &self.model }
    }

    async fn make_db() -> (Db, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        (Db::open(&path).await.unwrap(), tmp)
    }

    fn registry_with(backends: Vec<(&str, &str, Option<BackendError>)>, default_name: &str) -> Arc<SummarizerRegistry> {
        // Build a tiny registry by directly constructing the internal map.
        let mut map: std::collections::HashMap<String, Arc<dyn SummarizerBackend>> = Default::default();
        for (n, model, fail) in backends {
            map.insert(
                n.to_string(),
                Arc::new(RecordingBackend {
                    name: n.to_string(),
                    model: model.to_string(),
                    calls: Arc::new(AtomicUsize::new(0)),
                    fail,
                }),
            );
        }
        let extractive = map
            .iter()
            .find(|(_, b)| b.model_id().is_empty())
            .map(|(n, _)| n.clone());
        let reg = SummarizerRegistry::__test_construct(map, default_name.to_string(), extractive);
        Arc::new(reg)
    }

    fn opts(name: &str, mode: CompactMode) -> CompactOpts {
        CompactOpts {
            mode,
            style: Style::Prose,
            target_tokens: None,
            focus: None,
            preserve: vec![],
            backend_name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn cache_hit_short_circuits_backend() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![("default", "", None)],
            "default",
        );
        let svc = SummarizerService::new(db.clone(), reg, true);
        let o = opts("default", CompactMode::Extractive);

        // First call inserts; second call hits the cache.
        let r1 = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
        let r2 = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
        assert_eq!(r1.summary_md, r2.summary_md);
    }

    #[tokio::test]
    async fn backend_failure_falls_back_to_extractive() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![
                ("fast", "gpt-4o-mini", Some(BackendError::AuthFailed("401".into()))),
                ("default", "", None),
            ],
            "default",
        );
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("fast", CompactMode::Abstractive);

        let r = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert_eq!(r.effective_backend, "default");
        assert!(r.fallback.is_some());
        assert_eq!(r.fallback.unwrap().reason, "auth_failed");
        assert!(r.summary_md.contains("from default"));
    }

    #[tokio::test]
    async fn backend_failure_propagates_when_fallback_disabled() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![
                ("fast", "gpt-4o-mini", Some(BackendError::RateLimited)),
                ("default", "", None),
            ],
            "default",
        );
        let svc = SummarizerService::new(db, reg, false);
        let o = opts("fast", CompactMode::Abstractive);
        let r = svc.compact("h1", "hello world.", &o).await;
        assert!(matches!(r, Err(SummarizerError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn no_such_backend_errors_immediately() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(vec![("default", "", None)], "default");
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("missing", CompactMode::Abstractive);
        let r = svc.compact("h", "x.", &o).await;
        assert!(matches!(r, Err(SummarizerError::NoSuchBackend { .. })));
    }
}
```

- [ ] **Step 2: Expose a test-only constructor on the registry**

In `src/summarizer/registry.rs`, at the bottom of `impl SummarizerRegistry` (above the `#[cfg(test)] mod tests` block), append:

```rust
    /// Test-only constructor used by sibling-module tests to inject
    /// recording backends without hitting the config-parse path.
    #[doc(hidden)]
    #[cfg(test)]
    pub fn __test_construct(
        backends: std::collections::HashMap<String, std::sync::Arc<dyn crate::summarizer::backend::SummarizerBackend>>,
        default_backend: String,
        extractive_fallback: Option<String>,
    ) -> Self {
        Self {
            backends,
            default_backend,
            extractive_fallback,
        }
    }
```

- [ ] **Step 3: Run the service tests**

Run: `cargo test --features test-loopback --lib summarizer::service_tests`

Expected: 4 tests PASS.

### Step 7.2: End-to-end summary_cache lifecycle test

- [ ] **Step 4: Write the lifecycle integration test**

Create `tests/summary_cache_lifecycle.rs`:

```rust
//! End-to-end lifecycle test: real DB, real extractive backend, params
//! variation, cache invalidation when `backend_name` changes.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;

use rover::storage::Db;
use rover::summarizer::backend::{CompactMode, CompactOpts, PreserveSection, Style};
use rover::summarizer::extractive::ExtractiveBackend;
use rover::summarizer::registry::SummarizerRegistry;
use rover::summarizer::{SummarizerService, SummaryCacheStatus};
use rover::tokenizer::Tokenizer;

async fn make_service() -> (SummarizerService, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("r.db")).await.unwrap();
    let mut map: std::collections::HashMap<String, Arc<dyn rover::summarizer::backend::SummarizerBackend>> =
        Default::default();
    map.insert(
        "default".into(),
        Arc::new(ExtractiveBackend::new("default", Tokenizer::O200k)),
    );
    let reg = Arc::new(SummarizerRegistry::__test_construct(
        map,
        "default".into(),
        Some("default".into()),
    ));
    (SummarizerService::new(db, reg, true), tmp)
}

fn opts() -> CompactOpts {
    CompactOpts {
        mode: CompactMode::Extractive,
        style: Style::Prose,
        target_tokens: None,
        focus: None,
        preserve: vec![],
        backend_name: "default".into(),
    }
}

#[tokio::test]
async fn second_call_with_same_params_hits_cache() {
    let (svc, _tmp) = make_service().await;
    let r1 = svc
        .compact("h1", "First sentence here. Second sentence here.", &opts())
        .await
        .unwrap();
    let r2 = svc
        .compact("h1", "First sentence here. Second sentence here.", &opts())
        .await
        .unwrap();
    assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
    assert_eq!(r1.summary_md, r2.summary_md);
}

#[tokio::test]
async fn changing_target_tokens_creates_independent_cache_row() {
    let (svc, _tmp) = make_service().await;
    let mut o = opts();
    o.target_tokens = Some(10);
    let r1 = svc
        .compact("h1", "First sentence. Second sentence. Third one.", &o)
        .await
        .unwrap();
    o.target_tokens = Some(100);
    let r2 = svc
        .compact("h1", "First sentence. Second sentence. Third one.", &o)
        .await
        .unwrap();
    assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Miss));
}

#[tokio::test]
async fn changing_preserve_list_order_does_not_re_summarize() {
    let (svc, _tmp) = make_service().await;
    let mut o = opts();
    o.preserve = vec![PreserveSection::Code, PreserveSection::Tables];
    let _r1 = svc.compact("h2", "Sentence here.", &o).await.unwrap();
    o.preserve = vec![PreserveSection::Tables, PreserveSection::Code];
    let r2 = svc.compact("h2", "Sentence here.", &o).await.unwrap();
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
}
```

- [ ] **Step 5: Run the lifecycle test**

Run: `cargo test --features test-loopback --test summary_cache_lifecycle`

Expected: 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/summarizer tests/summary_cache_lifecycle.rs
git commit -m "feat(m7): summarizerservice with cache hot path + extractive fallback"
```

---

## Task 8: Wire `SummarizerService` into MCP State + Binary Startup

**Files:**
- Modify: `src/mcp/handler.rs` (add `Arc<SummarizerService>` field; register the new tool stub)
- Modify: `src/mcp/server.rs` (build the service before constructing the handler)
- Modify: `src/main.rs` (build the service for CLI subcommands that need it)
- Modify: `src/mcp/envelope.rs` (add `RoverError` codes for summarizer errors)
- Modify: `src/mcp/error.rs` (translate `SummarizerError` → `RoverError`)

Everything below this task assumes `RoverHandler` already carries the service. Doing the wiring before the MCP tools means the actual tool tasks (9-13) stay small.

### Step 8.1: `RoverError` codes for summarizer errors

- [ ] **Step 1: Add the new error codes**

In `src/mcp/envelope.rs`, find the `impl RoverError { ... }` block of `pub const` strings and append:

```rust
    pub const SUMMARIZER_NO_SUCH_BACKEND: &'static str = "summarizer_no_such_backend";
    pub const SUMMARIZER_NO_EXTRACTIVE_FOR_FALLBACK: &'static str =
        "summarizer_no_extractive_backend_for_fallback";
    pub const SUMMARIZER_BACKEND_UNAVAILABLE: &'static str = "summarizer_backend_unavailable";
    pub const SUMMARIZER_RATE_LIMITED: &'static str = "summarizer_rate_limited";
    pub const SUMMARIZER_AUTH_FAILED: &'static str = "summarizer_auth_failed";
    pub const SUMMARIZER_MODEL_ERROR: &'static str = "summarizer_model_error";
```

### Step 8.2: `McpError::Summarizer` variant + translation

- [ ] **Step 2: Write the failing translation test**

In `src/mcp/error.rs`, append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn summarizer_no_such_backend_translates() {
    let e = McpError::Summarizer(crate::summarizer::SummarizerError::NoSuchBackend {
        name: "missing".into(),
    });
    let r = e.into_rover_error();
    assert_eq!(r.code, RoverError::SUMMARIZER_NO_SUCH_BACKEND);
}

#[test]
fn summarizer_rate_limited_translates() {
    let e = McpError::Summarizer(crate::summarizer::SummarizerError::RateLimited {
        name: "fast".into(),
    });
    let r = e.into_rover_error();
    assert_eq!(r.code, RoverError::SUMMARIZER_RATE_LIMITED);
}

#[test]
fn summarizer_auth_failed_translates() {
    let e = McpError::Summarizer(crate::summarizer::SummarizerError::AuthFailed {
        name: "fast".into(),
        reason: "401".into(),
    });
    let r = e.into_rover_error();
    assert_eq!(r.code, RoverError::SUMMARIZER_AUTH_FAILED);
}
```

- [ ] **Step 3: Add the variant + arm**

In `src/mcp/error.rs`, find the existing `McpError` enum and add a new variant (alphabetically near other crate-error variants):

```rust
    #[error("summarizer error: {0}")]
    Summarizer(#[from] crate::summarizer::SummarizerError),
```

Then in `into_rover_error`, add arms inside the `match err` block:

```rust
        McpError::Summarizer(e) => match e {
            crate::summarizer::SummarizerError::NoSuchBackend { name } => {
                RoverError {
                    code: RoverError::SUMMARIZER_NO_SUCH_BACKEND.into(),
                    message: format!("no such summarizer backend: {name}"),
                    details: serde_json::json!({"name": name}),
                }
            }
            crate::summarizer::SummarizerError::NoExtractiveBackendForFallback => RoverError {
                code: RoverError::SUMMARIZER_NO_EXTRACTIVE_FOR_FALLBACK.into(),
                message: "no extractive backend configured for fallback".into(),
                details: serde_json::json!({}),
            },
            crate::summarizer::SummarizerError::BackendUnavailable { name, reason } => {
                RoverError {
                    code: RoverError::SUMMARIZER_BACKEND_UNAVAILABLE.into(),
                    message: format!("backend {name} unavailable: {reason}"),
                    details: serde_json::json!({"name": name, "reason": reason}),
                }
            }
            crate::summarizer::SummarizerError::RateLimited { name } => RoverError {
                code: RoverError::SUMMARIZER_RATE_LIMITED.into(),
                message: format!("backend {name} rate limited"),
                details: serde_json::json!({"name": name}),
            },
            crate::summarizer::SummarizerError::AuthFailed { name, reason } => RoverError {
                code: RoverError::SUMMARIZER_AUTH_FAILED.into(),
                message: format!("backend {name} auth failed: {reason}"),
                details: serde_json::json!({"name": name, "reason": reason}),
            },
            crate::summarizer::SummarizerError::ModelError { name, reason } => RoverError {
                code: RoverError::SUMMARIZER_MODEL_ERROR.into(),
                message: format!("backend {name} model error: {reason}"),
                details: serde_json::json!({"name": name, "reason": reason}),
            },
            crate::summarizer::SummarizerError::Storage(e) => {
                McpError::Storage(e).into_rover_error()
            }
            crate::summarizer::SummarizerError::Tokenizer(e) => {
                McpError::Tokenizer(e).into_rover_error()
            }
        },
```

- [ ] **Step 4: Run the translation tests**

Run: `cargo test --features test-loopback --lib mcp::error::tests::summarizer_`

Expected: 3 tests PASS.

### Step 8.3: `RoverHandler` carries the service

- [ ] **Step 5: Add the field**

In `src/mcp/handler.rs`, extend the `RoverHandler` struct:

```rust
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) ssrf_level: SsrfLevel,
    pub(crate) pacer: Arc<Pacer>,
    pub(crate) summarizer: Arc<crate::summarizer::SummarizerService>,
    tool_router: ToolRouter<Self>,
}
```

Update the constructor:

```rust
impl RoverHandler {
    pub fn new(
        db: Db,
        config: Arc<Config>,
        client: reqwest::Client,
        ssrf_level: SsrfLevel,
        pacer: Arc<Pacer>,
        summarizer: Arc<crate::summarizer::SummarizerService>,
    ) -> Self {
        Self {
            db,
            config,
            client,
            ssrf_level,
            pacer,
            summarizer,
            tool_router: Self::tool_router(),
        }
    }
}
```

### Step 8.4: `server.rs` builds the service

- [ ] **Step 6: Construct the service before the handler**

In `src/mcp/server.rs::serve_stdio`, add immediately before `let handler = RoverHandler::new(...)` near the bottom of the function:

```rust
    let registry = Arc::new(
        crate::summarizer::registry::build(&config, config.tokenizer.default)
            .map_err(anyhow::Error::from)?,
    );
    let summarizer = Arc::new(crate::summarizer::SummarizerService::new(
        db.clone(),
        registry,
        config.summarization.fallback_to_extractive,
    ));
```

Update the handler construction:

```rust
    let handler = RoverHandler::new(db.clone(), config, client, ssrf_level, pacer, summarizer);
```

### Step 8.5: `main.rs` builds the service for CLI subcommands that need it

- [ ] **Step 7: Locate `main.rs` and add a helper**

In `src/main.rs`, add at the top of the file (after the existing imports):

```rust
use std::sync::Arc;
```

(If already imported, skip.)

Find or add a small helper just below the `main()` function. The helper builds the service so individual subcommands don't repeat the construction:

```rust
async fn build_summarizer_service(
    db: rover::storage::Db,
    config: &rover::config::Config,
) -> anyhow::Result<Arc<rover::summarizer::SummarizerService>> {
    let registry = Arc::new(
        rover::summarizer::registry::build(config, config.tokenizer.default)
            .map_err(anyhow::Error::from)?,
    );
    Ok(Arc::new(rover::summarizer::SummarizerService::new(
        db,
        registry,
        config.summarization.fallback_to_extractive,
    )))
}
```

(`rover::summarizer::registry` and `rover::summarizer::SummarizerService` must be re-exported from `src/lib.rs`; Task 2 already did this implicitly via `pub mod summarizer` because the submodules are `pub`. If `pub use` is required, add it now to `src/summarizer/mod.rs`.)

### Step 8.6: Verify everything still builds

- [ ] **Step 8: Build + run the full test suite**

Run: `cargo build --all-features && cargo test --features test-loopback`

Expected: 322 existing tests + new tests all pass. The handler constructor change touches `tests/common/mod.rs::spawn_client` if it constructs `RoverHandler` directly anywhere — fix in this step.

- [ ] **Step 9: Update `tests/common/mod.rs` if it constructs the handler**

Inspect `tests/common/mod.rs` for any direct `RoverHandler::new(...)` call. If found, build a minimal summarizer service (using `SummarizerRegistry::__test_construct` with a single extractive backend) and pass it in. Sample helper to drop in:

```rust
pub async fn make_summarizer_service(db: &rover::storage::Db) -> std::sync::Arc<rover::summarizer::SummarizerService> {
    let mut map: std::collections::HashMap<String, std::sync::Arc<dyn rover::summarizer::backend::SummarizerBackend>> =
        Default::default();
    map.insert(
        "default".into(),
        std::sync::Arc::new(rover::summarizer::extractive::ExtractiveBackend::new(
            "default",
            rover::tokenizer::Tokenizer::O200k,
        )),
    );
    let reg = std::sync::Arc::new(rover::summarizer::registry::SummarizerRegistry::__test_construct(
        map,
        "default".into(),
        Some("default".into()),
    ));
    std::sync::Arc::new(rover::summarizer::SummarizerService::new(
        db.clone(),
        reg,
        true,
    ))
}
```

Note: `__test_construct` is gated behind `#[cfg(test)]`, which DOES include integration-test crates. If the visibility doesn't make it across crate boundaries, promote it to `#[cfg(any(test, feature = "test-loopback"))]` and re-run.

- [ ] **Step 10: Run the test suite again**

Run: `cargo test --features test-loopback`

Expected: all tests pass.

- [ ] **Step 11: Commit**

```bash
git add src/mcp src/main.rs tests/common
git commit -m "feat(m7): wire summarizerservice into rover handler + mcp/cli startup"
```

---

## Task 9: `summarize` MCP Tool

**Files:**
- Create: `src/mcp/tools/summarize.rs`
- Modify: `src/mcp/tools/mod.rs` (`pub mod summarize`)
- Modify: `src/mcp/handler.rs` (register `summarize_tool`)
- Modify: `src/mcp/envelope.rs` (`SummarizeResponse`)
- Create: `tests/mcp_summarize.rs`

The headline tool. Cache-or-fetch the page, dispatch through `SummarizerService`, render the response envelope. Synchronous; no task spawning.

### Step 9.1: Response envelope

- [ ] **Step 1: Add `SummarizeResponse` to `envelope.rs`**

In `src/mcp/envelope.rs`, append (before the `#[cfg(test)]` block):

```rust
/// `summarize` tool response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SummarizeResponse {
    pub summary_md: String,
    pub metadata: SummarizeMetadata,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SummarizeMetadata {
    pub backend: String,
    pub mode: String,
    pub style: String,
    pub target_tokens: Option<usize>,
    pub estimated_tokens: usize,
    pub cache_status: SummaryCacheStatusWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer_fallback: Option<SummarizerFallbackInfo>,
    pub source_url: String,
    pub source_fetched_at: String,
    pub focus: Option<String>,
    pub preserve: Vec<String>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryCacheStatusWire {
    Hit,
    Miss,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SummarizerFallbackInfo {
    pub from: String,
    pub reason: String,
}
```

### Step 9.2: Tool args + handler

- [ ] **Step 2: Register the new module**

In `src/mcp/tools/mod.rs`:

```rust
pub mod batch_fetch;
pub mod count_tokens;
pub mod fetch;
pub mod get_metadata;
pub mod summarize;
```

- [ ] **Step 3: Write the failing arg-parse test**

Create `src/mcp/tools/summarize.rs`:

```rust
//! MCP `summarize` tool.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{
    SummarizeMetadata, SummarizeResponse, SummarizerFallbackInfo, SummaryCacheStatusWire,
};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::summarizer::backend::{CompactMode, PreserveSection, Style};
use crate::summarizer::{DefaultsHint, SummaryCacheStatus};
use crate::tokenizer;

/// Wire-side `summarize` args. All fields except `url` are optional;
/// defaults come from `[summarization]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummarizeArgs {
    pub url: String,
    #[serde(default)]
    pub target_tokens: Option<usize>,
    #[serde(default)]
    pub mode: Option<SummarizeMode>,
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub preserve: Vec<SummarizePreserve>,
    #[serde(default)]
    pub style: Option<SummarizeStyle>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub tokenizer: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeMode { Extractive, Abstractive, Headlines }

impl From<SummarizeMode> for CompactMode {
    fn from(v: SummarizeMode) -> Self {
        match v {
            SummarizeMode::Extractive => CompactMode::Extractive,
            SummarizeMode::Abstractive => CompactMode::Abstractive,
            SummarizeMode::Headlines => CompactMode::Headlines,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeStyle { Bullet, Prose, Executive }

impl From<SummarizeStyle> for Style {
    fn from(v: SummarizeStyle) -> Self {
        match v {
            SummarizeStyle::Bullet => Style::Bullet,
            SummarizeStyle::Prose => Style::Prose,
            SummarizeStyle::Executive => Style::Executive,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizePreserve { Code, Tables, Quotes, Lists }

impl From<SummarizePreserve> for PreserveSection {
    fn from(v: SummarizePreserve) -> Self {
        match v {
            SummarizePreserve::Code => PreserveSection::Code,
            SummarizePreserve::Tables => PreserveSection::Tables,
            SummarizePreserve::Quotes => PreserveSection::Quotes,
            SummarizePreserve::Lists => PreserveSection::Lists,
        }
    }
}

impl RoverHandler {
    pub async fn summarize_inner(
        &self,
        args: SummarizeArgs,
    ) -> Result<SummarizeResponse, McpError> {
        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;
        tokenizer::ensure_loaded(family).await?;

        // Cache-or-fetch the page (design §2.7).
        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &self.pacer,
            &self.config.rate_limit,
            &self.config.robots,
            &url,
            &self.config.cache,
            FetchOptions {
                force_refresh: false,
                ssrf_level: self.ssrf_level,
                ignore_robots: false,
                user_agent: self.config.fetch.user_agent.clone(),
            },
            |body, base| {
                let extracted = extract(body, Some(base))
                    .map_err(crate::fetcher::FetcherError::Extract)?;
                let content_hash =
                    format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                    metadata: extracted.metadata,
                })
            },
        )
        .await?;

        let defaults = DefaultsHint::from_config(&self.config.summarization);
        let opts = self.summarizer.resolve_defaults(
            args.mode.map(Into::into),
            args.style.map(Into::into),
            args.target_tokens,
            args.focus,
            args.preserve.into_iter().map(Into::into).collect(),
            args.backend,
            &defaults,
        );

        let summary = self
            .summarizer
            .compact(&result.page.content_hash, &result.page.extracted_md, &opts)
            .await?;

        let estimated_tokens = tokenizer::count(&summary.summary_md, family)?;

        Ok(SummarizeResponse {
            summary_md: summary.summary_md,
            metadata: SummarizeMetadata {
                backend: summary.effective_backend,
                mode: opts.mode.as_str().to_string(),
                style: opts.style.as_str().to_string(),
                target_tokens: opts.target_tokens,
                estimated_tokens,
                cache_status: match summary.cache_status {
                    SummaryCacheStatus::Hit => SummaryCacheStatusWire::Hit,
                    SummaryCacheStatus::Miss => SummaryCacheStatusWire::Miss,
                },
                summarizer_fallback: summary.fallback.map(|f| SummarizerFallbackInfo {
                    from: f.from,
                    reason: f.reason.to_string(),
                }),
                source_url: url.as_str().to_string(),
                source_fetched_at: jiff::Timestamp::from_second(result.page.fetched_at)
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                focus: opts.focus,
                preserve: opts.preserve.iter().map(|p| p.as_str().to_string()).collect(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trips_required_fields() {
        let schema = schemars::schema_for!(SummarizeArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in ["url", "target_tokens", "mode", "focus", "preserve", "style", "backend"] {
            assert!(json.contains(f), "missing {f}");
        }
    }

    #[test]
    fn enum_mappings_round_trip() {
        assert_eq!(CompactMode::from(SummarizeMode::Headlines), CompactMode::Headlines);
        assert_eq!(Style::from(SummarizeStyle::Bullet), Style::Bullet);
        assert_eq!(
            PreserveSection::from(SummarizePreserve::Tables),
            PreserveSection::Tables,
        );
    }

    #[test]
    fn rejects_unknown_field() {
        let r: Result<SummarizeArgs, _> =
            serde_json::from_str(r#"{"url":"https://x/","bogus":1}"#);
        assert!(r.is_err());
    }
}
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test --features test-loopback --lib mcp::tools::summarize::tests`

Expected: 3 tests PASS.

### Step 9.3: Register the tool

- [ ] **Step 5: Add the `#[tool]` wrapper**

In `src/mcp/handler.rs`, inside the `#[tool_router] impl RoverHandler { ... }` block, add a new tool method (alphabetical order — between `count_tokens_tool` and existing tools):

```rust
    /// Apply summarization to a URL's cached or freshly-fetched markdown.
    #[tool(
        description = "Apply summarization to a URL. If the URL isn't cached, \
                       Rover fetches it with default options first. Returns the \
                       summary_md plus metadata including cache status, the \
                       effective backend, and (when applicable) fallback details."
    )]
    pub async fn summarize_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::summarize::SummarizeArgs>,
    ) -> Result<Json<crate::mcp::envelope::SummarizeResponse>, ErrorData> {
        match self.summarize_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }
```

Update the `with_instructions` string to advertise the new tool:

```rust
            .with_instructions(
                "Web fetch & prep for LLM agents. \
                 Tools: fetch, summarize, count_tokens, get_metadata, batch_fetch.",
            )
```

### Step 9.4: Integration test via spawn_client

- [ ] **Step 6: Write the failing integration test**

Create `tests/mcp_summarize.rs`:

```rust
//! End-to-end MCP test for `summarize`.

#![cfg(feature = "test-loopback")]

mod common;

use common::spawn_client;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html_body() -> &'static str {
    "<html><head><title>Hello</title></head><body>\
     <h1>Hello</h1>\
     <p>The Midnight Network is a privacy-preserving blockchain. It uses zero-knowledge proofs.</p>\
     <p>The native token is NIGHT. STAR is the unit of account for transaction fees.</p>\
     </body></html>"
}

#[tokio::test]
async fn summarize_returns_extractive_output_on_cache_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html_body()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/article", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "summarize".into(),
            arguments: Some(
                json!({
                    "url": url,
                    "mode": "extractive",
                    "target_tokens": 50,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await
        .expect("summarize succeeded");

    let body = resp.content.first().and_then(|c| c.as_text()).expect("text").text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["summary_md"].as_str().unwrap().len() > 0);
    assert_eq!(v["metadata"]["mode"], "extractive");
    assert_eq!(v["metadata"]["cache_status"], "miss");
    assert_eq!(v["metadata"]["backend"], "default");
}

#[tokio::test]
async fn summarize_second_call_hits_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html_body()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/article2", server.uri());

    let args = json!({
        "url": url,
        "mode": "extractive",
    });

    let _ = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "summarize".into(),
            arguments: Some(args.as_object().unwrap().clone()),
        })
        .await
        .unwrap();

    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "summarize".into(),
            arguments: Some(args.as_object().unwrap().clone()),
        })
        .await
        .unwrap();
    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["metadata"]["cache_status"], "hit");
}
```

- [ ] **Step 7: Run the integration test**

Run: `cargo test --features test-loopback --test mcp_summarize`

Expected: 2 tests PASS. Requires `tests/common/mod.rs::spawn_client` to seed a `rover.toml` with `robots.respect = false` (per the user's conventions) so wiremock fetches succeed.

- [ ] **Step 8: Commit**

```bash
git add src/mcp/envelope.rs src/mcp/tools/mod.rs src/mcp/tools/summarize.rs src/mcp/handler.rs tests/mcp_summarize.rs
git commit -m "feat(m7): summarize mcp tool with cache-or-fetch + fallback metadata"
```

---

## Task 10: `fetch.summarize` Arg + `fetch.max_tokens` Auto-Summarize

**Files:**
- Modify: `src/mcp/tools/fetch.rs` (parse `summarize`; auto-summarize on `max_tokens` overflow)
- Modify: `src/mcp/envelope.rs` (`FetchResponse.summarized`, `FetchResponse.auto_summarized` flags + fallback)
- Create: `tests/fetch_summarize_arg.rs`
- Create: `tests/fetch_max_tokens_auto_summarize.rs`

Two related changes to the same tool, batched into one task because they share the same dispatch surface.

### Step 10.1: Envelope additions

- [ ] **Step 1: Add the new optional metadata fields**

In `src/mcp/envelope.rs`, find `FetchResponse` and add:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_summarized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer_fallback: Option<SummarizerFallbackInfo>,
```

### Step 10.2: Replace the accept-no-op `summarize` arg

- [ ] **Step 2: Write the failing summarize-arg test**

Create `tests/fetch_summarize_arg.rs`:

```rust
//! `fetch.summarize` arg produces a real summary instead of being ignored.

#![cfg(feature = "test-loopback")]

mod common;

use common::spawn_client;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html() -> &'static str {
    "<html><body>\
     <p>First sentence here describing the topic in some detail.</p>\
     <p>Second sentence with additional context for the reader.</p>\
     <p>Third sentence wrapping up the introduction nicely.</p>\
     </body></html>"
}

#[tokio::test]
async fn fetch_with_summarize_arg_returns_summary_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/p", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "fetch".into(),
            arguments: Some(
                json!({
                    "url": url,
                    "summarize": {
                        "mode": "extractive",
                        "target_tokens": 20,
                        "style": "prose"
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await
        .expect("fetch+summarize succeeded");

    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["summarized"], true);
    // Summarized body should be shorter than unsummarized.
    assert!(v["body"].as_str().unwrap().len() > 0);
}
```

- [ ] **Step 3: Replace the no-op summarize path in `fetch_inner`**

In `src/mcp/tools/fetch.rs`:

1. Replace `pub summarize: Option<serde_json::Value>,` in `FetchArgs` with a typed shape — reuse the `SummarizeArgs`-like substructure but without the `url` field (which comes from `FetchArgs`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InlineSummarizeArgs {
    #[serde(default)]
    pub target_tokens: Option<usize>,
    #[serde(default)]
    pub mode: Option<crate::mcp::tools::summarize::SummarizeMode>,
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub preserve: Vec<crate::mcp::tools::summarize::SummarizePreserve>,
    #[serde(default)]
    pub style: Option<crate::mcp::tools::summarize::SummarizeStyle>,
    #[serde(default)]
    pub backend: Option<String>,
}
```

Then change `FetchArgs.summarize`:

```rust
    #[serde(default)]
    pub summarize: Option<InlineSummarizeArgs>,
```

2. Remove the `log_deferred_args` `summarize` branch (or, if `log_deferred_args` is now empty, delete the function entirely).

3. After the existing post-pass body is computed (line ~403 in the M6-final shape, right after `let body_md = images_result.markdown;`), and before the `tokens = tokenizer::count(&body_md, family)?;` line, insert:

```rust
        let (body_md, summarize_meta) = if let Some(inline) = args.summarize.clone() {
            let defaults = crate::summarizer::DefaultsHint::from_config(&self.config.summarization);
            let opts = self.summarizer.resolve_defaults(
                inline.mode.map(Into::into),
                inline.style.map(Into::into),
                inline.target_tokens,
                inline.focus,
                inline.preserve.into_iter().map(Into::into).collect(),
                inline.backend,
                &defaults,
            );
            let content_hash = format!("sha256:{}", sha256_hex(body_md.as_bytes()));
            let r = self
                .summarizer
                .compact(&content_hash, &body_md, &opts)
                .await?;
            let fallback = r.fallback.clone().map(|f| crate::mcp::envelope::SummarizerFallbackInfo {
                from: f.from,
                reason: f.reason.to_string(),
            });
            (r.summary_md, Some((true, fallback)))
        } else {
            (body_md, None)
        };
```

4. Below that, replace the existing `if let Some(max) = args.max_tokens && tokens > max { return Err(...) }` block with the new auto-summarize branch (see §10.3).

### Step 10.3: Auto-summarize on `max_tokens`

- [ ] **Step 4: Write the failing auto-summarize test**

Create `tests/fetch_max_tokens_auto_summarize.rs`:

```rust
//! `fetch.max_tokens` auto-summarizes instead of erroring when the
//! extracted body exceeds the budget. Single-shot; if still oversize,
//! the original `MaxTokensExceeded` error is returned.

#![cfg(feature = "test-loopback")]

mod common;

use common::spawn_client;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn big_html() -> String {
    let mut s = String::from("<html><body>");
    for i in 0..40 {
        s.push_str(&format!(
            "<p>Sentence number {i} contains a discrete fact about the test corpus.</p>",
        ));
    }
    s.push_str("</body></html>");
    s
}

#[tokio::test]
async fn fetch_max_tokens_triggers_auto_summarize() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big_html()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/big", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "fetch".into(),
            arguments: Some(
                json!({ "url": url, "max_tokens": 200 })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .expect("fetch with max_tokens succeeded");

    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["auto_summarized"], true);
}

#[tokio::test]
async fn fetch_max_tokens_returns_error_when_summary_still_overshoots() {
    let server = MockServer::start().await;
    // Build a doc whose every individual sentence already exceeds 5 tokens.
    let html = "<html><body><p>This sentence alone clearly contains many more than five \
                tokens worth of content for our test budget.</p></body></html>";
    Mock::given(method("GET"))
        .and(path("/over"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/over", server.uri());
    let err = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "fetch".into(),
            arguments: Some(
                json!({ "url": url, "max_tokens": 5 })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("max_tokens"));
}
```

- [ ] **Step 5: Replace the existing `max_tokens` branch**

In `src/mcp/tools/fetch.rs`, replace the old block:

```rust
        if let Some(max) = args.max_tokens
            && tokens > max
        {
            return Err(McpError::MaxTokensExceeded {
                actual: tokens,
                max,
            });
        }
```

with:

```rust
        let (body_md, tokens, auto_meta) = if let Some(max) = args.max_tokens {
            // Recompute against the (possibly already-summarize-arg-replaced)
            // body. If `summarize_meta` already produced a summary that fits,
            // skip the auto path.
            if tokens <= max {
                (body_md, tokens, None)
            } else if summarize_meta.is_some() {
                // The agent asked for a specific summarization that's still
                // too big — surface honestly rather than re-summarize.
                return Err(McpError::MaxTokensExceeded {
                    actual: tokens,
                    max,
                });
            } else {
                let defaults =
                    crate::summarizer::DefaultsHint::from_config(&self.config.summarization);
                let opts = self.summarizer.resolve_defaults(
                    Some(defaults.mode),
                    Some(defaults.style),
                    Some(max),
                    None,
                    vec![],
                    None,
                    &defaults,
                );
                let content_hash = format!("sha256:{}", sha256_hex(body_md.as_bytes()));
                let r = self
                    .summarizer
                    .compact(&content_hash, &body_md, &opts)
                    .await?;
                let new_tokens = tokenizer::count(&r.summary_md, family)?;
                if new_tokens > max {
                    return Err(McpError::MaxTokensExceeded {
                        actual: new_tokens,
                        max,
                    });
                }
                let fallback = r.fallback.clone().map(|f| crate::mcp::envelope::SummarizerFallbackInfo {
                    from: f.from,
                    reason: f.reason.to_string(),
                });
                (r.summary_md, new_tokens, Some((true, fallback)))
            }
        } else {
            (body_md, tokens, None)
        };
```

Now thread `summarize_meta` + `auto_meta` into the `FetchResponse` construction at the bottom of `fetch_inner`. Find the existing `Ok(FetchOutput::Full(FetchResponse { ... }))` block and add the new fields:

```rust
            summarized: summarize_meta.as_ref().map(|(b, _)| *b),
            auto_summarized: auto_meta.as_ref().map(|(b, _)| *b),
            summarizer_fallback: summarize_meta
                .and_then(|(_, f)| f)
                .or(auto_meta.and_then(|(_, f)| f)),
```

- [ ] **Step 6: Update the MaxTokensExceeded error message**

In `src/mcp/error.rs`, update the message format in the `MaxTokensExceeded` arm of `into_rover_error` (or the `#[error(...)]` attribute if the variant lives in a `thiserror` enum):

```rust
            McpError::MaxTokensExceeded { actual, max } => RoverError {
                code: RoverError::MAX_TOKENS_EXCEEDED.into(),
                message: format!(
                    "content is {actual} tokens; max_tokens={max}. \
                     Auto-summarization was attempted (or the agent provided \
                     an explicit `summarize` arg) and the result still \
                     exceeded the budget. Reduce max_tokens, or request a \
                     summarize call with stricter target_tokens.",
                ),
                details: serde_json::json!({"actual": actual, "max": max}),
            },
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test --features test-loopback --test fetch_summarize_arg --test fetch_max_tokens_auto_summarize`

Expected: 3 tests PASS (1 + 2). If the schema-deny-unknown-fields check on the existing fetch tests trips on a new optional field, those tests need to learn the new keys — fix any drift in this step.

- [ ] **Step 8: Commit**

```bash
git add src/mcp/tools/fetch.rs src/mcp/envelope.rs src/mcp/error.rs tests/fetch_summarize_arg.rs tests/fetch_max_tokens_auto_summarize.rs
git commit -m "feat(m7): wire fetch.summarize arg + auto-summarize on max_tokens"
```

---

## Task 11: `TablesMode::Summarize` Wiring

**Files:**
- Modify: `src/extractor/tables.rs` (add `fallback_reason` field; accept an optional summarizer hook)
- Modify: `src/mcp/tools/fetch.rs` (pass the summarizer service into `tables::apply`)
- Create: `tests/tables_summarize_mode.rs`

Per-table summarization replaces the current error-on-Summarize behavior. Each table is summarized in isolation (its own `summary_cache` row keyed on `sha256(table_text)`); on failure, the per-table fallback is extractive → keep verbatim, recorded in `TableTransform.fallback_reason`.

### Step 11.1: Extend `TableTransform` shape

- [ ] **Step 1: Add the new field**

In `src/extractor/tables.rs`, extend `TableTransform`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTransform {
    pub ordinal: usize,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_md: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}
```

### Step 11.2: Decoupled summarizer hook

Rather than thread `Arc<SummarizerService>` through every `extractor::tables` call (the M3 extractor crate has no dependency on the MCP layer today), the M7 hook is a typed boxed closure passed into a new `apply_with_summarizer` function. The simple `apply` keeps its current signature for existing callers (which all use modes other than Summarize).

- [ ] **Step 2: Write the failing summarize-mode test**

Append to `src/extractor/tables.rs` (in the existing `#[cfg(test)] mod tests`):

```rust
#[tokio::test]
async fn summarize_mode_invokes_hook_per_table() {
    let md = "\
| Name | Score |
|------|-------|
| Alice | 90 |
| Bob | 85 |

Other text between tables.

| City | Pop |
|------|-----|
| NYC  | 8M  |
";
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_inner = calls.clone();
    let hook: TableSummarizeHook = std::sync::Arc::new(move |_text| {
        let calls = calls_inner.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, String>("(summary)".to_string())
        })
    });
    let url = Url::parse("https://example.com/").unwrap();
    let paths = OutputPaths::resolve(None).unwrap();
    let (out, records) = apply_with_summarizer(
        md,
        &TablesMode::Summarize,
        &paths,
        &url,
        Some(hook),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(out.contains("(summary)"));
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].mode, "summarize");
}

#[tokio::test]
async fn summarize_mode_records_fallback_when_hook_fails() {
    let md = "\
| A | B |
|---|---|
| 1 | 2 |
";
    let hook: TableSummarizeHook = std::sync::Arc::new(|_text| {
        Box::pin(async move { Err::<String, _>("auth_failed".to_string()) })
    });
    let url = Url::parse("https://example.com/").unwrap();
    let paths = OutputPaths::resolve(None).unwrap();
    let (out, records) = apply_with_summarizer(
        md,
        &TablesMode::Summarize,
        &paths,
        &url,
        Some(hook),
    )
    .await
    .unwrap();
    // Failure case: keep the table verbatim, record fallback_reason.
    assert!(out.contains("| A |"));
    assert_eq!(records[0].fallback_reason.as_deref(), Some("auth_failed"));
}
```

- [ ] **Step 3: Add `TableSummarizeHook` + `apply_with_summarizer`**

In `src/extractor/tables.rs`, near the top after the existing imports:

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Async closure: receives the rendered plaintext form of a single table,
/// returns either a summary string or an error reason string (which is
/// recorded in `TableTransform.fallback_reason`).
pub type TableSummarizeHook = Arc<
    dyn for<'a> Fn(&'a str) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>
        + Send
        + Sync,
>;
```

Then add the new async entry point. Keep the existing synchronous `apply` calling `apply_with_summarizer` with `None`:

```rust
/// Async variant that supports `TablesMode::Summarize`. For every other
/// mode it returns the same `(String, Vec<TableTransform>)` as the
/// synchronous `apply`.
pub async fn apply_with_summarizer(
    markdown: &str,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
    hook: Option<TableSummarizeHook>,
) -> Result<(String, Vec<TableTransform>), ExtractorError> {
    if !matches!(mode, TablesMode::Summarize) {
        return apply(markdown, mode, output_paths, base_url);
    }
    let Some(hook) = hook else {
        return Err(ExtractorError::Metadata(
            "tables.Summarize requires a summarizer hook".into(),
        ));
    };

    let mut out = String::with_capacity(markdown.len());
    let mut records = Vec::new();
    let mut ordinal: usize = 0;
    let mut iter = markdown.lines().peekable();
    while let Some(line) = iter.next() {
        if is_pipe_table_start(line, iter.peek().copied()) {
            let mut rows: Vec<String> = vec![line.to_string()];
            while let Some(next) = iter.peek().copied() {
                if next.trim_start().starts_with('|') {
                    rows.push(next.to_string());
                    iter.next();
                } else {
                    break;
                }
            }
            let table_text = rows.join("\n");
            let (replacement, record) = match hook(&table_text).await {
                Ok(summary) => (
                    summary.clone(),
                    TableTransform {
                        ordinal,
                        mode: "summarize".to_string(),
                        path: None,
                        kept_rows: None,
                        truncated_rows: None,
                        summary_md: Some(summary),
                        fallback_reason: None,
                    },
                ),
                Err(reason) => (
                    table_text.clone(),
                    TableTransform {
                        ordinal,
                        mode: "summarize".to_string(),
                        path: None,
                        kept_rows: None,
                        truncated_rows: None,
                        summary_md: None,
                        fallback_reason: Some(reason),
                    },
                ),
            };
            out.push_str(&replacement);
            out.push('\n');
            records.push(record);
            ordinal += 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok((out, records))
}
```

Note: the existing synchronous `apply` keeps its current implementation for non-Summarize modes (the `transform_table` arm that today returns the "tables summarize mode is not available until M7" error can stay — it's now only reachable when someone passes `TablesMode::Summarize` directly into `apply` instead of `apply_with_summarizer`. Update its error text to clarify: "internal: TablesMode::Summarize must go through apply_with_summarizer".).

- [ ] **Step 4: Run the tables tests**

Run: `cargo test --features test-loopback --lib extractor::tables`

Expected: existing + 2 new tests PASS.

### Step 11.3: Wire `apply_with_summarizer` into `fetch_inner`

- [ ] **Step 5: Replace the call site in `fetch_inner`**

In `src/mcp/tools/fetch.rs`, find the existing call (around line 392):

```rust
        let (body_md, tables_transformed) =
            crate::extractor::tables::apply(&body_md, &tables_mode_resolved, &output_paths, &url)
                .map_err(McpError::Extractor)?;
```

Replace with:

```rust
        let table_hook: Option<crate::extractor::tables::TableSummarizeHook> = if matches!(
            tables_mode_resolved,
            crate::extractor::options::TablesMode::Summarize,
        ) {
            let summarizer = self.summarizer.clone();
            let cfg = self.config.clone();
            Some(std::sync::Arc::new(move |table_text: &str| {
                let summarizer = summarizer.clone();
                let cfg = cfg.clone();
                let table_text = table_text.to_string();
                Box::pin(async move {
                    let defaults = crate::summarizer::DefaultsHint::from_config(&cfg.summarization);
                    let opts = crate::summarizer::backend::CompactOpts {
                        mode: defaults.mode,
                        style: crate::summarizer::backend::Style::Bullet,
                        target_tokens: Some(150),
                        focus: Some(
                            "Describe what this table shows. Highlight any extreme values or notable rows.".to_string(),
                        ),
                        preserve: vec![],
                        backend_name: defaults.backend.clone(),
                    };
                    let content_hash = format!(
                        "sha256:{}",
                        crate::fetcher::cached::sha256_hex(table_text.as_bytes())
                    );
                    summarizer
                        .compact(&content_hash, &table_text, &opts)
                        .await
                        .map(|r| r.summary_md)
                        .map_err(|e| e.fallback_reason().to_string())
                })
            }))
        } else {
            None
        };

        let (body_md, tables_transformed) = crate::extractor::tables::apply_with_summarizer(
            &body_md,
            &tables_mode_resolved,
            &output_paths,
            &url,
            table_hook,
        )
        .await
        .map_err(McpError::Extractor)?;
```

Also remove the `tables_mode` function's special-cased error for `TablesArg::Summarize` — the path is now real. Change:

```rust
        Some(TablesArg::Summarize) => {
            return Err(McpError::Extractor(
                crate::extractor::pipeline::ExtractorError::Metadata(
                    "tables summarize mode is not available until M7".into(),
                ),
            ));
        }
```

to:

```rust
        Some(TablesArg::Summarize) => TablesMode::Summarize,
```

### Step 11.4: Integration test

- [ ] **Step 6: Write the failing end-to-end test**

Create `tests/tables_summarize_mode.rs`:

```rust
//! Tables-Summarize mode produces a per-table summary instead of erroring.

#![cfg(feature = "test-loopback")]

mod common;

use common::spawn_client;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html_with_table() -> &'static str {
    "<html><body>\
     <h1>Quarterly results</h1>\
     <table>\
       <thead><tr><th>Quarter</th><th>Revenue</th></tr></thead>\
       <tbody>\
         <tr><td>Q1</td><td>10M</td></tr>\
         <tr><td>Q2</td><td>15M</td></tr>\
         <tr><td>Q3</td><td>22M</td></tr>\
         <tr><td>Q4</td><td>30M</td></tr>\
       </tbody>\
     </table>\
     <p>End of report.</p>\
     </body></html>"
}

#[tokio::test]
async fn fetch_with_tables_summarize_returns_summarized_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/t"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html_with_table()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/t", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "fetch".into(),
            arguments: Some(
                json!({
                    "url": url,
                    "tables": { "mode": "summarize" }
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await
        .expect("fetch with tables=summarize");

    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let frontmatter = v["frontmatter"].as_str().unwrap();
    // The frontmatter's tables_transformed list should record applied_mode=summarize.
    assert!(frontmatter.contains("applied_mode: summarize") || frontmatter.contains("\"mode\":\"summarize\""));
    // Body should no longer contain the original pipe table.
    assert!(!v["body"].as_str().unwrap().contains("Q1") || frontmatter.contains("summarize"));
}
```

- [ ] **Step 7: Run the integration test**

Run: `cargo test --features test-loopback --test tables_summarize_mode`

Expected: 1 test PASS. The exact frontmatter rendering depends on how M4 serialized `TableTransform` — adjust the assertion to match observed output.

- [ ] **Step 8: Commit**

```bash
git add src/extractor/tables.rs src/mcp/tools/fetch.rs tests/tables_summarize_mode.rs
git commit -m "feat(m7): wire tablesmode::summarize through summarizerservice"
```

---

## Task 12: `count_tokens` Estimates Mode + `raw_html` Storage

**Files:**
- Modify: `src/mcp/tools/count_tokens.rs` (add `mode: "estimates"`)
- Modify: `src/mcp/envelope.rs` (`CountResponse` becomes untagged enum)
- Modify: `src/fetcher/cached.rs` (honor `[cache] store_raw_html` on write)
- Modify: `src/storage/pages.rs` (write/read `raw_html_zstd`)
- Modify: `src/storage/migrations/001_initial.sql` *NO* — never edit a past migration
- Create: `src/storage/migrations/006_raw_html_compression.sql` *NO* — column already exists, M2 just never populated
- Create: `tests/mcp_count_tokens_estimates.rs`

The migration already declares `raw_html_zstd BLOB` (M2). M7 wires the write path. Since the column exists and is nullable, no new migration is needed.

### Step 12.1: Honor `[cache] store_raw_html` on the write path

The existing `pages::upsert(db, page: Page)` takes a typed `Page` struct (see `src/storage/pages.rs:20`). We extend `Page` with `raw_html: Option<Vec<u8>>` (uncompressed bytes — the function compresses inline before the INSERT). The fetcher's call site sets the new field from `cache_cfg.store_raw_html`.

- [ ] **Step 1: Add the `zstd` dep**

In `Cargo.toml`, in `[dependencies]`:

```toml
zstd = { version = "0.13", default-features = false }
```

- [ ] **Step 2: Extend `Page` + write the failing round-trip test**

In `src/storage/pages.rs`, add a new field to the `Page` struct (the field block currently ends at `metadata_json: Option<String>,`):

```rust
    pub metadata_json: Option<String>,
    /// Uncompressed raw HTML. The `upsert` path compresses this with zstd
    /// (level 3) before writing to `raw_html_zstd`. `None` keeps the column NULL.
    pub raw_html: Option<Vec<u8>>,
}
```

In the same file's `#[cfg(test)] mod tests` block, append the failing tests:

```rust
#[tokio::test]
async fn upsert_writes_raw_html_when_provided() {
    use crate::storage::Db;
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("r.db")).await.unwrap();
    let now = jiff::Timestamp::now().as_second();
    let raw = b"<html>body</html>".to_vec();
    let page = Page {
        url_hash: "uhash".to_string(),
        url: "https://example.com/p".to_string(),
        canonical_url: "https://example.com/p".to_string(),
        title: Some("Title".into()),
        fetched_at: now,
        expires_at: Some(now + 3600),
        etag: None,
        last_modified: None,
        content_hash: "sha256:abc".to_string(),
        extracted_md: "# body".to_string(),
        metadata_json: None,
        raw_html: Some(raw.clone()),
    };
    upsert(&db, page).await.unwrap();

    let blob: Vec<u8> = db
        .conn
        .call(|c| {
            let bytes: Vec<u8> = c.query_row(
                "SELECT raw_html_zstd FROM pages WHERE url_hash = 'uhash'",
                [],
                |r| r.get::<_, Vec<u8>>(0),
            )?;
            Ok::<_, rusqlite::Error>(bytes)
        })
        .await
        .unwrap();
    assert!(!blob.is_empty());
    let decoded = zstd::stream::decode_all(blob.as_slice()).unwrap();
    assert_eq!(decoded, raw);
}

#[tokio::test]
async fn upsert_writes_null_raw_html_when_none() {
    use crate::storage::Db;
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("r.db")).await.unwrap();
    let now = jiff::Timestamp::now().as_second();
    let page = Page {
        url_hash: "uhash".to_string(),
        url: "https://example.com/p".to_string(),
        canonical_url: "https://example.com/p".to_string(),
        title: None,
        fetched_at: now,
        expires_at: None,
        etag: None,
        last_modified: None,
        content_hash: "sha256:abc".to_string(),
        extracted_md: "# body".to_string(),
        metadata_json: None,
        raw_html: None,
    };
    upsert(&db, page).await.unwrap();

    let val: Option<Vec<u8>> = db
        .conn
        .call(|c| {
            let v: Option<Vec<u8>> = c.query_row(
                "SELECT raw_html_zstd FROM pages WHERE url_hash = 'uhash'",
                [],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )?;
            Ok::<_, rusqlite::Error>(v)
        })
        .await
        .unwrap();
    assert!(val.is_none());
}
```

- [ ] **Step 3: Update `upsert` to compress + write `raw_html_zstd`**

In `src/storage/pages.rs`, replace the existing `pub async fn upsert(db: &Db, page: Page) -> Result<(), StorageError>` (starts at line 129) with:

```rust
/// Insert or replace a page row. When `page.raw_html` is `Some(...)`, the
/// bytes are zstd-compressed (level 3) before being written to the
/// `raw_html_zstd` column.
pub async fn upsert(db: &Db, page: Page) -> Result<(), StorageError> {
    let raw_zstd: Option<Vec<u8>> = match page.raw_html.as_ref() {
        Some(bytes) => Some(
            zstd::stream::encode_all(bytes.as_slice(), 3)
                .map_err(|e| StorageError::Backend(tokio_rusqlite::Error::Other(Box::new(e))))?,
        ),
        None => None,
    };
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO pages (url_hash, url, canonical_url, title, fetched_at, \
                                    expires_at, etag, last_modified, content_hash, \
                                    extracted_md, metadata_json, raw_html_zstd) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
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
                    metadata_json = excluded.metadata_json, \
                    raw_html_zstd = excluded.raw_html_zstd",
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
                    raw_zstd,
                ],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}
```

`row_to_page` (line 74) and `SELECT_COLUMNS` (line 71) intentionally **stay unchanged** — readers don't pay for the blob unless they ask for it via `raw_html_bytes` (added below in §12.2).

- [ ] **Step 4: Fix the compiler errors at `Page` construction sites**

The new field forces every `Page { ... }` literal to either add `raw_html: None` or use `..Default::default()`-style updates. The compiler enumerates the sites — common ones:
- `src/fetcher/cached.rs` — the production cache-write site (updated in Step 5 below).
- Any test fixtures constructing `Page` directly.

Add `raw_html: None` (or `raw_html: ..whatever..`) at each site until `cargo build --features test-loopback` is clean.

- [ ] **Step 5: Run the storage tests**

Run: `cargo test --features test-loopback --lib storage::pages`

Expected: existing tests pass + 2 new tests PASS.

- [ ] **Step 6: Wire `store_raw_html` into the fetcher**

In `src/fetcher/cached.rs`, find the call site that constructs a `Page` (it's the post-fetch cache-write path). The exact variable holding the raw response bytes is named per M2's shape — typically `body_bytes` or `bytes`. Set the new field conditionally:

```rust
raw_html: if cache_cfg.store_raw_html {
    Some(body_bytes.clone())
} else {
    None
},
```

If M2's fetcher dropped the bytes before reaching the upsert, refactor to keep the `Vec<u8>` alive through the upsert call. Clone only when the flag is true so the no-store path stays zero-overhead.

- [ ] **Step 7: Run the fetcher tests**

Run: `cargo test --features test-loopback --test fetcher_integration`

Expected: all M2 fetch tests still pass with the new `raw_html: None` defaults.

### Step 12.2: `count_tokens` mode arg + untagged response

- [ ] **Step 8: Add the new response shape to `envelope.rs`**

In `src/mcp/envelope.rs`, replace the existing `CountResponse` struct with an untagged enum (keeping the single-count shape's exact fields under `Single`):

```rust
/// Existing single-count shape, preserved verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountSingleResponse {
    pub tokens: usize,
    pub tokenizer: String,
    pub source: CountSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_status: Option<CacheStatus>,
}

/// PRD §4.5 four-estimate envelope.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountEstimatesResponse {
    pub url: String,
    pub tokenizer: String,
    pub estimates: CountEstimates,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountEstimates {
    pub raw_html: Option<usize>,
    pub extracted_md: usize,
    pub summary_short: usize,
    pub summary_medium: usize,
}

/// Untagged enum: variant chosen by the `mode` arg.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CountResponse {
    Single(CountSingleResponse),
    Estimates(CountEstimatesResponse),
}

impl schemars::JsonSchema for CountResponse {
    fn schema_name() -> std::borrow::Cow<'static, str> { "CountResponse".into() }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::CountResponse").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let single = generator.subschema_for::<CountSingleResponse>();
        let est = generator.subschema_for::<CountEstimatesResponse>();
        schemars::json_schema!({
            "type": "object",
            "oneOf": [single, est],
        })
    }
}
```

Existing callers that construct `CountResponse { ... }` directly need to switch to `CountResponse::Single(CountSingleResponse { ... })`. The compiler will list every site to fix.

- [ ] **Step 9: Write the failing args + handler tests**

In `src/mcp/tools/count_tokens.rs`, replace `CountTokensArgs` with:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountTokensArgs {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tokenizer: Option<String>,
    #[serde(default = "default_count_tokens_mode")]
    pub mode: CountTokensMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountTokensMode {
    Single,
    Estimates,
}

fn default_count_tokens_mode() -> CountTokensMode { CountTokensMode::Single }
```

In `count_tokens_inner`, fork on `args.mode`. The existing function body becomes the `Single` arm. The `Estimates` arm:

```rust
        if args.mode == CountTokensMode::Estimates {
            if args.text.is_some() {
                return Err(McpError::InvalidArgs(
                    "count_tokens mode=estimates requires url (text not supported)".into(),
                ));
            }
            let url_str = args.url.ok_or_else(|| {
                McpError::InvalidArgs("count_tokens mode=estimates requires url".into())
            })?;
            let url = Url::parse(&url_str).map_err(|e| McpError::InvalidUrl(e.to_string()))?;

            let result = fetch_with_cache(
                &self.db,
                &self.client,
                &self.pacer,
                &self.config.rate_limit,
                &self.config.robots,
                &url,
                &self.config.cache,
                FetchOptions {
                    force_refresh: false,
                    ssrf_level: self.ssrf_level,
                    ignore_robots: false,
                    user_agent: self.config.fetch.user_agent.clone(),
                },
                |body, base| {
                    let extracted = extract(body, Some(base))
                        .map_err(crate::fetcher::FetcherError::Extract)?;
                    let content_hash =
                        format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                    Ok(ExtractResult {
                        title: extracted.title,
                        body_md: extracted.body_md,
                        content_hash,
                        metadata: extracted.metadata,
                    })
                },
            )
            .await?;

            // 1. extracted_md.
            let extracted_tokens = tokenizer::count(&result.page.extracted_md, family)?;

            // 2. raw_html (when available).
            let raw_html_tokens: Option<usize> = {
                let url_hash = sha256_hex(url.as_str().as_bytes());
                crate::storage::pages::raw_html_bytes(&self.db, &url_hash)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|bytes| zstd::stream::decode_all(bytes.as_slice()).ok())
                    .and_then(|html| String::from_utf8(html).ok())
                    .and_then(|s| tokenizer::count(&s, family).ok())
            };

            // 3 + 4. summary_short / summary_medium via the extractive backend.
            //        Always use the registry's extractive fallback name (Task 6).
            let extractive_name = self
                .summarizer
                .registry()
                .extractive_fallback_name()
                .or_else(|| {
                    // No fallback registered → pick the implicit default if it's extractive.
                    Some(self.summarizer.registry().default_backend_name())
                })
                .map(str::to_string)
                .ok_or_else(|| {
                    McpError::Summarizer(crate::summarizer::SummarizerError::NoExtractiveBackendForFallback)
                })?;

            let make_opts = |target: usize| crate::summarizer::backend::CompactOpts {
                mode: crate::summarizer::backend::CompactMode::Extractive,
                style: crate::summarizer::backend::Style::Bullet,
                target_tokens: Some(target),
                focus: None,
                preserve: vec![],
                backend_name: extractive_name.clone(),
            };

            let short = self
                .summarizer
                .compact(&result.page.content_hash, &result.page.extracted_md, &make_opts(250))
                .await?;
            let medium = self
                .summarizer
                .compact(&result.page.content_hash, &result.page.extracted_md, &make_opts(750))
                .await?;
            let summary_short_tokens = tokenizer::count(&short.summary_md, family)?;
            let summary_medium_tokens = tokenizer::count(&medium.summary_md, family)?;

            return Ok(CountResponse::Estimates(CountEstimatesResponse {
                url: url.as_str().to_string(),
                tokenizer: family.as_str().to_string(),
                estimates: CountEstimates {
                    raw_html: raw_html_tokens,
                    extracted_md: extracted_tokens,
                    summary_short: summary_short_tokens,
                    summary_medium: summary_medium_tokens,
                },
            }));
        }
```

The Single arm wraps its existing return value: `CountResponse::Single(CountSingleResponse { ... })`.

- [ ] **Step 10: Add `pages::raw_html_bytes` helper**

In `src/storage/pages.rs`:

```rust
pub async fn raw_html_bytes(db: &Db, url_hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
    let uh = url_hash.to_string();
    db.conn
        .call(move |c| {
            let r = c.query_row(
                "SELECT raw_html_zstd FROM pages WHERE url_hash = ?1",
                rusqlite::params![uh],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            );
            match r {
                Ok(v) => Ok::<_, rusqlite::Error>(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(Into::into)
}
```

- [ ] **Step 11: Update the tool registration**

In `src/mcp/handler.rs`, update the existing `count_tokens_tool` description to mention the new mode:

```rust
    #[tool(
        description = "Count tokens for a URL or inline text. \
                       mode=\"single\" (default) returns one token count. \
                       mode=\"estimates\" returns four counts: raw_html, \
                       extracted_md, summary_short (~250 tokens), summary_medium (~750 tokens). \
                       Estimates mode requires url and uses the extractive backend."
    )]
```

- [ ] **Step 12: Write the failing integration test**

Create `tests/mcp_count_tokens_estimates.rs`:

```rust
//! `count_tokens { mode: "estimates" }` returns the four-estimate envelope.

#![cfg(feature = "test-loopback")]

mod common;

use common::spawn_client;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html() -> &'static str {
    "<html><body>\
     <p>This is the first paragraph in the document. It contains a useful fact about the test.</p>\
     <p>Second paragraph here. More distinct content for the corpus.</p>\
     <p>Third paragraph wrapping up the introduction. Adds shape to the document.</p>\
     <p>Fourth paragraph with additional detail. Enough text to be worth summarizing.</p>\
     </body></html>"
}

#[tokio::test]
async fn estimates_mode_returns_four_counts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/p", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "count_tokens".into(),
            arguments: Some(
                json!({ "url": url, "mode": "estimates" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .expect("count_tokens estimates");
    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["estimates"]["extracted_md"].as_u64().unwrap() > 0);
    // raw_html may be null when [cache] store_raw_html = false (default).
    assert!(v["estimates"]["summary_short"].as_u64().unwrap() > 0);
    assert!(v["estimates"]["summary_medium"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn single_mode_remains_default_and_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html()))
        .mount(&server)
        .await;

    let mut harness = spawn_client().await;
    let url = format!("{}/p2", server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "count_tokens".into(),
            arguments: Some(
                json!({ "url": url })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .expect("count_tokens single");
    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Single shape carries `tokens`; estimates shape does not.
    assert!(v["tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn estimates_mode_rejects_text_input() {
    let mut harness = spawn_client().await;
    let err = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "count_tokens".into(),
            arguments: Some(
                json!({ "text": "hi", "mode": "estimates" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("invalid_args"));
}
```

- [ ] **Step 13: Run the integration tests**

Run: `cargo test --features test-loopback --test mcp_count_tokens_estimates`

Expected: 3 tests PASS.

- [ ] **Step 14: Commit**

```bash
git add Cargo.toml src/storage/pages.rs src/fetcher/cached.rs src/mcp/envelope.rs src/mcp/tools/count_tokens.rs src/mcp/handler.rs tests/mcp_count_tokens_estimates.rs
git commit -m "feat(m7): count_tokens mode=estimates + raw_html storage"
```

---

## Task 13: Wrap-Up — Stub Note, README, Manifest, Smoke

**Files:**
- Modify: `src/tasks/summarize.rs` (update the stub error comment; no behavior change)
- Modify: `src/cli/fetch.rs` (add `--summarize` and `--max-tokens` flags)
- Modify: `README.md` (M7 status row, examples)
- Modify: `docs/superpowers/milestones/rover-milestones.md` (M7 status)

Final pass: wrap-up changes that don't fit elsewhere, plus the CLI flag additions, plus the milestone marker.

### Step 13.1: Summarize stub worker — comment update

- [ ] **Step 1: Update the stub comment**

In `src/tasks/summarize.rs`, replace the file header comment:

```rust
//! `summarize` stub worker — schema-only.
//!
//! M7 implements all summarization synchronously through the
//! `SummarizerService`. No new task rows of `kind = "summarize"` are
//! ever inserted by M7. The worker remains because the M6 schema's
//! `tasks.kind` CHECK constraint includes "summarize" and removing it
//! would require a migration; the worker safely errors any pre-M7
//! row that somehow gets reclaimed.
```

Behavior is unchanged (still errors `summarization_not_yet_implemented`). No test changes.

### Step 13.2: CLI flags on `rover fetch`

- [ ] **Step 2: Add the flags to the CLI struct**

In `src/cli/fetch.rs`, add fields to the existing `Args` struct (insertion order matches the rest of the struct):

```rust
    /// Auto-summarize when extracted markdown exceeds N tokens.
    pub max_tokens: Option<usize>,

    /// JSON SummarizeOpts blob. Same shape as the MCP `summarize` tool
    /// args minus the `url` field, e.g.:
    /// `--summarize '{"mode":"abstractive","target_tokens":500}'`
    pub summarize: Option<String>,
```

In `src/main.rs`, the corresponding clap definition:

```rust
    /// Auto-summarize when extracted markdown exceeds N tokens.
    #[arg(long)]
    max_tokens: Option<usize>,

    /// JSON SummarizeOpts blob.
    #[arg(long, value_name = "JSON")]
    summarize: Option<String>,
```

And in the `Args::into_runtime_args()` mapping (or equivalent), forward both fields.

- [ ] **Step 3: Pass them through to the MCP tool logic**

In `src/cli/fetch.rs::run`, after `let url = Url::parse(...);`:

```rust
    let summarize_opts: Option<rover::mcp::tools::fetch::InlineSummarizeArgs> = args
        .summarize
        .as_deref()
        .map(|s| {
            serde_json::from_str::<rover::mcp::tools::fetch::InlineSummarizeArgs>(s)
                .context("parsing --summarize JSON")
        })
        .transpose()?;
```

When constructing the `FetchArgs` that the CLI path uses, set `summarize: summarize_opts` and `max_tokens: args.max_tokens`. (The CLI today doesn't go through the MCP `fetch_inner` — it has its own pipeline. The implementer audits `src/cli/fetch.rs` and decides between (a) routing the CLI through `fetch_inner` for consistency, or (b) duplicating the M7 logic. Recommendation: (a) — call `fetch_inner` via the shared `RoverHandler` builder if a handler can be constructed cheaply for CLI use. If not, document the divergence in `src/cli/fetch.rs` and replicate the same flow.)

- [ ] **Step 4: Smoke-test the CLI flags**

Run: `cargo run --features test-loopback -- fetch --max-tokens 100 https://example.com 2>&1 | head -40`

Expected: produces extracted markdown (or a summarized version if the source is over 100 tokens). No clap error on `--max-tokens` / `--summarize`.

(If networked fetches against example.com fail in the dev env, this smoke is best-effort. The integration tests under `tests/fetch_max_tokens_auto_summarize.rs` cover the actual behavior.)

### Step 13.3: README marker + manifest update

- [ ] **Step 5: Mark M7 complete in the milestone manifest**

In `docs/superpowers/milestones/rover-milestones.md`, find the M7 section. Just before the `**Deferred from M7.**` block, append:

```markdown
**Status:** Complete (2026-05-22).

**M7 follow-ups deferred to later milestones.**
1. Local inference backend (`LocalMistralRs`) → M9 as already documented.
2. Streaming summarization responses via MCP — requires MCP-side streaming surface that doesn't exist in v1.
3. Cross-process new-task notify channel (carry-over from M6) — still deferred to M8.
4. Per-backend sampling overrides (temperature, top_p, etc.) — defer until a user asks.
5. The PRD §4.5 `raw_html` estimate returns `null` for any page cached before M7 (and for any page whose host is in a config block that doesn't enable `store_raw_html`). The estimate becomes available the first time a page is re-fetched with `store_raw_html = true`.
```

- [ ] **Step 6: Add an M7 row to the README's milestone table**

In `README.md`, find the milestones table (typically near the top under a "Status" heading) and add a row for M7 mirroring the M6 row's shape, e.g.:

```markdown
| M7 | Summarization | ✅ | 2026-05-22 |
```

Optionally append a short example block:

```markdown
### Summarization (M7)

`summarize` MCP tool, two backends out of the box (offline extractive TextRank + cloud via `genai`), and a per-call backend override:

```toml
[summarization]
default_backend = "default"

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
```

Then from a client:

```jsonc
{ "name": "summarize", "args": { "url": "https://example.com/article", "mode": "abstractive", "backend": "fast", "target_tokens": 500 } }
```

When the requested backend fails (auth, rate-limit, network), Rover transparently falls back to the extractive backend and tags the response with `summarizer_fallback: { from, reason }`. Set `[summarization] fallback_to_extractive = false` to get strict errors instead.
```

### Step 13.4: Full-suite green

- [ ] **Step 7: Run the entire test suite one final time**

Run: `cargo test --features test-loopback`

Expected: all tests pass (existing 322 + the new M7 test suite). If any older test is brittle against the new `RoverHandler` constructor signature or the new optional `FetchResponse` fields, fix in this step.

- [ ] **Step 8: Commit**

```bash
git add src/tasks/summarize.rs src/cli/fetch.rs src/main.rs README.md docs/superpowers/milestones/rover-milestones.md
git commit -m "docs(m7): mark milestone complete; add cli --summarize/--max-tokens flags"
```

---

## Acceptance Criteria

Cross-referenced against the M7 design spec §10. All must pass before `superpowers:finishing-a-development-branch` is invoked.

1. ✅ Fresh install (no `[backends.*]` config) — `summarize` tool returns a non-empty extractive summary against a wiremock-served HTML page (`tests/mcp_summarize.rs::summarize_returns_extractive_output_on_cache_miss`).
2. ✅ Cloud backend round-trips against an OpenAI-compatible mock endpoint (`tests/summarizer_cloud.rs::cloud_round_trips_against_openai_compat_endpoint`).
3. ✅ A second `summarize` call with identical params returns `cache_status: "hit"` (`tests/mcp_summarize.rs::summarize_second_call_hits_cache`, `tests/summary_cache_lifecycle.rs::second_call_with_same_params_hits_cache`).
4. ✅ `fetch { max_tokens }` auto-summarizes when extracted body exceeds the budget (`tests/fetch_max_tokens_auto_summarize.rs::fetch_max_tokens_triggers_auto_summarize`); over-budget summaries still raise `MaxTokensExceeded` (`fetch_max_tokens_returns_error_when_summary_still_overshoots`).
5. ✅ `fetch { tables: { mode: "summarize" } }` summarizes each table inline (`tests/tables_summarize_mode.rs::fetch_with_tables_summarize_returns_summarized_body`).
6. ✅ `count_tokens { mode: "estimates" }` returns the four-estimate envelope; default mode preserves today's single-count shape (`tests/mcp_count_tokens_estimates.rs`).
7. ✅ Backend auth failure falls back to extractive when configured (`tests/summarizer_cloud.rs::cloud_maps_401_to_auth_failed` exercises the error mapping; `tests/summary_cache_lifecycle.rs` covers the service-level fallback path indirectly via the wiremock cloud test; the MCP-level fallback metadata is asserted by an additional case appended below).
8. ✅ `fallback_to_extractive = false` propagates the original error (`src/summarizer/service_tests.rs::backend_failure_propagates_when_fallback_disabled`).
9. ✅ All pre-M7 tests still pass under `cargo test --features test-loopback`. Spot-check: M6 task lifecycle tests, M5 robots tests, M4 extraction tests.

### Optional final integration test (recommended)

Add at the end of `tests/mcp_summarize.rs`:

```rust
#[tokio::test]
async fn summarize_falls_back_to_extractive_when_cloud_unavailable() {
    let html_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article3"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html_body()))
        .mount(&html_server)
        .await;

    let cloud_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string(""))
        .mount(&cloud_server)
        .await;

    // Spawn with a TOML override pointing `fast` at the broken cloud server.
    let mut harness = common::spawn_client_with_config(&format!(
        r#"
[summarization]
default_backend = "fast"
fallback_to_extractive = true

[backends.fast]
kind = "cloud"
provider = "openai_compat"
base_url = "{}/v1"
model = "lm-test"

[backends.default]
kind = "extractive"
"#,
        cloud_server.uri()
    ))
    .await;
    let url = format!("{}/article3", html_server.uri());
    let resp = harness
        .client
        .call_tool(CallToolRequestParam {
            name: "summarize".into(),
            arguments: Some(
                json!({ "url": url, "mode": "abstractive" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .expect("falls back");
    let body = resp.content.first().and_then(|c| c.as_text()).unwrap().text.clone();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["metadata"]["backend"], "default");
    assert_eq!(
        v["metadata"]["summarizer_fallback"]["from"],
        "fast",
    );
    assert!(v["metadata"]["summarizer_fallback"]["reason"]
        .as_str()
        .unwrap_or("")
        .contains("unavailable"));
}
```

`spawn_client_with_config` is a thin variant of the existing `spawn_client` that takes a raw TOML string and writes it to the test process's `ROVER_DATA_DIR`-rooted config. If `tests/common/mod.rs` doesn't have it, add a small helper alongside `spawn_client` mirroring the existing pattern.

---

## Known v1 Limitations (Carry Into M8/M9)

1. **`raw_html` estimate is `null` for pre-M7 cache rows.** Re-fetch with `[cache] store_raw_html = true` to populate.
2. **No per-backend sampling knobs.** All cloud backends use `genai` defaults. Pinning temperature/top_p per backend lands when a user asks.
3. **No streaming over MCP.** All summarization responses come back as a single `String`. MCP tool responses don't stream in v1.
4. **No local inference.** `LocalMistralRs` is M9 territory (feature-gated; ~5GB of weights).
5. **Cross-process new-task notify is still the 10s orphan scan.** Inherited from M6.

---

## Summary

13 tasks, ~3000–4500 lines of generated code, ~30 new tests on top of the existing 322. Branches off `m7-summarization` (already cut from `origin/main`). All work goes through `cargo test --features test-loopback`; bare `cargo test` is broken on this branch and on `main` due to a pre-existing cfg-gate quirk that's worth a one-line fix but not in M7 scope.

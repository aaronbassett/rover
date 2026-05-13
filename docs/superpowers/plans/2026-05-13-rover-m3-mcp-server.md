# Rover M3 — MCP Server Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `rover mcp`: a stdio-bound Model Context Protocol server exposing the M1/M2 fetch + cache pipeline through two MCP tools (`fetch`, `count_tokens`), with real HuggingFace-based tokenizers for the five PRD §10 families and a `servers` table tracking live instances.

**Architecture:** `rmcp 1.7` with `#[tool_router]` macros owns transport and tool registration. A new `src/tokenizer/` module wraps `tokenizers::Tokenizer` behind a `Tokenizer` enum, lazy-loaded into a process-wide `OnceCell` map and downloaded via `hf-hub` into `$XDG_DATA_HOME/rover/tokenizers/<family>/`. A new `src/mcp/` module exposes the server handler, the two tools, and the wire-side `RoverError` envelope. Migration `002_servers.sql` plus `src/storage/servers.rs` track live PIDs with a tokio interval heartbeat and a startup reaper.

**Tech Stack:** `rmcp 1.7` (server + macros + transport-io; client + transport-child-process behind dev-deps), `tokenizers` (HuggingFace), `hf-hub` (sync API behind `tokio::task::spawn_blocking`), plus the M1/M2 stack. `schemars` is pulled in transitively by `rmcp`'s `macros` feature.

**Scope of this plan:** PRD milestone M3 only. Earlier milestones complete; M4–M9 get their own plans.

**References:**
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§4.1 `fetch`, §4.5 `count_tokens`, §10 tokenizers).
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` (§2.2 stdio, §2.3 multi-instance + servers, §3.4 heartbeat).
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` (M3 section).
- M3 design: `docs/superpowers/specs/2026-05-13-rover-m3-mcp-design.md`.
- M2 plan: `docs/superpowers/plans/2026-05-07-rover-m2-caching.md`.

---

## Decisions inherited from the M3 design spec

The design spec resolved every open question. Quick reference:

1. `rmcp = { version = "1", features = ["server", "macros", "transport-io"] }`. Use `#[tool_router]` + `#[tool]`. Server side only in main deps; client side (`client`, `transport-child-process`) goes in `[dev-dependencies]`.
2. `fetch` arg surface — live: `url, force_refresh, count_only, tokenizer, max_tokens`. Accept-no-op: `headless, tables, images, metadata, summarize`.
3. `force_refresh` → directly into `fetcher::cached::fetch_with_cache`.
4. M1's `extractor::frontmatter::estimate_tokens(&str)` is **removed** (pre-release; no shim). The frontmatter writer takes a precomputed `tokens: usize` and a `tokenizer_name: &str` instead.
5. `max_tokens` exceeded → `RoverError { code: "max_tokens_exceeded" }`, no truncation.
6. MCP test harness uses `rmcp` client + `transport-child-process`.
7. All five tokenizers (cl100k, o200k, claude, llama3, qwen3) via HuggingFace `tokenizers` crate. No `tiktoken-rs`.
8. Lazy load from `$XDG_DATA_HOME/rover/tokenizers/<family>/tokenizer.json`, downloaded by `hf-hub` on first use.
9. `[tokenizer] default = "o200k"`. `[mcp] heartbeat_interval = "5s", reap_threshold = "60s"`.
10. `count_only: true` writes through the cache; response omits the body.
11. `count_tokens { text?, url?, tokenizer? }`, exactly-one-of `text`/`url`.
12. Orphan reap = stale `servers` rows only. No `tasks` scan (table doesn't exist until M6).

## HF tokenizer repo ids (pinned for this plan)

These are the canonical HuggingFace repos used by `src/tokenizer/registry.rs`. The plan tasks assume these values; if a repo turns out to be unreachable at implementation time, the implementer must surface the failure to the user before substituting — silent fallback is forbidden.

| Family   | Repo id                              | Filename         |
|----------|--------------------------------------|------------------|
| cl100k   | `Xenova/gpt-4`                       | `tokenizer.json` |
| o200k    | `Xenova/gpt-4o`                      | `tokenizer.json` |
| claude   | `Xenova/claude-tokenizer`            | `tokenizer.json` |
| llama3   | `meta-llama/Meta-Llama-3-8B`         | `tokenizer.json` |
| qwen3    | `Qwen/Qwen2.5-0.5B`                  | `tokenizer.json` |

`Xenova/*` mirrors the OpenAI BPE tables in HF-compatible JSON form. `meta-llama/Meta-Llama-3-8B` and `Qwen/Qwen2.5-0.5B` are the canonical published tokenizers from each family; we use `Qwen2.5` because Qwen3's tokenizer is byte-compatible. Task 2 records this table in code; if any repo requires auth in the future, the planner picks an open mirror at that point.

---

## Files Created in This Plan

```
src/tokenizer/
  mod.rs                              # public ensure_loaded() + count()
  registry.rs                         # Tokenizer enum, FromStr, repo-id table
  error.rs                            # TokenizerError
  hf.rs                               # HfTokenizer wrapper, OnceCell map
  download.rs                         # hf-hub fetch into XDG

src/mcp/
  mod.rs                              # pub surface
  envelope.rs                         # FetchResponse, CountResponse, RoverError
  error.rs                            # McpError
  handler.rs                          # RoverHandler { db, config, client }
  server.rs                           # serve_stdio(), startup/heartbeat/shutdown
  tools/
    mod.rs
    fetch.rs                          # #[tool] fn fetch + FetchArgs
    count_tokens.rs                   # #[tool] fn count_tokens + CountTokensArgs

src/storage/
  servers.rs                          # upsert_self, heartbeat, reap_stale, delete_self
  migrations/
    002_servers.sql

src/cli/
  mcp.rs                              # rover mcp body

tests/
  mcp_smoke.rs                        # end-to-end via rmcp client + child process
  tokenizer_integration.rs            # real hf-hub download (network-gated)
  servers_lifecycle.rs                # multi-row reap simulation
  fixtures/tokenizer/tiny.json        # 30-line fixture tokenizer for unit tests

# Modified
Cargo.toml                            # +rmcp, tokenizers, hf-hub; +dev rmcp[client]
src/lib.rs                            # +pub mod mcp; +pub mod tokenizer;
src/main.rs                           # wire Mcp subcommand
src/cli/mod.rs                        # +pub mod mcp;
src/cli/fetch.rs                      # use new frontmatter signature; choose tokenizer
src/config.rs                         # [tokenizer] + [mcp] sections
src/error.rs                          # +Mcp, +Tokenizer variants
src/extractor/frontmatter.rs          # drop estimate_tokens; accept tokens + tokenizer_name
src/storage/mod.rs                    # embed 002_servers.sql; re-export servers
```

Inline unit tests live in `#[cfg(test)] mod tests` at the bottom of each `*.rs`.

---

## Task 1: Dependencies + module scaffolds

**Files:**
- Modify: `Cargo.toml`
- Create: `src/tokenizer/mod.rs` (stub)
- Create: `src/mcp/mod.rs` (stub)
- Modify: `src/lib.rs`

This task lays the skeleton: new module declarations, dep entries, no real logic. Lets the rest of the plan compile incrementally.

- [ ] **Step 1: Add main deps to `Cargo.toml`**

In the `[dependencies]` block, append (after `humantime-serde`):

```toml
rmcp = { version = "1", features = ["server", "macros", "transport-io"] }
tokenizers = { version = "0.21", default-features = false, features = ["onig"] }
hf-hub = { version = "0.4", default-features = false, features = ["ureq", "rustls-tls"] }
```

Notes:
- `rmcp 1.7` pulls in `schemars` transitively through `server`+`macros`; we don't declare schemars directly.
- `tokenizers` with the `onig` feature gives full regex support for the BPE pre-tokenizers used by every family we ship.
- `hf-hub`'s `ureq` + `rustls-tls` give a sync downloader with no native-tls dependency; we run it under `spawn_blocking`.

- [ ] **Step 2: Add dev deps to `Cargo.toml`**

In the `[dev-dependencies]` block, append:

```toml
rmcp = { version = "1", features = ["client", "transport-child-process"] }
```

(Cargo merges this with the main `rmcp` entry — both feature sets are active for tests.)

- [ ] **Step 3: Create `src/tokenizer/mod.rs` (stub)**

```rust
//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. Tasks 2-4 fill this module in.

pub mod error;
pub mod registry;

pub use error::TokenizerError;
pub use registry::Tokenizer;
```

- [ ] **Step 4: Create `src/mcp/mod.rs` (stub)**

```rust
//! MCP server mode (rover mcp).
//!
//! Tasks 8-11 fill this module in. The shape is:
//!   - envelope.rs: wire types returned to clients
//!   - error.rs:    internal McpError
//!   - handler.rs:  RoverHandler { db, config, client }
//!   - tools/:      #[tool] handlers
//!   - server.rs:   serve_stdio + lifecycle
```

- [ ] **Step 5: Wire modules into `src/lib.rs`**

Replace the existing `pub mod` list (alphabetical position) with:

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
pub mod mcp;
pub mod storage;
pub mod telemetry;
pub mod tokenizer;
```

- [ ] **Step 6: Build to verify deps resolve**

Run:

```bash
cargo build
```

Expected: compiles cleanly. The first build may take a few minutes (rmcp + tokenizers + hf-hub are sizeable). Warnings are denied by `[lints.rust]` in `Cargo.toml`; any warning fails the build, so new modules must remain warning-free.

- [ ] **Step 7: Create the empty error/registry stubs so the re-exports compile**

Create `src/tokenizer/error.rs`:

```rust
//! Tokenizer module errors. Real variants land in Task 2.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("tokenizer module not yet initialised")]
    Stub,
}
```

Create `src/tokenizer/registry.rs`:

```rust
//! Tokenizer enum + repo-id table. Real implementation lands in Task 2.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tokenizer {
    Cl100k,
    O200k,
    Claude,
    Llama3,
    Qwen3,
}
```

- [ ] **Step 8: Build again to confirm stubs compile**

```bash
cargo build
```

Expected: compiles cleanly.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/tokenizer/ src/mcp/
git commit -m "feat(m3): scaffold tokenizer + mcp modules with rmcp/tokenizers/hf-hub deps"
```

---

## Task 2: Tokenizer registry (enum, FromStr, repo-id table)

**Files:**
- Modify: `src/tokenizer/registry.rs`
- Modify: `src/tokenizer/error.rs`

Replace stubs with the real `Tokenizer` enum, FromStr/Display, and the HF repo-id table.

- [ ] **Step 1: Write the failing tests in `src/tokenizer/registry.rs`**

Append to the file (over the existing stub):

```rust
//! Tokenizer enum + repo-id table.

use std::fmt;
use std::str::FromStr;

use super::TokenizerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tokenizer {
    Cl100k,
    O200k,
    Claude,
    Llama3,
    Qwen3,
}

impl Tokenizer {
    /// Canonical short name used in TOML config and the MCP `tokenizer` arg.
    pub fn as_str(self) -> &'static str {
        match self {
            Tokenizer::Cl100k => "cl100k",
            Tokenizer::O200k => "o200k",
            Tokenizer::Claude => "claude",
            Tokenizer::Llama3 => "llama3",
            Tokenizer::Qwen3 => "qwen3",
        }
    }

    /// HuggingFace `(repo_id, filename)` pair used by the downloader.
    pub fn hf_source(self) -> (&'static str, &'static str) {
        match self {
            Tokenizer::Cl100k => ("Xenova/gpt-4", "tokenizer.json"),
            Tokenizer::O200k => ("Xenova/gpt-4o", "tokenizer.json"),
            Tokenizer::Claude => ("Xenova/claude-tokenizer", "tokenizer.json"),
            Tokenizer::Llama3 => ("meta-llama/Meta-Llama-3-8B", "tokenizer.json"),
            Tokenizer::Qwen3 => ("Qwen/Qwen2.5-0.5B", "tokenizer.json"),
        }
    }

    /// All known variants, in declaration order. Used by integration tests.
    pub const ALL: [Tokenizer; 5] = [
        Tokenizer::Cl100k,
        Tokenizer::O200k,
        Tokenizer::Claude,
        Tokenizer::Llama3,
        Tokenizer::Qwen3,
    ];
}

impl fmt::Display for Tokenizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tokenizer {
    type Err = TokenizerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cl100k" => Ok(Tokenizer::Cl100k),
            "o200k" => Ok(Tokenizer::O200k),
            "claude" => Ok(Tokenizer::Claude),
            "llama3" => Ok(Tokenizer::Llama3),
            "qwen3" => Ok(Tokenizer::Qwen3),
            other => Err(TokenizerError::UnknownFamily(other.to_string())),
        }
    }
}

impl serde::Serialize for Tokenizer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Tokenizer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_known_families() {
        for t in Tokenizer::ALL {
            let parsed: Tokenizer = t.as_str().parse().unwrap();
            assert_eq!(parsed, t);
        }
    }

    #[test]
    fn unknown_family_errors() {
        let result: Result<Tokenizer, _> = "gpt-5".parse();
        assert!(matches!(result, Err(TokenizerError::UnknownFamily(s)) if s == "gpt-5"));
    }

    #[test]
    fn hf_source_is_non_empty() {
        for t in Tokenizer::ALL {
            let (repo, file) = t.hf_source();
            assert!(!repo.is_empty(), "repo for {t} is empty");
            assert!(!file.is_empty(), "file for {t} is empty");
            assert_eq!(file, "tokenizer.json");
        }
    }

    #[test]
    fn serde_round_trip_via_string() {
        let json = serde_json::to_string(&Tokenizer::O200k).unwrap();
        assert_eq!(json, "\"o200k\"");
        let parsed: Tokenizer = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(parsed, Tokenizer::Claude);
    }
}
```

- [ ] **Step 2: Replace `src/tokenizer/error.rs` with the real error enum**

```rust
//! Tokenizer-module errors.

use thiserror::Error;

use crate::tokenizer::registry::Tokenizer;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("unknown tokenizer family: {0}")]
    UnknownFamily(String),

    #[error("could not download tokenizer for {family}: {source}")]
    Download {
        family: Tokenizer,
        #[source]
        source: hf_hub::api::sync::ApiError,
    },

    #[error("tokenizer file for {family} is corrupt: {source}")]
    Parse {
        family: Tokenizer,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("io error reading tokenizer at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("tokenizer {0} is not loaded; call ensure_loaded() first")]
    NotLoaded(Tokenizer),
}
```

The `Parse` variant uses a boxed error because `tokenizers::Error` is a newtype around a boxed trait object — the boxing chain happens naturally at the call site in Task 3. `serde_json` is already a transitive dep via `rmcp`/`schemars`; if it isn't pulled implicitly into this test (`use serde_json` above), add it under `[dev-dependencies]` as `serde_json = "1"` here.

- [ ] **Step 3: Add `serde_json` to dev-deps if needed**

If `cargo test` complains about `serde_json` being unresolved, add to `[dev-dependencies]`:

```toml
serde_json = "1"
```

(It's needed by the registry test plus integration tests below.)

- [ ] **Step 4: Run the tests**

```bash
cargo test --lib tokenizer::registry
```

Expected: all four tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/tokenizer/
git commit -m "feat(tokenizer): registry enum with FromStr, Display, serde, hf source table"
```

---

## Task 3: HfTokenizer wrapper + fixture-based tests

**Files:**
- Create: `src/tokenizer/hf.rs`
- Modify: `src/tokenizer/mod.rs` (`pub mod hf;`)
- Create: `tests/fixtures/tokenizer/tiny.json`

Wraps `tokenizers::Tokenizer` behind a tiny `HfTokenizer` newtype that exposes `from_path(&Path) -> Result<Self, TokenizerError>` and `count(&self, &str) -> usize`. No registry/loading logic yet; just the type that holds a loaded tokenizer.

- [ ] **Step 1: Vendor the fixture tokenizer**

The unit test needs a minimal `tokenizer.json` that parses and produces deterministic counts without a network round trip. Use the `tokenizers` crate's smallest known-good test artifact: a 4-token byte-level BPE.

Create `tests/fixtures/tokenizer/tiny.json`:

```json
{
  "version": "1.0",
  "truncation": null,
  "padding": null,
  "added_tokens": [],
  "normalizer": null,
  "pre_tokenizer": { "type": "ByteLevel", "add_prefix_space": false, "trim_offsets": true },
  "post_processor": null,
  "decoder": { "type": "ByteLevel", "add_prefix_space": false, "trim_offsets": true },
  "model": {
    "type": "BPE",
    "dropout": null,
    "unk_token": null,
    "continuing_subword_prefix": null,
    "end_of_word_suffix": null,
    "fuse_unk": false,
    "byte_fallback": false,
    "vocab": { "a": 0, "b": 1, "c": 2, "ab": 3 },
    "merges": ["a b"]
  }
}
```

This is a four-vocab byte-level BPE that merges `a b` into the token `ab`. Tokenizing `"abab"` should yield 2 tokens (`ab`, `ab`); tokenizing `"abc"` should yield 2 tokens (`ab`, `c`).

- [ ] **Step 2: Write the failing test in `src/tokenizer/hf.rs`**

```rust
//! Thin wrapper around `tokenizers::Tokenizer`.

use std::path::Path;

use tokenizers::Tokenizer as Inner;

use crate::tokenizer::{Tokenizer, TokenizerError};

#[derive(Debug)]
pub struct HfTokenizer {
    inner: Inner,
}

impl HfTokenizer {
    /// Parse a `tokenizer.json` file from disk.
    pub fn from_path(path: &Path, family: Tokenizer) -> Result<Self, TokenizerError> {
        let inner = Inner::from_file(path).map_err(|e| TokenizerError::Parse {
            family,
            source: e,
        })?;
        Ok(Self { inner })
    }

    /// Count tokens in `text`. Special tokens are not added.
    pub fn count(&self, text: &str) -> Result<usize, TokenizerError> {
        let encoded = self
            .inner
            .encode(text, false)
            .map_err(|e| TokenizerError::Parse {
                family: Tokenizer::Cl100k, // placeholder; encode errors are vanishingly rare
                source: e,
            })?;
        Ok(encoded.get_ids().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/tokenizer/tiny.json");
        p
    }

    #[test]
    fn parses_fixture_tokenizer() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        // Just confirm the type round-trips through parse.
        let _ = tk;
    }

    #[test]
    fn counts_merged_pair_as_one_token() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        assert_eq!(tk.count("abab").unwrap(), 2); // ab, ab
    }

    #[test]
    fn counts_partial_merge_correctly() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        assert_eq!(tk.count("abc").unwrap(), 2); // ab, c
    }

    #[test]
    fn parse_failure_surfaces_family() {
        // Point at a path that exists but isn't a tokenizer.json.
        let bad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
        let err = HfTokenizer::from_path(&bad, Tokenizer::Llama3).unwrap_err();
        match err {
            TokenizerError::Parse { family, .. } => assert_eq!(family, Tokenizer::Llama3),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Wire `hf` into `src/tokenizer/mod.rs`**

```rust
//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. Tasks 4 fills in the load+download
//! orchestration; this module currently exposes the registry, error type, and
//! parser-wrapper.

pub mod error;
pub mod hf;
pub mod registry;

pub use error::TokenizerError;
pub use registry::Tokenizer;
```

- [ ] **Step 4: Run the tests, expect them to pass**

```bash
cargo test --lib tokenizer::hf
```

Expected: all four tests pass. If `tokenizers::Tokenizer::from_file` rejects the fixture (because the JSON shape changed in newer `tokenizers` versions), pin `tokenizers = "0.21.0"` exactly and re-run.

- [ ] **Step 5: Commit**

```bash
git add src/tokenizer/ tests/fixtures/
git commit -m "feat(tokenizer): HfTokenizer wrapper with fixture-based parse + count tests"
```

---

## Task 4: Tokenizer download + public ensure_loaded/count API

**Files:**
- Create: `src/tokenizer/download.rs`
- Modify: `src/tokenizer/mod.rs` (add `download`, `ensure_loaded`, `count`)

Wires the `hf-hub` sync downloader behind `tokio::task::spawn_blocking`, then exposes the process-wide `OnceCell` cache + public `ensure_loaded(t).await` + sync `count(text, t)`.

- [ ] **Step 1: Write `src/tokenizer/download.rs`**

```rust
//! HuggingFace tokenizer file downloader.
//!
//! Uses `hf-hub`'s sync API behind `spawn_blocking`. The on-disk layout is
//! `$XDG_DATA_HOME/rover/tokenizers/<family>/tokenizer.json`. If the file is
//! already present, the function is a no-op; otherwise it pulls from HF and
//! copies into place.

use std::fs;
use std::path::{Path, PathBuf};

use hf_hub::api::sync::{Api, ApiBuilder};

use crate::tokenizer::{Tokenizer, TokenizerError};

/// Ensure the tokenizer.json for `family` exists under `root`, downloading
/// from HuggingFace if missing. Blocks; call inside `spawn_blocking`.
pub fn ensure_on_disk(root: &Path, family: Tokenizer) -> Result<PathBuf, TokenizerError> {
    let dest_dir = root.join(family.as_str());
    let dest_file = dest_dir.join("tokenizer.json");
    if dest_file.exists() {
        return Ok(dest_file);
    }

    fs::create_dir_all(&dest_dir).map_err(|source| TokenizerError::Io {
        path: dest_dir.display().to_string(),
        source,
    })?;

    let (repo_id, filename) = family.hf_source();
    tracing::info!(
        target: "rover::tokenizer",
        family = %family,
        repo = repo_id,
        "downloading tokenizer from HuggingFace"
    );

    let api: Api = ApiBuilder::new()
        .with_progress(false)
        .build()
        .map_err(|e| TokenizerError::Download { family, source: e })?;
    let staged: PathBuf = api
        .model(repo_id.to_string())
        .get(filename)
        .map_err(|e| TokenizerError::Download { family, source: e })?;

    // hf-hub places the file in its own cache. Copy (not symlink — Windows
    // would need elevated perms) into XDG so removal of the hf-hub cache
    // doesn't leave us with a dangling tokenizer.
    fs::copy(&staged, &dest_file).map_err(|source| TokenizerError::Io {
        path: dest_file.display().to_string(),
        source,
    })?;

    let size = fs::metadata(&dest_file)
        .map(|m| m.len())
        .unwrap_or(0);
    tracing::info!(
        target: "rover::tokenizer",
        family = %family,
        bytes = size,
        path = %dest_file.display(),
        "downloaded tokenizer"
    );

    Ok(dest_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_present_file_is_returned_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("cl100k");
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("tokenizer.json");
        fs::write(&f, "{}").unwrap();

        let result = ensure_on_disk(root, Tokenizer::Cl100k).unwrap();
        assert_eq!(result, f);
    }
}
```

- [ ] **Step 2: Write the public API in `src/tokenizer/mod.rs`**

Replace the body of `src/tokenizer/mod.rs` with:

```rust
//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. The public surface is two
//! functions:
//!
//!   - [`ensure_loaded`] is async; it downloads (if needed) and parses the
//!     tokenizer into a process-wide cache.
//!   - [`count`] is synchronous; it returns a token count from the cached
//!     tokenizer. Returns [`TokenizerError::NotLoaded`] if `ensure_loaded`
//!     hasn't been called for the family.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

pub mod download;
pub mod error;
pub mod hf;
pub mod registry;

pub use error::TokenizerError;
pub use hf::HfTokenizer;
pub use registry::Tokenizer;

/// Process-wide registry. Initialised on first access.
fn registry() -> &'static RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>> {
    static REG: OnceLock<RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>>> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Compute the XDG tokenizer root (`$XDG_DATA_HOME/rover/tokenizers` or the
/// platform-appropriate fallback). The `ROVER_DATA_DIR` env var, when set,
/// overrides the parent. Mirrors `cli::fetch::data_dir`.
pub fn xdg_root() -> Result<PathBuf, TokenizerError> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        let p = PathBuf::from(env_dir).join("tokenizers");
        return Ok(p);
    }
    let base = dirs::data_local_dir().ok_or_else(|| TokenizerError::Io {
        path: "<data_local_dir>".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no data dir"),
    })?;
    Ok(base.join("rover").join("tokenizers"))
}

/// Download (if needed) and parse the tokenizer for `family` into the
/// process-wide cache. Subsequent calls for the same family are no-ops.
pub async fn ensure_loaded(family: Tokenizer) -> Result<(), TokenizerError> {
    if registry().read().expect("tokenizer registry rwlock poisoned").contains_key(&family) {
        return Ok(());
    }

    let root = xdg_root()?;
    let path: PathBuf = tokio::task::spawn_blocking(move || download::ensure_on_disk(&root, family))
        .await
        .map_err(|e| TokenizerError::Io {
            path: "<spawn_blocking>".to_string(),
            source: std::io::Error::other(format!("spawn_blocking join failed: {e}")),
        })??;

    let parsed = tokio::task::spawn_blocking(move || HfTokenizer::from_path(&path, family))
        .await
        .map_err(|e| TokenizerError::Io {
            path: "<spawn_blocking>".to_string(),
            source: std::io::Error::other(format!("spawn_blocking join failed: {e}")),
        })??;

    registry()
        .write()
        .expect("tokenizer registry rwlock poisoned")
        .insert(family, Arc::new(parsed));
    Ok(())
}

/// Synchronously count tokens in `text` using the cached tokenizer for
/// `family`. Returns [`TokenizerError::NotLoaded`] if [`ensure_loaded`] has
/// not been called.
pub fn count(text: &str, family: Tokenizer) -> Result<usize, TokenizerError> {
    let map = registry().read().expect("tokenizer registry rwlock poisoned");
    let tk = map.get(&family).ok_or(TokenizerError::NotLoaded(family))?;
    tk.count(text)
}

/// Test-only: clear the global registry. Used by unit tests to keep state
/// independent.
#[cfg(test)]
pub(crate) fn _clear_registry_for_tests() {
    registry()
        .write()
        .expect("tokenizer registry rwlock poisoned")
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/tokenizer/tiny.json");
        p
    }

    #[test]
    fn count_without_load_errors() {
        _clear_registry_for_tests();
        let err = count("abab", Tokenizer::Llama3).unwrap_err();
        assert!(matches!(err, TokenizerError::NotLoaded(Tokenizer::Llama3)));
    }

    #[tokio::test]
    async fn manual_insert_then_count_works() {
        _clear_registry_for_tests();
        let tk = HfTokenizer::from_path(&fixture(), Tokenizer::Cl100k).unwrap();
        registry().write().unwrap().insert(Tokenizer::Cl100k, Arc::new(tk));
        assert_eq!(count("abab", Tokenizer::Cl100k).unwrap(), 2);
    }
}
```

These unit tests deliberately avoid `ensure_loaded` so they don't hit the network. The real `ensure_loaded` path is exercised in `tests/tokenizer_integration.rs` in Task 13.

- [ ] **Step 3: Wire `Tokenizer` variant into `src/error.rs`**

Replace `src/error.rs` with:

```rust
//! Crate-wide error type.
//!
//! Per design supplement §4.4: per-module error enums via `thiserror`,
//! `anyhow` only at the binary boundary.

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

    #[error("tokenizer error: {0}")]
    Tokenizer(#[from] crate::tokenizer::TokenizerError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: Build and run all tokenizer tests**

```bash
cargo build
cargo test --lib tokenizer
```

Expected: build clean, all tokenizer tests pass (registry, hf, download, mod).

- [ ] **Step 5: Commit**

```bash
git add src/tokenizer/ src/error.rs
git commit -m "feat(tokenizer): public ensure_loaded/count over OnceCell + hf-hub downloader"
```

---

## Task 5: Frontmatter refactor — drop estimate_tokens, accept precomputed count

**Files:**
- Modify: `src/extractor/frontmatter.rs`
- Modify: `src/cli/fetch.rs`

Replace the M1 chars/4 heuristic with a writer that takes a precomputed `tokens: usize` and a `tokenizer_name: &str`, and have the CLI feed those in via the real tokenizer.

- [ ] **Step 1: Update tests in `src/extractor/frontmatter.rs`**

Replace the entire `#[cfg(test)] mod tests { … }` block at the bottom of the file with the version below, and update the `PageMeta` struct + `render` function above it.

Replace lines 19–91 (`pub struct PageMeta` through `pub fn estimate_tokens`) with:

```rust
/// Inputs for the M1 frontmatter envelope.
pub struct PageMeta<'a> {
    pub url: &'a Url,
    pub canonical_url: &'a Url,
    pub title: Option<&'a str>,
    pub fetched_at: Timestamp,
    pub body: &'a str,
    /// Precomputed token count for `body`, in units of `tokenizer_name`.
    pub tokens: usize,
    /// Short tokenizer family name (e.g. `"o200k"`). Surfaced in the
    /// `tokenizer` frontmatter field so consumers know how `tokens` was
    /// measured.
    pub tokenizer_name: &'a str,
}

/// Render `meta` as a frontmatter-envelope string followed by `body`.
pub fn render(meta: &PageMeta<'_>) -> String {
    let mut buf = String::with_capacity(meta.body.len() + 256);
    buf.push_str("---\n");

    write_field(&mut buf, "url", meta.url.as_str());
    if meta.canonical_url != meta.url {
        write_field(&mut buf, "canonical_url", meta.canonical_url.as_str());
    }
    if let Some(t) = meta.title {
        write_field(&mut buf, "title", t);
    }
    write_field(&mut buf, "fetched_at", &meta.fetched_at.to_string());

    let content_hash = sha256_hex(meta.body.as_bytes());
    let hash_field = format!("sha256:{content_hash}");
    write_field(&mut buf, "content_hash", &hash_field);

    buf.push_str(&format!("estimated_tokens: {}\n", meta.tokens));
    write_field(&mut buf, "tokenizer", meta.tokenizer_name);

    buf.push_str("---\n\n");
    buf.push_str(meta.body);
    if !meta.body.ends_with('\n') {
        buf.push('\n');
    }
    buf
}
```

Then replace the `#[cfg(test)]` block with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;

    fn ts() -> Timestamp {
        "2026-05-07T12:34:56Z".parse().unwrap()
    }
    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn meta<'a>(url: &'a Url, body: &'a str) -> PageMeta<'a> {
        PageMeta {
            url,
            canonical_url: url,
            title: Some("Sample"),
            fetched_at: ts(),
            body,
            tokens: 7,
            tokenizer_name: "o200k",
        }
    }

    #[test]
    fn emits_required_fields() {
        let url = u("https://example.com/page");
        let body = "# Title\n\nBody.\n";
        let out = render(&meta(&url, body));

        assert!(out.starts_with("---\n"));
        assert!(out.contains(r#"url: "https://example.com/page""#));
        assert!(out.contains(r#"title: "Sample""#));
        assert!(out.contains(r#"fetched_at: "2026-05-07T12:34:56Z""#));
        assert!(out.contains("content_hash: \"sha256:"));
        assert!(out.contains("estimated_tokens: 7"));
        assert!(out.contains(r#"tokenizer: "o200k""#));
        assert!(out.ends_with(body));
    }

    #[test]
    fn omits_canonical_when_same_as_url() {
        let url = u("https://example.com/page");
        let out = render(&PageMeta { title: None, ..meta(&url, "x") });
        assert!(!out.contains("canonical_url"));
    }

    #[test]
    fn includes_canonical_when_different() {
        let url = u("https://example.com/page?utm=1");
        let canon = u("https://example.com/page");
        let out = render(&PageMeta {
            canonical_url: &canon,
            title: None,
            ..meta(&url, "x")
        });
        assert!(out.contains(r#"canonical_url: "https://example.com/page""#));
    }

    #[test]
    fn quotes_in_title_are_escaped() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            title: Some(r#"He said "hi""#),
            ..meta(&url, "x")
        });
        assert!(out.contains(r#"title: "He said \"hi\"""#));
    }

    #[test]
    fn content_hash_is_deterministic() {
        let url = u("https://example.com/p");
        let body = "stable body";
        let a = render(&meta(&url, body));
        let b = render(&meta(&url, body));
        assert_eq!(a, b);
    }

    #[test]
    fn token_count_is_passed_through_verbatim() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta { tokens: 1234, ..meta(&url, "hello") });
        assert!(out.contains("estimated_tokens: 1234"));
    }

    #[test]
    fn body_terminates_with_newline() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            title: None,
            ..meta(&url, "no trailing newline")
        });
        assert!(out.ends_with('\n'));
    }
}
```

Note the `estimate_tokens` test is gone — the function is removed.

- [ ] **Step 2: Run the failing tests**

```bash
cargo test --lib extractor::frontmatter
```

Expected: most tests fail to compile (struct literal pattern incompatible with old struct) until step 3 lands. If you ran step 1 already, expect the build to fail at `src/cli/fetch.rs` referring to the old shape.

- [ ] **Step 3: Update `src/cli/fetch.rs` to pass token count + name**

Locate the block (around lines 75–86 of the current file) that builds `PageMeta` and renders it. Replace with:

```rust
    let canonical =
        Url::parse(&result.page.canonical_url).context("parsing canonical URL from cache row")?;

    // Choose the tokenizer for frontmatter `estimated_tokens` from config.
    let family = cfg.tokenizer.default;
    rover::tokenizer::ensure_loaded(family)
        .await
        .context("loading default tokenizer")?;
    let tokens = rover::tokenizer::count(&result.page.extracted_md, family)
        .context("counting tokens for frontmatter")?;

    let meta = PageMeta {
        url: &url,
        canonical_url: &canonical,
        title: result.page.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &result.page.extracted_md,
        tokens,
        tokenizer_name: family.as_str(),
    };
```

(The `cfg.tokenizer.default` field lands in Task 6. Until that task is merged this step's code won't compile — that's expected; the build only needs to be green at the end of each task.)

Add `use crate::tokenizer;` near the existing `use crate::...` block at the top.

Replace `rover::tokenizer::ensure_loaded` and `rover::tokenizer::count` with `crate::tokenizer::ensure_loaded` and `crate::tokenizer::count` if the file already imports from `crate::`. (Both work; pick whichever matches the existing imports.)

- [ ] **Step 4: Defer the build check until after Task 6**

This task and Task 6 land together for the binary to build. The unit tests for `frontmatter` should pass in isolation:

```bash
cargo test --lib extractor::frontmatter
```

Expected: 7 tests pass.

Full crate build will fail (`cfg.tokenizer` unknown). That's OK; Task 6 fixes it.

- [ ] **Step 5: Commit**

```bash
git add src/extractor/frontmatter.rs src/cli/fetch.rs
git commit -m "refactor(frontmatter): drop chars/4 estimate; accept tokens + tokenizer_name"
```

---

## Task 6: Config — `[tokenizer]` and `[mcp]` sections

**Files:**
- Modify: `src/config.rs`

Adds the two config sections required by M3. Validation: tokenizer family must parse via `Tokenizer::from_str`; both `[mcp]` durations must be `> 0`.

- [ ] **Step 1: Write the failing tests at the bottom of `src/config.rs`**

In the `#[cfg(test)] mod tests` block, append:

```rust
    #[test]
    fn default_tokenizer_is_o200k() {
        let cfg = Config::default();
        assert_eq!(cfg.tokenizer.default, crate::tokenizer::Tokenizer::O200k);
    }

    #[test]
    fn default_mcp_intervals() {
        let cfg = Config::default();
        assert_eq!(cfg.mcp.heartbeat_interval, Duration::from_secs(5));
        assert_eq!(cfg.mcp.reap_threshold, Duration::from_secs(60));
    }

    #[test]
    fn load_tokenizer_override() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[tokenizer]
default = "claude"
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.tokenizer.default, crate::tokenizer::Tokenizer::Claude);
    }

    #[test]
    fn load_unknown_tokenizer_errors() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[tokenizer]
default = "gpt-5"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn load_mcp_overrides() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[mcp]
heartbeat_interval = "10s"
reap_threshold = "2m"
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.mcp.heartbeat_interval, Duration::from_secs(10));
        assert_eq!(cfg.mcp.reap_threshold, Duration::from_secs(120));
    }

    #[test]
    fn load_rejects_zero_heartbeat() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[mcp]
heartbeat_interval = "0s"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }
```

- [ ] **Step 2: Run the tests, expect compile failure**

```bash
cargo test --lib config
```

Expected: build fails — `cfg.tokenizer` and `cfg.mcp` don't exist.

- [ ] **Step 3: Add the config sections**

In `src/config.rs`, add to `Config`:

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
}
```

Append the two structs after `CacheConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizerConfig {
    #[serde(default = "default_tokenizer")]
    pub default: crate::tokenizer::Tokenizer,
}

impl Default for TokenizerConfig {
    fn default() -> Self {
        Self { default: default_tokenizer() }
    }
}

fn default_tokenizer() -> crate::tokenizer::Tokenizer {
    crate::tokenizer::Tokenizer::O200k
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default = "default_heartbeat_interval", with = "humantime_serde")]
    pub heartbeat_interval: Duration,

    #[serde(default = "default_reap_threshold", with = "humantime_serde")]
    pub reap_threshold: Duration,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: default_heartbeat_interval(),
            reap_threshold: default_reap_threshold(),
        }
    }
}

fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(5)
}

fn default_reap_threshold() -> Duration {
    Duration::from_secs(60)
}
```

Extend `validate` to reject zero durations:

```rust
fn validate(cfg: &mut Config) -> Result<(), String> {
    if cfg.fetch.timeout_secs == 0 {
        return Err("fetch.timeout_secs must be > 0".to_string());
    }
    if cfg.cache.min_ttl > cfg.cache.default_ttl {
        return Err(format!(
            "cache.min_ttl ({:?}) must be <= cache.default_ttl ({:?})",
            cfg.cache.min_ttl, cfg.cache.default_ttl
        ));
    }
    if cfg.cache.default_ttl > cfg.cache.max_ttl {
        return Err(format!(
            "cache.default_ttl ({:?}) must be <= cache.max_ttl ({:?})",
            cfg.cache.default_ttl, cfg.cache.max_ttl
        ));
    }
    if cfg.mcp.heartbeat_interval.as_secs() == 0
        && cfg.mcp.heartbeat_interval.subsec_nanos() == 0
    {
        return Err("mcp.heartbeat_interval must be > 0".to_string());
    }
    if cfg.mcp.reap_threshold.as_secs() == 0 && cfg.mcp.reap_threshold.subsec_nanos() == 0 {
        return Err("mcp.reap_threshold must be > 0".to_string());
    }
    for d in &mut cfg.cache.override_no_store_domains {
        d.make_ascii_lowercase();
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests + full crate build**

```bash
cargo test --lib config
cargo build
```

Expected: config tests pass; the binary now compiles (Task 5's reference to `cfg.tokenizer.default` resolves).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add [tokenizer] and [mcp] sections with humantime durations"
```

---

## Task 7: `servers` table — migration + async API

**Files:**
- Create: `src/storage/migrations/002_servers.sql`
- Create: `src/storage/servers.rs`
- Modify: `src/storage/mod.rs` (add migration tuple + `pub mod servers;`)

Adds the `servers` table that tracks live `rover mcp` instances, plus a thin async API (`upsert_self`, `heartbeat`, `reap_stale`, `delete_self`).

- [ ] **Step 1: Create `src/storage/migrations/002_servers.sql`**

```sql
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
```

- [ ] **Step 2: Append the migration tuple in `src/storage/mod.rs`**

Find the `MIGRATIONS` const and replace it with:

```rust
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial.sql", include_str!("migrations/001_initial.sql")),
    ("002_servers.sql", include_str!("migrations/002_servers.sql")),
];
```

Then add `pub mod servers;` near the other `pub mod` lines in the same file.

- [ ] **Step 3: Write the failing tests + implementation in `src/storage/servers.rs`**

Create the file with:

```rust
//! `servers` table: tracks live `rover mcp` instances.
//!
//! Each running server inserts its own PID row on startup and refreshes
//! `last_heartbeat` on a tokio interval. Stale rows (`last_heartbeat`
//! older than the configured threshold) are reaped at startup. Clean
//! shutdown deletes the own row.

use std::time::Duration;

use rusqlite::params;

use super::{Db, StorageError};

/// Row shape returned by query helpers + used by tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub pid: i64,
    pub version: String,
    pub started_at: i64,
    pub last_heartbeat: i64,
}

impl Db {
    /// Upsert the current process's row. If a row already exists for `pid`
    /// (from a crashed prior run of this PID), its `started_at` is reset.
    pub async fn upsert_server_self(&self, pid: i64, version: String) -> Result<(), StorageError> {
        let now = now_epoch();
        let version_for_sql = version.clone();
        self.conn
            .call(move |c| {
                c.execute(
                    "INSERT INTO servers(pid, version, started_at, last_heartbeat)
                     VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(pid) DO UPDATE SET
                       version = excluded.version,
                       started_at = excluded.started_at,
                       last_heartbeat = excluded.last_heartbeat",
                    params![pid, version_for_sql, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Refresh `last_heartbeat` for `pid`. If no row exists, this is a no-op
    /// (the next heartbeat tick will pick it up; logs are emitted at the
    /// caller).
    pub async fn heartbeat_server(&self, pid: i64) -> Result<(), StorageError> {
        let now = now_epoch();
        self.conn
            .call(move |c| {
                c.execute(
                    "UPDATE servers SET last_heartbeat = ?1 WHERE pid = ?2",
                    params![now, pid],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Delete rows whose `last_heartbeat` is older than `threshold` ago.
    /// Returns the number of rows removed.
    pub async fn reap_stale_servers(&self, threshold: Duration) -> Result<usize, StorageError> {
        let cutoff = now_epoch() - threshold.as_secs() as i64;
        let removed = self
            .conn
            .call(move |c| {
                let n = c.execute(
                    "DELETE FROM servers WHERE last_heartbeat < ?1",
                    params![cutoff],
                )?;
                Ok::<_, rusqlite::Error>(n)
            })
            .await?;
        Ok(removed)
    }

    /// Remove the row for `pid` (idempotent).
    pub async fn delete_server_self(&self, pid: i64) -> Result<(), StorageError> {
        self.conn
            .call(move |c| {
                c.execute("DELETE FROM servers WHERE pid = ?1", params![pid])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Read all rows (testing aid).
    #[cfg(test)]
    pub async fn list_servers(&self) -> Result<Vec<ServerRow>, StorageError> {
        let rows = self
            .conn
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT pid, version, started_at, last_heartbeat FROM servers ORDER BY pid",
                )?;
                let mut out = Vec::new();
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    out.push(ServerRow {
                        pid: r.get(0)?,
                        version: r.get(1)?,
                        started_at: r.get(2)?,
                        last_heartbeat: r.get(3)?,
                    });
                }
                Ok::<_, rusqlite::Error>(out)
            })
            .await?;
        Ok(rows)
    }
}

fn now_epoch() -> i64 {
    jiff::Timestamp::now().as_second()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_db() -> Db {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        // Keep tempdir alive by leaking it; this is a unit test, the OS will
        // reclaim on exit.
        std::mem::forget(tmp);
        db
    }

    #[tokio::test]
    async fn upsert_is_idempotent_and_updates_version() {
        let db = fresh_db().await;
        db.upsert_server_self(123, "0.1.0".into()).await.unwrap();
        db.upsert_server_self(123, "0.1.1".into()).await.unwrap();
        let rows = db.list_servers().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 123);
        assert_eq!(rows[0].version, "0.1.1");
    }

    #[tokio::test]
    async fn heartbeat_updates_last_heartbeat() {
        let db = fresh_db().await;
        db.upsert_server_self(7, "v".into()).await.unwrap();
        let initial = db.list_servers().await.unwrap()[0].last_heartbeat;
        // Push the timestamp backwards to simulate elapsed time.
        db.conn
            .call(|c| {
                c.execute("UPDATE servers SET last_heartbeat = 100 WHERE pid = 7", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        db.heartbeat_server(7).await.unwrap();
        let updated = db.list_servers().await.unwrap()[0].last_heartbeat;
        assert!(updated > 100);
        assert!(updated >= initial);
    }

    #[tokio::test]
    async fn reap_stale_removes_old_rows_only() {
        let db = fresh_db().await;
        db.upsert_server_self(1, "v".into()).await.unwrap();
        db.upsert_server_self(2, "v".into()).await.unwrap();
        db.upsert_server_self(3, "v".into()).await.unwrap();
        // Mark PID 1 and 2 as ancient.
        db.conn
            .call(|c| {
                c.execute("UPDATE servers SET last_heartbeat = 0 WHERE pid IN (1,2)", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        let removed = db.reap_stale_servers(Duration::from_secs(60)).await.unwrap();
        assert_eq!(removed, 2);
        let rows = db.list_servers().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 3);
    }

    #[tokio::test]
    async fn delete_self_is_idempotent() {
        let db = fresh_db().await;
        db.upsert_server_self(42, "v".into()).await.unwrap();
        db.delete_server_self(42).await.unwrap();
        db.delete_server_self(42).await.unwrap();
        assert!(db.list_servers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn heartbeat_on_missing_pid_is_noop() {
        let db = fresh_db().await;
        db.heartbeat_server(999).await.unwrap();
        assert!(db.list_servers().await.unwrap().is_empty());
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --lib storage::servers
cargo test --lib storage   # ensure existing storage tests still pass
```

Expected: all five new tests pass; existing storage tests (open_creates_db, broken_migration, pages CRUD, etc.) still pass — migration `002_servers.sql` runs cleanly alongside `001_initial.sql`.

- [ ] **Step 5: Commit**

```bash
git add src/storage/
git commit -m "feat(storage): servers table + async upsert/heartbeat/reap/delete API"
```

---

## Task 8: MCP envelope + internal error types

**Files:**
- Create: `src/mcp/envelope.rs`
- Create: `src/mcp/error.rs`
- Modify: `src/mcp/mod.rs`
- Modify: `src/error.rs` (add `Mcp` variant)

Defines the wire types returned to MCP clients (`FetchResponse`, `CountResponse`, `RoverError`) and the internal `McpError` enum with translation to `RoverError` codes.

- [ ] **Step 1: Create `src/mcp/envelope.rs`**

```rust
//! Wire-side envelope types returned to MCP clients.
//!
//! These are the JSON shapes Claude Code (or any other MCP client) sees.
//! The `code` strings on [`RoverError`] are stable from M3 onward and will
//! be documented in `docs/mcp-tools.md` (M8).

use serde::{Deserialize, Serialize};

/// Status of a fetch response relative to the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStatus {
    Hit,
    Miss,
    Stale,
    Revalidated304,
}

impl From<crate::fetcher::cached::CacheStatus> for CacheStatus {
    fn from(v: crate::fetcher::cached::CacheStatus) -> Self {
        use crate::fetcher::cached::CacheStatus as C;
        match v {
            C::Hit => CacheStatus::Hit,
            C::Miss => CacheStatus::Miss,
            C::Stale => CacheStatus::Stale,
            C::Revalidated304 => CacheStatus::Revalidated304,
        }
    }
}

/// Where the token count came from on a `count_tokens` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountSource {
    Text,
    Url,
}

/// Successful `fetch` response (full content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    pub markdown: String,
    pub frontmatter: String,
    pub cache_status: CacheStatus,
}

/// `count_tokens` or `fetch{count_only:true}` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountResponse {
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

/// Stable error envelope returned over MCP. `code` is from the fixed set
/// documented in the M3 design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoverError {
    pub code: &'static str,
    pub message: String,
}

impl RoverError {
    pub const MAX_TOKENS_EXCEEDED: &'static str = "max_tokens_exceeded";
    pub const INVALID_ARGS: &'static str = "invalid_args";
    pub const INVALID_URL: &'static str = "invalid_url";
    pub const SSRF_DENIED: &'static str = "ssrf_denied";
    pub const FETCH_FAILED: &'static str = "fetch_failed";
    pub const EXTRACT_FAILED: &'static str = "extract_failed";
    pub const STORAGE_ERROR: &'static str = "storage_error";
    pub const TOKENIZER_UNAVAILABLE: &'static str = "tokenizer_unavailable";

    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_response_serialises_snake_case_cache_status() {
        let v = FetchResponse {
            markdown: "x".into(),
            frontmatter: "f".into(),
            cache_status: CacheStatus::Revalidated304,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"cache_status\":\"revalidated304\""), "got: {s}");
    }

    #[test]
    fn count_response_omits_optional_fields() {
        let v = CountResponse {
            tokens: 7,
            tokenizer: "o200k".into(),
            source: CountSource::Text,
            url: None,
            content_hash: None,
            fetched_at: None,
            cache_status: None,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("url"));
        assert!(!s.contains("content_hash"));
        assert!(!s.contains("cache_status"));
    }

    #[test]
    fn rover_error_codes_are_stable_constants() {
        // Compile-time check that the constants exist and are str slices.
        let codes: &[&'static str] = &[
            RoverError::MAX_TOKENS_EXCEEDED,
            RoverError::INVALID_ARGS,
            RoverError::FETCH_FAILED,
            RoverError::SSRF_DENIED,
            RoverError::EXTRACT_FAILED,
            RoverError::STORAGE_ERROR,
            RoverError::TOKENIZER_UNAVAILABLE,
            RoverError::INVALID_URL,
        ];
        // Codes must be unique.
        for (i, a) in codes.iter().enumerate() {
            for (j, b) in codes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "duplicate code: {a}");
                }
            }
        }
    }
}
```

- [ ] **Step 2: Create `src/mcp/error.rs`**

```rust
//! Internal MCP-layer errors. Translated to `RoverError` before crossing
//! the tool boundary.

use thiserror::Error;

use crate::extractor::ExtractorError;
use crate::fetcher::FetcherError;
use crate::mcp::envelope::RoverError;
use crate::storage::StorageError;
use crate::tokenizer::TokenizerError;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("tokenizer error: {0}")]
    Tokenizer(#[from] TokenizerError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] ExtractorError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("max_tokens exceeded: {actual} > {max}")]
    MaxTokensExceeded { actual: usize, max: usize },
}

impl McpError {
    /// Translate to the stable wire envelope. Logs the full internal chain
    /// at `warn` so operators retain diagnostics on stderr.
    pub fn into_rover_error(self) -> RoverError {
        match &self {
            Self::MaxTokensExceeded { actual, max } => {
                let msg = format!(
                    "extracted content is {actual} tokens; max_tokens={max}. \
                     summarize tool not yet available (M7)"
                );
                RoverError::new(RoverError::MAX_TOKENS_EXCEEDED, msg)
            }
            Self::InvalidArgs(m) => RoverError::new(RoverError::INVALID_ARGS, m.clone()),
            Self::InvalidUrl(m) => RoverError::new(RoverError::INVALID_URL, m.clone()),
            Self::Tokenizer(e) => match e {
                TokenizerError::UnknownFamily(name) => RoverError::new(
                    RoverError::INVALID_ARGS,
                    format!("unknown tokenizer family: {name}"),
                ),
                TokenizerError::Download { family, .. } => RoverError::new(
                    RoverError::TOKENIZER_UNAVAILABLE,
                    format!("could not fetch tokenizer for {family}: {e}"),
                ),
                TokenizerError::Parse { family, .. } => RoverError::new(
                    RoverError::TOKENIZER_UNAVAILABLE,
                    format!("tokenizer file for {family} is corrupt: {e}"),
                ),
                TokenizerError::Io { .. } | TokenizerError::NotLoaded(_) => RoverError::new(
                    RoverError::TOKENIZER_UNAVAILABLE,
                    e.to_string(),
                ),
            },
            Self::Fetcher(e) => {
                use crate::fetcher::FetcherError as F;
                match e {
                    F::Ssrf(_) => RoverError::new(RoverError::SSRF_DENIED, e.to_string()),
                    F::Status { .. } => RoverError::new(RoverError::FETCH_FAILED, e.to_string()),
                    _ => RoverError::new(RoverError::FETCH_FAILED, e.to_string()),
                }
            }
            Self::Extractor(_) => RoverError::new(RoverError::EXTRACT_FAILED, self.to_string()),
            Self::Storage(_) => RoverError::new(RoverError::STORAGE_ERROR, self.to_string()),
        }
    }
}

/// Convenience: log + translate. Use this at the tool boundary.
pub(crate) fn log_and_translate(err: McpError) -> RoverError {
    tracing::warn!(target: "rover::mcp", error = ?err, "tool error");
    err.into_rover_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_tokens_translation_uses_stable_code() {
        let e = McpError::MaxTokensExceeded { actual: 5000, max: 1000 };
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::MAX_TOKENS_EXCEEDED);
        assert!(r.message.contains("5000"));
        assert!(r.message.contains("1000"));
        assert!(r.message.contains("summarize"));
    }

    #[test]
    fn invalid_args_translation() {
        let e = McpError::InvalidArgs("bad".into());
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
        assert_eq!(r.message, "bad");
    }
}
```

Note `FetcherError` variants `Ssrf` and `Status { .. }` are referenced by the translation. If the real names differ (consult `src/fetcher/mod.rs` and `src/fetcher/fetch.rs`), adjust the match arms to compile — the `_ =>` catch-all keeps coverage. The variant set is closed in M2, so the match should be exhaustive after a quick read.

- [ ] **Step 3: Wire envelope+error into `src/mcp/mod.rs`**

Replace the body of `src/mcp/mod.rs` with:

```rust
//! MCP server mode (rover mcp).
//!
//! Architecture: a `RoverHandler` (Task 9) backed by `rmcp`'s `#[tool_router]`
//! macros holds the `Db` + `Config` + `reqwest::Client` shared state. Two
//! tools (`fetch`, `count_tokens`) wrap the M1/M2 pipeline behind typed
//! arg structs. Errors are translated to a stable wire envelope.

pub mod envelope;
pub mod error;

pub use envelope::{CacheStatus, CountResponse, CountSource, FetchResponse, RoverError};
pub use error::McpError;
```

- [ ] **Step 4: Add `Mcp` variant to `src/error.rs`**

```rust
#[derive(Debug, Error)]
pub enum Error {
    // ... existing variants ...

    #[error("mcp error: {0}")]
    Mcp(#[from] crate::mcp::McpError),
}
```

(Insert above the `Io` variant.)

- [ ] **Step 5: Build and run tests**

```bash
cargo build
cargo test --lib mcp
```

Expected: clean build, all envelope + error tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/ src/error.rs
git commit -m "feat(mcp): envelope wire types + internal McpError with stable code translation"
```

---

## Task 9: MCP handler skeleton + `fetch` tool

**Files:**
- Create: `src/mcp/handler.rs`
- Create: `src/mcp/tools/mod.rs`
- Create: `src/mcp/tools/fetch.rs`
- Modify: `src/mcp/mod.rs`

Sets up `RoverHandler` (the type that `#[tool_router]` decorates) and ships the `fetch` tool with its `FetchArgs` struct, schema derivation, no-op debug logs for unimplemented args, and `max_tokens` enforcement.

- [ ] **Step 1: Create `src/mcp/handler.rs`**

```rust
//! Shared MCP server state.

use std::sync::Arc;

use crate::config::Config;
use crate::storage::Db;

/// State shared across all MCP tool invocations.
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
}

impl RoverHandler {
    pub fn new(db: Db, config: Arc<Config>, client: reqwest::Client) -> Self {
        Self { db, config, client }
    }
}
```

- [ ] **Step 2: Create `src/mcp/tools/mod.rs`**

```rust
pub mod count_tokens;
pub mod fetch;
```

(`count_tokens.rs` lands in Task 10. For this task's build to succeed, also create it as a one-line stub now:)

Create `src/mcp/tools/count_tokens.rs`:

```rust
//! count_tokens tool. Real implementation lands in Task 10.
```

- [ ] **Step 3: Write the failing tests + implementation in `src/mcp/tools/fetch.rs`**

```rust
//! MCP `fetch` tool — wraps the M1/M2 pipeline behind a typed arg struct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::frontmatter::{PageMeta, render as render_frontmatter};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::envelope::{CacheStatus, CountResponse, CountSource, FetchResponse};
use crate::mcp::error::McpError;
use crate::mcp::handler::RoverHandler;
use crate::tokenizer::{self, Tokenizer};

/// Wire-side `fetch` tool arguments.
///
/// Live in M3: `url`, `force_refresh`, `count_only`, `tokenizer`, `max_tokens`.
/// Accept-no-op (schema-stable, body-deferred): `headless`, `tables`, `images`,
/// `metadata`, `summarize`. Their values are accepted by the schema and
/// emit one `tracing::debug` line each.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    pub url: String,

    #[serde(default)]
    pub force_refresh: bool,

    #[serde(default)]
    pub count_only: bool,

    #[serde(default)]
    pub tokenizer: Option<Tokenizer>,

    #[serde(default)]
    pub max_tokens: Option<usize>,

    // ---- accept-no-op until later milestones ----
    #[serde(default)]
    pub headless: Option<serde_json::Value>,
    #[serde(default)]
    pub tables: Option<serde_json::Value>,
    #[serde(default)]
    pub images: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub summarize: Option<serde_json::Value>,
}

/// One of the two response shapes the `fetch` tool can produce, depending
/// on `count_only`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FetchOutput {
    Full(FetchResponse),
    Count(CountResponse),
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    /// Task 11 wires this into the router; here it's a plain async method.
    pub async fn fetch_inner(&self, args: FetchArgs) -> Result<FetchOutput, McpError> {
        log_deferred_args(&args);

        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = args.tokenizer.unwrap_or(self.config.tokenizer.default);

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &url,
            &self.config.cache,
            FetchOptions {
                force_refresh: args.force_refresh,
                ssrf_level: SsrfLevel::Strict,
            },
            |body, base| {
                let extracted = extract(body, Some(base))
                    .map_err(|_| crate::fetcher::FetcherError::Decode)?;
                let content_hash =
                    format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                })
            },
        )
        .await?;

        tokenizer::ensure_loaded(family).await?;
        let tokens = tokenizer::count(&result.page.extracted_md, family)?;

        if let Some(max) = args.max_tokens
            && tokens > max
        {
            return Err(McpError::MaxTokensExceeded { actual: tokens, max });
        }

        let cache_status: CacheStatus = result.cache_status.into();

        if args.count_only {
            return Ok(FetchOutput::Count(CountResponse {
                tokens,
                tokenizer: family.as_str().to_string(),
                source: CountSource::Url,
                url: Some(url.as_str().to_string()),
                content_hash: Some(format!("sha256:{}", result.page.content_hash)),
                fetched_at: Some(
                    jiff::Timestamp::from_second(result.page.fetched_at)
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                ),
                cache_status: Some(cache_status),
            }));
        }

        let canonical = Url::parse(&result.page.canonical_url)
            .map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let frontmatter = render_frontmatter(&PageMeta {
            url: &url,
            canonical_url: &canonical,
            title: result.page.title.as_deref(),
            fetched_at: jiff::Timestamp::now(),
            body: &result.page.extracted_md,
            tokens,
            tokenizer_name: family.as_str(),
        });

        Ok(FetchOutput::Full(FetchResponse {
            markdown: result.page.extracted_md,
            frontmatter,
            cache_status,
        }))
    }
}

fn log_deferred_args(args: &FetchArgs) {
    if let Some(v) = &args.headless {
        tracing::debug!(target: "rover::mcp", arg = "headless", value = ?v, "ignored until M9");
    }
    if let Some(v) = &args.tables {
        tracing::debug!(target: "rover::mcp", arg = "tables", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.images {
        tracing::debug!(target: "rover::mcp", arg = "images", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.metadata {
        tracing::debug!(target: "rover::mcp", arg = "metadata", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.summarize {
        tracing::debug!(target: "rover::mcp", arg = "summarize", value = ?v, "ignored until M7");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_args_deserialize_minimal() {
        let v: FetchArgs = serde_json::from_str(r#"{"url":"https://example.com"}"#).unwrap();
        assert_eq!(v.url, "https://example.com");
        assert!(!v.force_refresh);
        assert!(!v.count_only);
        assert!(v.tokenizer.is_none());
        assert!(v.max_tokens.is_none());
    }

    #[test]
    fn fetch_args_accept_deferred_keys_as_no_op() {
        let v: FetchArgs = serde_json::from_str(
            r#"{
                "url":"https://example.com",
                "headless":"auto",
                "tables":{"mode":"Sample"},
                "images":{"mode":"Drop"},
                "metadata":{"preset":"default"},
                "summarize":{"target_tokens":500}
            }"#,
        )
        .unwrap();
        assert!(v.headless.is_some());
        assert!(v.tables.is_some());
        assert!(v.summarize.is_some());
    }

    #[test]
    fn fetch_args_reject_unknown_fields() {
        let r: Result<FetchArgs, _> =
            serde_json::from_str(r#"{"url":"https://example.com","bogus":1}"#);
        assert!(r.is_err());
    }

    #[test]
    fn fetch_args_parse_tokenizer_string() {
        let v: FetchArgs = serde_json::from_str(
            r#"{"url":"https://example.com","tokenizer":"claude"}"#,
        )
        .unwrap();
        assert_eq!(v.tokenizer, Some(Tokenizer::Claude));
    }

    #[test]
    fn fetch_args_schema_contains_all_documented_fields() {
        let schema = schemars::schema_for!(FetchArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for field in [
            "url",
            "force_refresh",
            "count_only",
            "tokenizer",
            "max_tokens",
            "headless",
            "tables",
            "images",
            "metadata",
            "summarize",
        ] {
            assert!(json.contains(field), "schema missing field: {field}");
        }
    }
}
```

Note: the `if let Some(max) = args.max_tokens && tokens > max` syntax requires Rust 1.85 (let-chains stabilised). Cargo.toml already pins `rust-version = "1.85"`. If the toolchain in CI is older, replace with a nested `if let`.

Note 2: `JsonSchema` is from `schemars` (transitively pulled by `rmcp`); add `schemars` directly as a dep if `cargo build` complains it isn't resolvable. If it does, append to `[dependencies]`:

```toml
schemars = "0.9"
```

- [ ] **Step 4: Re-export new types via `src/mcp/mod.rs`**

Append to `src/mcp/mod.rs`:

```rust
pub mod handler;
pub mod tools;

pub use handler::RoverHandler;
```

- [ ] **Step 5: Build + run tests**

```bash
cargo build
cargo test --lib mcp::tools::fetch
```

Expected: clean build, all 5 fetch-tool tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/ Cargo.toml Cargo.lock
git commit -m "feat(mcp): fetch tool with FetchArgs schema, max_tokens enforcement, no-op deferrals"
```

---

## Task 10: MCP `count_tokens` tool

**Files:**
- Modify: `src/mcp/tools/count_tokens.rs`

Wires the `count_tokens` tool body. Two input modes (text or url), exactly-one-of validation, URL mode shares the cached fetch pipeline.

- [ ] **Step 1: Replace `src/mcp/tools/count_tokens.rs`**

```rust
//! MCP `count_tokens` tool.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::envelope::{CacheStatus, CountResponse, CountSource};
use crate::mcp::error::McpError;
use crate::mcp::handler::RoverHandler;
use crate::tokenizer::{self, Tokenizer};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountTokensArgs {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tokenizer: Option<Tokenizer>,
}

impl RoverHandler {
    pub async fn count_tokens_inner(
        &self,
        args: CountTokensArgs,
    ) -> Result<CountResponse, McpError> {
        match (args.text.as_deref(), args.url.as_deref()) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(McpError::InvalidArgs(
                    "count_tokens requires exactly one of text or url".into(),
                ));
            }
            _ => {}
        }

        let family = args.tokenizer.unwrap_or(self.config.tokenizer.default);
        tokenizer::ensure_loaded(family).await?;

        if let Some(text) = args.text {
            let tokens = tokenizer::count(&text, family)?;
            return Ok(CountResponse {
                tokens,
                tokenizer: family.as_str().to_string(),
                source: CountSource::Text,
                url: None,
                content_hash: None,
                fetched_at: None,
                cache_status: None,
            });
        }

        // URL mode.
        let url_str = args.url.expect("validated above");
        let url = Url::parse(&url_str).map_err(|e| McpError::InvalidUrl(e.to_string()))?;

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &url,
            &self.config.cache,
            FetchOptions { force_refresh: false, ssrf_level: SsrfLevel::Strict },
            |body, base| {
                let extracted = extract(body, Some(base))
                    .map_err(|_| crate::fetcher::FetcherError::Decode)?;
                let content_hash =
                    format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                })
            },
        )
        .await?;

        let tokens = tokenizer::count(&result.page.extracted_md, family)?;
        let cache_status: CacheStatus = result.cache_status.into();

        Ok(CountResponse {
            tokens,
            tokenizer: family.as_str().to_string(),
            source: CountSource::Url,
            url: Some(url.as_str().to_string()),
            content_hash: Some(format!("sha256:{}", result.page.content_hash)),
            fetched_at: Some(
                jiff::Timestamp::from_second(result.page.fetched_at)
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
            ),
            cache_status: Some(cache_status),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::envelope::RoverError;

    fn fake_handler() -> RoverHandler {
        // Build a handler whose db/client are unused — the tests below hit
        // the synchronous validation path only.
        let cfg = std::sync::Arc::new(crate::config::Config::default());
        let client = reqwest::Client::new();
        // Open a throwaway in-memory db via tempdir.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async { crate::storage::Db::open(&path).await.unwrap() });
        std::mem::forget(tmp);
        RoverHandler::new(db, cfg, client)
    }

    #[tokio::test]
    async fn rejects_both_text_and_url() {
        let h = fake_handler();
        let err = h
            .count_tokens_inner(CountTokensArgs {
                text: Some("hi".into()),
                url: Some("https://example.com".into()),
                tokenizer: None,
            })
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[tokio::test]
    async fn rejects_neither() {
        let h = fake_handler();
        let err = h
            .count_tokens_inner(CountTokensArgs::default())
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[test]
    fn schema_contains_all_fields() {
        let schema = schemars::schema_for!(CountTokensArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in ["text", "url", "tokenizer"] {
            assert!(json.contains(f), "missing {f}");
        }
    }
}
```

The "text mode happy path" test isn't in this unit test suite because it would require the real tokenizer cache; that scenario is covered in `tests/mcp_smoke.rs` (Task 13) using the fixture tokenizer family seeding pattern.

- [ ] **Step 2: Run the tests**

```bash
cargo test --lib mcp::tools::count_tokens
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/mcp/tools/count_tokens.rs
git commit -m "feat(mcp): count_tokens tool with exactly-one-of validation"
```

---

## Task 11: MCP server lifecycle — serve_stdio, heartbeat, signal handler, ServerHandler impl

**Files:**
- Create: `src/mcp/server.rs`
- Modify: `src/mcp/mod.rs`
- Modify: `src/mcp/handler.rs` (add the `#[tool_router]` impl)

Wires the two tool bodies into rmcp's `ServerHandler` via `#[tool_router]`, then drives the stdio server lifecycle: startup reap → upsert server row → spawn heartbeat → serve → SIGINT/SIGTERM → delete row → exit.

- [ ] **Step 1: Extend `src/mcp/handler.rs` with the tool router impl**

Append to the existing file:

```rust
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::ErrorData;
use rmcp::ServerHandler;

use crate::mcp::tools::count_tokens::CountTokensArgs;
use crate::mcp::tools::fetch::{FetchArgs, FetchOutput};

#[tool_router]
impl RoverHandler {
    /// Fetch a URL and return cleaned Markdown with frontmatter.
    #[rmcp::tool(description = "Fetch a URL and return cleaned Markdown with frontmatter. \
                                Set count_only=true to return only token counts.")]
    pub async fn fetch_tool(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<Json<FetchOutput>, ErrorData> {
        match self.fetch_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }

    /// Count tokens in either an inline `text` or a fetched `url`.
    #[rmcp::tool(description = "Count tokens in either an inline text string or a URL's \
                                extracted Markdown. Exactly one of text/url is required.")]
    pub async fn count_tokens_tool(
        &self,
        Parameters(args): Parameters<CountTokensArgs>,
    ) -> Result<Json<crate::mcp::envelope::CountResponse>, ErrorData> {
        match self.count_tokens_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }
}

#[tool_handler]
impl ServerHandler for RoverHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "rover".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Web fetch & prep for LLM agents. Tools: fetch, count_tokens.".into(),
            ),
        }
    }
}

fn into_error_data(err: crate::mcp::error::McpError) -> ErrorData {
    let r = crate::mcp::error::log_and_translate(err);
    ErrorData {
        code: rmcp::model::ErrorCode::INTERNAL_ERROR,
        message: format!("{}: {}", r.code, r.message),
        data: serde_json::to_value(&r).ok(),
    }
}
```

The exact `rmcp` import paths above are from rmcp 1.7's published docs:
- `rmcp::ServerHandler` (re-export of `rmcp::handler::server::ServerHandler`)
- `rmcp::ServiceExt` (used in `server.rs` step 2)
- `rmcp::transport::io::stdio`
- `rmcp::model::{ErrorData, ErrorCode, CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo}`
- `rmcp::handler::server::wrapper::{Parameters, Json}`
- `#[rmcp::tool]`, `#[rmcp::tool_router]`, `#[rmcp::tool_handler]`

If any path resolves differently in your built `cargo doc --open --no-deps -p rmcp`, adjust the `use` statements accordingly. Run that command before this step.

- [ ] **Step 2: Create `src/mcp/server.rs`**

```rust
//! `rover mcp` server lifecycle.
//!
//! Wires together: startup reap of stale `servers` rows, upsert of the
//! current process's row, a tokio interval heartbeat task, a SIGINT/SIGTERM
//! handler, and the rmcp stdio service.

use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::mcp::handler::RoverHandler;
use crate::storage::Db;

pub async fn serve_stdio(db: Db, config: Arc<Config>) -> anyhow::Result<()> {
    let pid = std::process::id() as i64;
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Startup reap: drop dead rows from prior crashes before claiming our own.
    let reaped = db.reap_stale_servers(config.mcp.reap_threshold).await?;
    if reaped > 0 {
        tracing::info!(
            target: "rover::mcp",
            reaped,
            "reaped stale servers rows on startup"
        );
    }

    db.upsert_server_self(pid, version.clone()).await?;
    tracing::info!(
        target: "rover::mcp",
        pid,
        version = %version,
        "rover mcp registered"
    );

    let cancel = CancellationToken::new();

    // Heartbeat task.
    {
        let db = db.clone();
        let interval = config.mcp.heartbeat_interval;
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(e) = db.heartbeat_server(pid).await {
                            tracing::warn!(target: "rover::mcp", error = ?e, "heartbeat failed");
                        } else {
                            tracing::trace!(target: "rover::mcp", "heartbeat");
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // Signal handler task.
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT");
            let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM");
            tokio::select! {
                _ = sigint.recv() => tracing::info!(target: "rover::mcp", "SIGINT received"),
                _ = sigterm.recv() => tracing::info!(target: "rover::mcp", "SIGTERM received"),
            }
            cancel.cancel();
        });
    }

    let client = reqwest::Client::builder()
        .user_agent(config.fetch.user_agent.clone())
        .timeout(config.fetch.timeout())
        .build()?;
    let handler = RoverHandler::new(db.clone(), config, client);

    let service = handler.serve(stdio()).await?;

    // Wait until either the client closes the transport or a signal fires.
    tokio::select! {
        _ = service.waiting() => {
            tracing::info!(target: "rover::mcp", "client disconnected");
        }
        _ = cancel.cancelled() => {
            tracing::info!(target: "rover::mcp", "shutting down on signal");
        }
    }

    db.delete_server_self(pid).await?;
    Ok(())
}
```

If your dependency graph doesn't already have `tokio-util`, add it to `[dependencies]` in Cargo.toml:

```toml
tokio-util = "0.7"
```

- [ ] **Step 3: Wire `server` into `src/mcp/mod.rs`**

Append:

```rust
pub mod server;

pub use server::serve_stdio;
```

- [ ] **Step 4: Build to verify the rmcp wiring compiles**

```bash
cargo build
```

Expected: clean build. If rmcp's macro path differs slightly (e.g. `rmcp::handler::server::tool::tool_router` instead of `rmcp::tool_router`), inspect `cargo doc --open --no-deps -p rmcp` and fix the `use`/attribute paths. The macros are visible from the crate root in 1.7 (`#[rmcp::tool]` shape is supported).

- [ ] **Step 5: No new unit tests in this task**

Lifecycle is exercised end-to-end in `tests/mcp_smoke.rs` (Task 13). Unit testing tokio interval + signal handling in isolation buys little.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/ Cargo.toml Cargo.lock
git commit -m "feat(mcp): stdio server lifecycle with reaper, heartbeat, signal handler"
```

---

## Task 12: CLI wiring — `rover mcp` subcommand

**Files:**
- Create: `src/cli/mcp.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

Removes the "not yet implemented" stub for `Command::Mcp` and routes it through `cli::mcp::run`, which opens the cache DB and calls `mcp::serve_stdio`.

- [ ] **Step 1: Create `src/cli/mcp.rs`**

```rust
//! `rover mcp` subcommand — start the MCP server over stdio.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use crate::config;
use crate::mcp;
use crate::storage::Db;

pub async fn run(config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = Arc::new(config::load(config_path).context("loading config")?);

    let data_dir = data_dir()?;
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    mcp::serve_stdio(db, cfg).await
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        return Ok(PathBuf::from(env_dir));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
}
```

- [ ] **Step 2: Wire into `src/cli/mod.rs`**

```rust
//! CLI command implementations.

pub mod cache;
pub mod fetch;
pub mod mcp;
```

- [ ] **Step 3: Update `src/main.rs` dispatch**

Replace the `Command::Mcp | Command::Batch { .. } | ...` no-op arm with a dedicated `Mcp` arm:

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
        Command::Mcp => rover::cli::mcp::run(cli.config.as_deref()).await,
        Command::Batch { .. }
        | Command::Task { .. }
        | Command::Doctor
        | Command::Config(_) => {
            eprintln!("not yet implemented (planned for a later milestone)");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rover: {e}");
            ExitCode::from(1)
        }
    }
}
```

- [ ] **Step 4: Quick manual smoke**

```bash
cargo build
# Run in a terminal:
RUST_LOG=debug cargo run --quiet -- mcp <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}
EOF
```

Expected: one JSON-RPC response line on stdout with the server info, then the server exits because stdin closed. Log lines appear on stderr. If the response is malformed or absent, inspect rmcp's stdio handshake and fix the `get_info` shape.

- [ ] **Step 5: Commit**

```bash
git add src/cli/ src/main.rs
git commit -m "feat(cli): wire rover mcp subcommand to MCP stdio server"
```

---

## Task 13: Integration tests

**Files:**
- Create: `tests/mcp_smoke.rs`
- Create: `tests/tokenizer_integration.rs`
- Create: `tests/servers_lifecycle.rs`

Three integration test files. `mcp_smoke` is the canonical end-to-end check; the other two cover specific subsystems.

- [ ] **Step 1: Create `tests/servers_lifecycle.rs`**

```rust
//! Integration test for the storage::servers reaper. No subprocess
//! spawning — uses synthetic rows with controlled heartbeat timestamps.

use std::time::Duration;

use rover::storage::Db;

#[tokio::test]
async fn reap_keeps_recent_rows_drops_old() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();

    db.upsert_server_self(11, "0.1.0".into()).await.unwrap();
    db.upsert_server_self(22, "0.1.0".into()).await.unwrap();
    db.upsert_server_self(33, "0.1.0".into()).await.unwrap();

    // Mark PIDs 11 and 22 as ancient (epoch 0).
    db.conn
        .call(|c| {
            c.execute(
                "UPDATE servers SET last_heartbeat = 0 WHERE pid IN (11, 22)",
                [],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .unwrap();

    let removed = db
        .reap_stale_servers(Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(removed, 2);
    let rows = db.list_servers().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, 33);
}
```

If `Db::conn` isn't `pub`, expose it via a `cfg(test)` accessor in `src/storage/mod.rs`. Alternative: drop this test and rely on the inline unit test in Task 7. Recommended: keep the inline test as the source of truth and delete this file. (Decision left to the implementer; keeping just to make the test surface visible.)

Actually — since `Db::conn` is `pub(crate)` and we want the integration test to exercise the public API, replace the body with a sleep-based version that doesn't poke at `conn`:

```rust
//! Integration test for the storage::servers reaper.

use std::time::Duration;

use rover::storage::Db;

#[tokio::test]
async fn reap_after_threshold_removes_stale_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();

    db.upsert_server_self(11, "0.1.0".into()).await.unwrap();
    // Wait a hair longer than the threshold and reap with a tiny threshold.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let removed = db.reap_stale_servers(Duration::from_secs(1)).await.unwrap();
    assert_eq!(removed, 1);
    assert!(db.list_servers().await.unwrap().is_empty());
}
```

- [ ] **Step 2: Make `Db::list_servers` available outside tests**

The inline `#[cfg(test)]` gate hides `list_servers` from integration tests. In `src/storage/servers.rs`, remove the `#[cfg(test)]` attribute on `list_servers` so the function is public. (Keep the API; remove only the gate.)

- [ ] **Step 3: Create `tests/tokenizer_integration.rs`**

```rust
//! Integration test for tokenizer download + load. Hits HuggingFace, so
//! it's `#[ignore]` by default. Run with: `cargo test --test
//! tokenizer_integration -- --ignored --nocapture`.

use rover::tokenizer::{self, Tokenizer};

#[tokio::test]
#[ignore = "hits HuggingFace network"]
async fn ensure_loaded_then_count_works_for_all_families() {
    let tmp = tempfile::tempdir().unwrap();
    // Direct the XDG root at a tempdir so the test doesn't pollute the user.
    unsafe { std::env::set_var("ROVER_DATA_DIR", tmp.path()) };

    for family in Tokenizer::ALL {
        tokenizer::ensure_loaded(family)
            .await
            .unwrap_or_else(|e| panic!("ensure_loaded({family}) failed: {e}"));
        let n = tokenizer::count("hello world", family).expect("count");
        assert!(n > 0, "expected non-zero tokens for {family}, got {n}");
    }
}
```

`std::env::set_var` is `unsafe` in Rust 2024 edition. The crate is on edition 2024, so the `unsafe` block is required.

- [ ] **Step 4: Create `tests/mcp_smoke.rs`**

```rust
//! End-to-end smoke test for `rover mcp` via an rmcp client + child process.
//!
//! These tests spawn the test-built `rover` binary, speak MCP over stdio,
//! and exercise the `fetch` and `count_tokens` tools against a `wiremock`
//! server. The binary is built with `--features test-loopback` so SSRF
//! allows the wiremock loopback address.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::model::CallToolRequestParam;
use serde_json::json;
use tokio::process::Command;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const HTML_BODY: &str = "<html><head><title>Sample</title></head>\
                          <body><article><h1>Sample</h1>\
                          <p>Hello world from wiremock.</p></article></body></html>";

async fn start_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(HTML_BODY),
        )
        .mount(&server)
        .await;
    server
}

fn bin_path() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("rover")
}

async fn spawn_client(
    data_dir: &std::path::Path,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let mut cmd = Command::new(bin_path());
    cmd.arg("mcp");
    cmd.env("ROVER_DATA_DIR", data_dir);
    cmd.env("RUST_LOG", "info,rover=debug");
    let proc = TokioChildProcess::new(cmd).expect("spawn rover mcp");
    ().serve(proc).await.expect("client handshake")
}

async fn call_tool(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: serde_json::Value,
) -> rmcp::model::CallToolResult {
    client
        .call_tool(CallToolRequestParam {
            name: name.into(),
            arguments: args.as_object().cloned(),
        })
        .await
        .expect("call_tool")
}

#[tokio::test]
async fn lists_two_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"fetch_tool"), "missing fetch_tool: {names:?}");
    assert!(
        names.contains(&"count_tokens_tool"),
        "missing count_tokens_tool: {names:?}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn fetch_against_wiremock_returns_markdown() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(
        &client,
        "fetch_tool",
        json!({"url": server.uri()}),
    )
    .await;
    assert!(!res.is_error.unwrap_or(false), "tool returned error: {res:?}");
    let payload = res
        .structured_content
        .as_ref()
        .or_else(|| res.content.first().and_then(|c| c.as_text()).map(|t| &t.text))
        .expect("response payload");
    let txt = match payload {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    assert!(txt.contains("Sample"), "expected title in markdown: {txt}");
    assert!(txt.contains("cache_status"), "expected cache_status: {txt}");

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn count_only_returns_count_envelope() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(
        &client,
        "fetch_tool",
        json!({"url": server.uri(), "count_only": true}),
    )
    .await;
    assert!(!res.is_error.unwrap_or(false));
    // The structured payload should have `tokens` and not `markdown`.
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"tokens\""), "missing tokens: {blob}");
    assert!(!blob.contains("\"markdown\""), "unexpected markdown: {blob}");

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn max_tokens_exceeded_is_structured_error() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(
        &client,
        "fetch_tool",
        json!({"url": server.uri(), "max_tokens": 1}),
    )
    .await;
    assert!(res.is_error.unwrap_or(false), "expected error: {res:?}");
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("max_tokens_exceeded"),
        "expected code in payload: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn count_tokens_with_text_works() {
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;
    let res = call_tool(
        &client,
        "count_tokens_tool",
        json!({"text": "hello world"}),
    )
    .await;
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"tokens\""));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn count_tokens_neither_arg_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;
    let res = call_tool(&client, "count_tokens_tool", json!({})).await;
    assert!(res.is_error.unwrap_or(false));
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("invalid_args"), "blob: {blob}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn second_fetch_reports_cache_hit() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;

    let _first = call_tool(&client, "fetch_tool", json!({"url": server.uri()})).await;
    let second = call_tool(&client, "fetch_tool", json!({"url": server.uri()})).await;
    let blob = serde_json::to_string(&second).unwrap();
    assert!(blob.contains("\"hit\""), "expected hit, blob: {blob}");

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn servers_row_is_cleaned_up_on_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let client = spawn_client(tmp.path()).await;
        // Round-trip something so the server is definitely up.
        let _ = client.list_all_tools().await.unwrap();
        client.cancel().await.unwrap();
        // Give the server a moment to delete its row.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let db = rover::storage::Db::open(tmp.path().join("rover.db")).await.unwrap();
    let rows = db.list_servers().await.unwrap();
    assert!(rows.is_empty(), "expected servers table empty, got {rows:?}");
}
```

Notes for the implementer:
- The wiremock server must be reachable through SSRF. Currently `cli::mcp::run` always uses `SsrfLevel::Strict`. For these tests to pass, set the env var `ROVER_MCP_SSRF=test-loopback` (or similar) and have `mcp::server::serve_stdio` consult it. **Decision:** add a single env var `ROVER_MCP_SSRF=test_loopback` that switches the SSRF level when the binary is built with `--features test-loopback`. Add this in Task 11 if not already present, or extend `serve_stdio` here as part of Task 13 wiring before running the tests.

  Add to `src/mcp/server.rs` near the `reqwest::Client::builder()` block:

  ```rust
  let ssrf_level = match std::env::var("ROVER_MCP_SSRF").as_deref() {
      #[cfg(feature = "test-loopback")]
      Ok("test_loopback") => crate::fetcher::ssrf::SsrfLevel::TestLoopback,
      _ => crate::fetcher::ssrf::SsrfLevel::Strict,
  };
  ```

  Plumb `ssrf_level` into both tools (extend `RoverHandler` with an `ssrf_level: SsrfLevel` field; pass into `FetchOptions`). Update the `cargo test --features test-loopback ...` invocation below accordingly.

- The exact field names on `CallToolResult` (e.g. `is_error`, `structured_content`, `content`) come from rmcp 1.7's `model` module. If they differ, adjust the assertions. `cargo doc -p rmcp --no-deps --open` is your reference.

- [ ] **Step 5: Plumb `ssrf_level` through `RoverHandler`**

In `src/mcp/handler.rs`, add `pub(crate) ssrf_level: crate::fetcher::ssrf::SsrfLevel` to `RoverHandler`, update `new` to take it, and replace the hardcoded `SsrfLevel::Strict` in both `fetch_inner` and `count_tokens_inner` with `self.ssrf_level`.

In `src/mcp/server.rs`, compute `ssrf_level` as in step 4's note and pass it to `RoverHandler::new`.

In `src/cli/mcp.rs`, pass `SsrfLevel::Strict` to `serve_stdio` (or read the env var inside `serve_stdio` — pick one).

- [ ] **Step 6: Run integration tests**

```bash
# Smoke test (the meat of M3 acceptance).
cargo test --test mcp_smoke --features test-loopback -- --nocapture
# Servers lifecycle (no features).
cargo test --test servers_lifecycle -- --nocapture
# Tokenizer integration (network — skip in CI unless wired).
cargo test --test tokenizer_integration -- --ignored --nocapture
```

Expected: `mcp_smoke` — 8 tests pass. `servers_lifecycle` — 1 test passes. `tokenizer_integration` — 1 test passes when run with `--ignored` and network is available.

If the smoke tests fail because rmcp's child-process transport requires a different `Command` shape (e.g. `.stdin(Stdio::piped()).stdout(Stdio::piped())` set explicitly), inspect rmcp's `transport-child-process` docs and adjust.

- [ ] **Step 7: Commit**

```bash
git add tests/ src/mcp/ src/cli/mcp.rs
git commit -m "test(m3): mcp_smoke (rmcp client over child process), tokenizer + servers integrations"
```

---

## Acceptance check

After Task 13, run the full battery:

```bash
cargo fmt --check
cargo clippy --all-targets --features test-loopback -- -D warnings
cargo test --features test-loopback
cargo test --test tokenizer_integration -- --ignored
```

All four must pass before tagging M3 complete.

Manual smoke (per the design's acceptance criteria):

```bash
# Wire into Claude Code (or any MCP-capable client):
claude mcp add rover -- "$(pwd)/target/debug/rover" mcp
# Then in Claude Code: ask it to fetch a URL. It should hand back markdown.
```

Confirm:
- `rover mcp` stdout contains only JSON-RPC frames (no stray prints).
- After clean shutdown of the client, `sqlite3 ~/.local/share/rover/rover.db 'SELECT * FROM servers'` returns zero rows.
- After a `kill -9`'d run, the next `rover mcp` startup logs `reaped stale servers rows on startup`.

## Self-review summary

- **Spec coverage:** every section of the M3 design has at least one task. Tokenizer module = Tasks 1–4. Frontmatter refactor = Task 5. Config = Task 6. `servers` table = Task 7. MCP envelope+error = Task 8. `fetch` tool = Task 9. `count_tokens` tool = Task 10. Lifecycle = Task 11. CLI wiring = Task 12. Tests = Task 13.
- **Placeholder scan:** no TBDs or "implement later" steps. Two "verify against `cargo doc`" notes (rmcp paths, `CallToolResult` field names) are documented unknowns the spec explicitly defers to plan time and are tagged for verification rather than guesswork.
- **Type consistency:** `Tokenizer` enum, `FetchArgs`/`CountTokensArgs`, `FetchResponse`/`CountResponse`/`RoverError`, `CacheStatus`, `CountSource`, `RoverHandler`, `McpError`, and `ServerRow` field names are stable across tasks. The two tool methods are `fetch_tool` and `count_tokens_tool` (rmcp uses the Rust method name as the tool name by default; smoke tests assert on those names).
- **rmcp escape hatches:** the plan calls out at three points where the implementer must verify rmcp's exact API paths against `cargo doc` (Task 11 step 1, Task 13 step 4, and Task 13 step 6). These are not vague — they identify exactly which import lines to check.

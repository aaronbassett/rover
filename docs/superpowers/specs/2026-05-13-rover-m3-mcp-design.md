# Rover M3 — MCP Server Mode Design

> **Status:** approved design, pending implementation plan.
> **Scope:** Milestone M3 only. Sequels (M4–M9) get their own designs.
> **Companion docs:**
> - PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§4.1 `fetch` tool, §4.5 `count_tokens` tool, §10 token counting).
> - Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` (§2.2 stdio transport, §2.3 multi-instance + servers + owner_pid, §3.4 heartbeat).
> - Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` (M3 section).

## Goal

Ship `rover mcp`: a stdio-bound Model Context Protocol server that exposes the
M1/M2 fetch + cache pipeline behind two MCP tools (`fetch`, `count_tokens`),
wires real tokenizers for the five families in PRD §10, and tracks live
instances in a `servers` table so M6 can later layer task-ownership on top.

## Decisions on the manifest's open questions

The manifest flagged five planning-time questions plus several others that
surfaced during brainstorming. All are resolved here so the implementation plan
doesn't need to re-litigate them.

1. **`rmcp` version & pattern.** Pin `rmcp = { version = "1", features = ["server", "macros", "transport-io"] }`. Use the `#[tool_router]` + `#[tool]` macro pattern; JSON Schema is derived from typed arg structs via `schemars`.
2. **`fetch` tool arg shape.** Live args drive behavior in M3: `url`, `force_refresh`, `count_only`, `tokenizer`, `max_tokens`. Accept-and-no-op args reserve schema surface for later milestones: `headless`, `tables`, `images`, `metadata`, `summarize`. Each no-op arg emits one `tracing::debug` line noting the deferral.
3. **`force_refresh` plumbing.** MCP arg → `FetchArgs.force_refresh` → directly into `fetcher::cached::fetch_with_cache`'s existing flag.
4. **Token counter call sites & `max_tokens` behavior.** M1's `extractor::frontmatter::estimate_tokens(&str)` is **removed** (pre-release; no shims). A new `tokenizer::count(&str, Tokenizer)` replaces it, and the single frontmatter call site is updated to pass a tokenizer chosen from `[tokenizer] default` (or per-call MCP arg). When `max_tokens` is exceeded, the `fetch` tool returns a structured error with `code: "max_tokens_exceeded"` pointing at `summarize` (which doesn't exist until M7).
5. **MCP test harness.** `tests/mcp_smoke.rs` uses `rmcp`'s own client with the `transport-child-process` feature, spawning the test-built `rover` binary via `assert_cmd::cargo::cargo_bin`. Closest fidelity to how Claude Code will connect.
6. **Tokenizer scope.** All five PRD §10 families ship in M3: `cl100k`, `o200k`, `claude`, `llama3`, `qwen3`.
7. **Tokenizer crate strategy.** Unify on the HuggingFace `tokenizers` crate for every family. No `tiktoken-rs`. One code path, one cache shape.
8. **Tokenizer storage & download.** Lazy load from `$XDG_DATA_HOME/rover/tokenizers/<family>/tokenizer.json`. First call for a family triggers an `hf-hub` download into that path; later calls hit an in-process `OnceCell`-backed map of `Arc<HfTokenizer>`.
9. **Default tokenizer.** `[tokenizer] default = "o200k"`. Configurable in TOML; the MCP `fetch.tokenizer` arg overrides per-call.
10. **`count_only` semantics.** Same cache behavior as a normal fetch (writes through), response payload omits the markdown body and includes `tokens`, `tokenizer`, `url`, `content_hash`, `fetched_at`, `cache_status`.
11. **`count_tokens` tool surface.** `{ text?: string, url?: string, tokenizer?: string }`. Exactly one of `text`/`url` is required. URL mode reuses the cached fetch pipeline.
12. **Orphan reap in M3.** Reap stale `servers` rows on startup only (rows with `last_heartbeat < now() - reap_threshold`, default 60s). No `tasks` scan — that whole table doesn't exist until M6.

## Architecture

`rover mcp` becomes a stdio-bound MCP server using `rmcp 1.7` with `server` +
`macros` + `transport-io` features. Tool registration uses `#[tool_router]` /
`#[tool]`; JSON Schema is derived via `schemars` from typed arg structs.

A new `src/tokenizer/` module owns the `Tokenizer` enum (`Cl100k`, `O200k`,
`Claude`, `Llama3`, `Qwen3`) and a process-wide `OnceCell<RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>>>`
cache. First use of a family triggers an `hf-hub` download of the canonical
`tokenizer.json` into `$XDG_DATA_HOME/rover/tokenizers/<family>/`, then parse
and cache. All five families load through the HuggingFace `tokenizers` crate.

A new `src/mcp/` module hosts the server handler, tool definitions, and a
shared envelope type. The MCP `fetch` tool wraps `fetcher::cached::fetch_with_cache`
(M2) + the extractor pipeline (M1) + the frontmatter writer. The MCP
`count_tokens` tool branches on `text`/`url` input; URL mode shares the fetch
pipeline.

Multi-instance lifecycle: migration `002_servers.sql` adds the `servers` table
(`pid`, `started_at`, `last_heartbeat`, `version`). `rover mcp` startup upserts
its row, reaps stale rows once, then spawns a heartbeat task on a tokio
interval (~5s). SIGINT/SIGTERM trap → delete own row → exit. No `owner_pid`
column on `pages`; that arrives with M6.

### Module layout introduced by M3

```
src/mcp/
  mod.rs                              # pub mod surface
  server.rs                           # serve_stdio(), startup/shutdown wiring
  handler.rs                          # RoverHandler { db, config, client }
  envelope.rs                         # FetchResponse, CountResponse, RoverError
  error.rs                            # McpError (internal thiserror enum)
  tools/
    mod.rs
    fetch.rs                          # #[tool] fn fetch + FetchArgs
    count_tokens.rs                   # #[tool] fn count_tokens + CountTokensArgs

src/tokenizer/
  mod.rs                              # public count() + ensure_loaded()
  registry.rs                         # Tokenizer enum, FromStr, repo-id table
  hf.rs                               # HfTokenizer wrapper, OnceCell map
  download.rs                         # hf-hub fetch into XDG
  error.rs                            # TokenizerError

src/storage/
  servers.rs                          # upsert_self, heartbeat, reap_stale, delete_self
  migrations/
    002_servers.sql                   # CREATE TABLE servers (...)

src/cli/
  mcp.rs                              # rover mcp subcommand body (tiny)

tests/
  mcp_smoke.rs
  tokenizer_integration.rs
  servers_lifecycle.rs
  fixtures/tokenizer/                 # small fixture tokenizer.json for unit tests
```

### Files modified

- `src/main.rs` — replace the `Mcp` placeholder with `cli::mcp::run`.
- `src/cli/mod.rs` — `pub mod mcp;`.
- `src/config.rs` — add `[tokenizer]` and `[mcp]` sections.
- `src/error.rs` — add `Mcp(#[from] McpError)` and `Tokenizer(#[from] TokenizerError)` variants.
- `src/extractor/frontmatter.rs` — drop `estimate_tokens`; accept a `Tokenizer` (or precomputed token count) on the writer signature.
- `src/storage/mod.rs` — embed migration `002_servers.sql`; re-export `servers` module.
- `Cargo.toml` — add `rmcp`, `tokenizers`, `hf-hub`, `schemars` (if not already a transitive dep), `tokio` features for `signal`.

## Components

### `src/tokenizer/`

- **`registry.rs`** — `Tokenizer` enum with `serde` rename
  (`"cl100k" | "o200k" | "claude" | "llama3" | "qwen3"`), `FromStr`, `Display`.
  Const map from variant to canonical HF repo id + filename. Repo IDs pinned at
  implementation plan time after the planner audits which HF mirrors are
  canonical for each family.
- **`hf.rs`** — wraps `tokenizers::Tokenizer`, exposes
  `count(text: &str) -> usize`. One `OnceCell<RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>>>`
  per process.
- **`download.rs`** — uses `hf-hub`'s sync API behind
  `tokio::task::spawn_blocking` to fetch `tokenizer.json` into XDG. Returns a
  `PathBuf` for the on-disk file. Emits `tracing::info` spans for download
  start and end with byte counts.
- **`mod.rs`** — public surface:
  - `pub async fn ensure_loaded(t: Tokenizer) -> Result<(), TokenizerError>`
  - `pub fn count(text: &str, t: Tokenizer) -> Result<usize, TokenizerError>`
    (must be preceded by `ensure_loaded`; returns `NotLoaded` otherwise to
    keep `count` synchronous and cheap).
- **`error.rs`** — `TokenizerError`: `Download`, `Parse`, `Io`, `UnknownFamily(String)`, `NotLoaded(Tokenizer)`.

### `src/mcp/`

- **`server.rs`** — `pub async fn serve_stdio(db: Db, config: Arc<Config>) -> Result<()>`.
  Constructs `RoverHandler`, registers tools via `#[tool_router]`, binds to
  `transport-io::stdio()`. Spawns server-row insert + heartbeat before serve,
  installs SIGINT/SIGTERM handler, deletes row on graceful shutdown.
- **`handler.rs`** — `RoverHandler { db: Db, config: Arc<Config>, client: reqwest::Client }`.
- **`envelope.rs`** —
  - `FetchResponse { markdown: String, frontmatter: String, cache_status: CacheStatus }`
  - `CountResponse { tokens: usize, tokenizer: String, source: CountSource, url?: String, content_hash?: String, fetched_at?: String, cache_status?: CacheStatus }`
  - `RoverError { code: &'static str, message: String }`
- **`error.rs`** — internal `McpError` enum with `#[from]` for `TokenizerError`,
  `FetcherError`, `ExtractorError`, `StorageError`, plus `InvalidArgs(String)`
  and `Transport(rmcp::Error)`.
- **`tools/fetch.rs`** — `FetchArgs { url, force_refresh?, count_only?, tokenizer?, max_tokens?, headless?, tables?, images?, metadata?, summarize? }`.
  Accept-no-op args emit `tracing::debug!(arg = "tables", value = ?value, "ignored until M4")`
  (and analogues). Live args drive `fetch_with_cache`, optionally swap the
  tokenizer, optionally trim to a count-only response. On `max_tokens`
  exceeded → `RoverError { code: "max_tokens_exceeded", … }`.
- **`tools/count_tokens.rs`** — `CountTokensArgs { text?, url?, tokenizer? }`.
  Validates exactly one of `text`/`url` is set. URL path reuses
  `fetch_with_cache`. Returns `CountResponse { source: Text | Url, … }`.

### `src/storage/servers.rs`

Async API: `upsert_self(pid, version)`, `heartbeat(pid)`,
`reap_stale(threshold: Duration) -> usize`, `delete_self(pid)`. Inline unit
tests against an in-memory connection.

### `src/cli/mcp.rs`

Body of the `Mcp` subcommand. Loads config, opens `Db`, calls
`mcp::server::serve_stdio(db, config)`. Tiny — most logic lives in `mcp::server`.

### Config additions (`src/config.rs`)

```toml
[tokenizer]
default = "o200k"

[mcp]
heartbeat_interval = "5s"
reap_threshold    = "60s"
```

Both sections use `humantime-serde` for durations (already a dep from M2).

## Data flow

### `rover mcp` startup

1. Parse CLI → load TOML config → `telemetry::init` (stderr sink, since stdout is the MCP transport).
2. `Db::open` (applies migrations 001 + 002).
3. `servers::upsert_self(pid, rover_version)`.
4. `servers::reap_stale(reap_threshold)` once at startup — drops dead rows from prior crashes.
5. Spawn heartbeat task: `tokio::interval(heartbeat_interval)` → `servers::heartbeat(pid)`. Logs at `trace`.
6. Spawn signal handler (SIGINT, SIGTERM) → cancellation token → `servers::delete_self(pid)` → exit 0.
7. `rmcp::ServiceExt::serve(handler, stdio())` runs until cancelled or stdin EOF.

### `fetch` tool call

```
client → rmcp → RoverHandler::fetch(FetchArgs)
  → log no-op for headless/tables/images/metadata/summarize
  → fetcher::cached::fetch_with_cache(url, force_refresh)        [M2]
      → returns FetchedPage + cache_status
  → extractor::pipeline::extract(bytes, charset)                 [M1]
  → tokenizer = args.tokenizer.unwrap_or(config.tokenizer.default)
  → tokenizer::ensure_loaded(tokenizer).await?
  → tokens = tokenizer::count(markdown, tokenizer)?
  → if args.max_tokens.is_some_and(|m| tokens > m):
        Err(RoverError { code: "max_tokens_exceeded", … })
  → frontmatter::write(meta, tokens, tokenizer_name)
  → if args.count_only:
        Ok(CountResponse { tokens, tokenizer, source: Url, url, content_hash, fetched_at, cache_status })
  → else:
        Ok(FetchResponse { markdown, frontmatter, cache_status })
```

### `count_tokens` tool call

```
client → rmcp → RoverHandler::count_tokens(CountTokensArgs)
  → validate exactly-one-of text/url
  → tokenizer = args.tokenizer.unwrap_or(config.tokenizer.default)
  → tokenizer::ensure_loaded(tokenizer).await?
  → if text:
        Ok(CountResponse { tokens: count(text, tokenizer)?, tokenizer, source: Text })
  → if url:
        page = fetch_with_cache(url, force_refresh=false)
        markdown = extract(page)
        Ok(CountResponse { tokens: count(markdown, tokenizer)?, tokenizer, source: Url, url, content_hash, fetched_at, cache_status })
```

### First-use tokenizer load (lazy)

```
ensure_loaded(t):
  → fast path: registry map has t → return Ok
  → slow path: spawn_blocking move ||
      path = $XDG_DATA_HOME/rover/tokenizers/<t>/tokenizer.json
      if !path.exists():
          hf_hub::api::sync::Api::new()?.model(repo_id).get(filename)?
          (hf-hub stages into its own cache; we then copy or symlink into XDG)
      parsed = tokenizers::Tokenizer::from_file(path)?
      registry: write Arc<HfTokenizer>
      Ok
count(text, t):
  → registry read → Arc::clone → encode → ids.len()
```

The download path is network-bound (~MB-scale JSON, seconds on a typical
connection). Parse is ~50–200ms. Subsequent calls hit the in-memory `Arc`
with no I/O.

### Heartbeat loop

```
loop {
    tokio::time::sleep(heartbeat_interval).await;
    if cancellation_token.is_cancelled() { break; }
    if let Err(e) = servers::heartbeat(pid).await {
        tracing::warn!(error = ?e, "heartbeat failed");
    }
}
```

No per-tick reap. Startup reap is sufficient for M3.

## Error handling

Module errors stay as per-module `thiserror` enums (manifest cross-cut).

```rust
// src/tokenizer/error.rs
pub enum TokenizerError {
    Download(#[from] hf_hub::ApiError),
    Parse(#[from] tokenizers::Error),
    Io(#[from] std::io::Error),
    UnknownFamily(String),
    NotLoaded(Tokenizer),
}

// src/mcp/error.rs
pub enum McpError {
    Tokenizer(#[from] TokenizerError),
    Fetcher(#[from] FetcherError),
    Extractor(#[from] ExtractorError),
    Storage(#[from] StorageError),
    InvalidArgs(String),
    Transport(#[from] rmcp::Error),    // exact path pinned at plan time
}
```

These bubble up via `crate::Error` (extended with `Mcp` and `Tokenizer`
variants).

### MCP-tool boundary translation

`rmcp` tool handlers return `Result<T, rmcp::Error>` over the wire. We never
leak internal `thiserror` types to clients. Every tool body returns
`Result<T, RoverError>`; a single translation layer maps `McpError` →
`RoverError { code, message }`:

| Internal failure | `code` | Message shape |
|---|---|---|
| `TokenizerError::Download` | `tokenizer_unavailable` | `could not fetch tokenizer for <family>: <reason>` |
| `TokenizerError::Parse` | `tokenizer_unavailable` | `tokenizer file for <family> is corrupt: <reason>` |
| `TokenizerError::UnknownFamily` | `invalid_args` | `unknown tokenizer family: <name>` |
| `TokenizerError::NotLoaded` | `tokenizer_unavailable` | `tokenizer <family> not loaded` (internal bug — should never reach clients) |
| `FetcherError::Ssrf` | `ssrf_denied` | `<url> rejected by SSRF policy: <reason>` |
| `FetcherError::Status(4xx/5xx)` | `fetch_failed` | `<url> returned <code>` |
| `FetcherError::Decode` / charset | `fetch_failed` | `could not decode <url>: <reason>` |
| `ExtractorError::*` | `extract_failed` | `extraction failed for <url>: <reason>` |
| `StorageError::*` | `storage_error` | `cache backend error: <reason>` |
| `max_tokens` exceeded | `max_tokens_exceeded` | `extracted content is N tokens; max_tokens=M. summarize tool not yet available (M7)` |
| `count_tokens` neither/both of text/url | `invalid_args` | `count_tokens requires exactly one of text or url` |

`code` strings are stable from M3 onward and will be documented in
`docs/mcp-tools.md` when M8 writes that doc.

### Transport-level error shape

A `RoverError` becomes an MCP `tool_result` with `is_error: true` and JSON
content `{ code, message }`. The handler wrapper performs this translation
once, not per-tool.

### Logging

Every translation emits one `tracing::warn` with the full internal error chain
(`error = ?err`) so the MCP-facing message stays user-friendly while operators
retain full diagnostics on stderr. SSRF denials log at `info`, not `warn`
(they are expected user-error class, not server faults).

### Server lifecycle errors

- `Db::open` failure during startup → exit 1, log to stderr, do not start serving. No `servers` row written.
- `servers::upsert_self` failure → exit 1.
- Heartbeat failures → `warn`, keep serving.
- Signal handler install failure → exit 1.
- Stdin EOF (client disconnect) → graceful shutdown, delete own row, exit 0.

### Panic policy

Same as M1/M2: no `unwrap`/`expect` outside tests and known-infallible paths.
Module errors carry `#[source]` to preserve chains.

## Testing

### Unit tests (inline `#[cfg(test)] mod tests`)

- `tokenizer::registry` — `FromStr` round-trips for all five families; unknown family produces `UnknownFamily(name)`.
- `tokenizer::hf` — given a small fixture `tokenizer.json` under `tests/fixtures/tokenizer/`, `count` returns expected token counts for short strings. No download path exercised here.
- `storage::servers` — `upsert_self` is idempotent; `heartbeat` updates `last_heartbeat` monotonically; `reap_stale` deletes rows older than the threshold and returns the count; `delete_self` is idempotent.
- `mcp::envelope` — `RoverError` JSON shape is stable (snapshot or hand-written assertion).
- `mcp::tools::fetch::FetchArgs` — `schemars`-derived schema contains the five live args and the five accept-no-op args; missing `url` is a deserialize error.
- `mcp::tools::count_tokens` — exactly-one-of validation for `text`/`url`.

### Integration tests (`tests/`)

```
tests/mcp_smoke.rs              # rover mcp end-to-end via rmcp client
tests/tokenizer_integration.rs  # real HF download to a tempdir
tests/servers_lifecycle.rs      # multi-row simulation of heartbeat + reap
```

#### `tests/mcp_smoke.rs`

Uses `rmcp`'s own client with `transport-child-process` to spawn the
test-built `rover` binary via `assert_cmd::cargo::cargo_bin`. Sets
`ROVER_DATA_DIR` to a tempdir to keep the test hermetic. Cases:

1. `tools/list` returns exactly two tools, `fetch` and `count_tokens`, with the expected input-schema fields.
2. `fetch` against a `wiremock` server (with `--features test-loopback`) returns markdown with frontmatter and `cache_status: "miss"`.
3. Second `fetch` of the same URL returns `cache_status: "hit"`.
4. `fetch` with `force_refresh: true` returns `cache_status: "miss"` again (or `revalidated_304` if the etag matches).
5. `fetch` with `count_only: true` returns the count envelope with no `markdown` field.
6. `fetch` with `max_tokens: 1` against a page that exceeds it returns `is_error: true` with `code: "max_tokens_exceeded"`.
7. `count_tokens` with `text: "hello world"` returns a positive count; with both `text` and `url` set returns `code: "invalid_args"`.
8. Clean shutdown (client-side close) → the `servers` row is gone by the time a follow-up test process opens the DB.

#### `tests/tokenizer_integration.rs`

Gated behind `#[ignore]` (network); CI opts in. Covers: first-call download to
a tempdir, second-call hits the in-memory cache (verified via a download-call
counter or `tracing` subscriber), all five families parse and tokenize a fixed
input.

#### `tests/servers_lifecycle.rs`

Opens a `Db` against a tempfile, inserts three synthetic rows with controlled
`last_heartbeat` timestamps, runs `reap_stale(threshold)`, asserts only the
recent rows survive. No subprocess spawning; the multi-process behavior is
exercised in `mcp_smoke` test 8.

### Conventions carried forward

- `wiremock` for HTTP mocking; `test-loopback` SSRF feature unchanged.
- Each integration test gets its own tempdir + `ROVER_DATA_DIR`. No shared state.
- `assert_cmd::cargo::cargo_bin("rover")` for binary discovery (already used by M1's `tests/cli_fetch.rs`).
- TDD discipline: each task in the implementation plan starts from a failing test.

## Acceptance criteria

PRD §14 M3: "Claude Code can connect to Rover and successfully fetch URLs."

Concrete checks the implementation plan must produce green:

- `cargo test` passes, including the eight `mcp_smoke` cases.
- Manual: `claude mcp add rover -- cargo run -- mcp` (or equivalent) lists `fetch` and `count_tokens` in Claude Code and fetches a real URL.
- `rover mcp` startup is silent on stdout; all diagnostics go to stderr.
- After clean shutdown, `SELECT * FROM servers` returns zero rows.
- After a SIGKILL'd run, the next `rover mcp` startup reaps the orphaned row.

## Deferred from M3

- `headless` arg behavior — schema-accepted, body deferred to M9.
- `tables`, `images`, `metadata` arg structures — schema-accepted, bodies in M4.
- `summarize` arg, `max_tokens` auto-summarize — M7. M3 errors instead.
- `batch_fetch`, `summarize`, `get_metadata` MCP tools — M6, M7, M4 respectively.
- Stale-while-revalidate task scheduling — M6 (M2 ships stale-served-without-revalidation today).
- `owner_pid` column on `pages` and on the future `tasks` table — M6.
- All `rover doctor` checks of M3 subsystems — M8.

## Out of scope (won't fix in M3)

- Tokenizer model refresh / version pinning UX. The HF repo pulls whatever the
  default revision is; if a tokenizer family ships a breaking update, we'll
  pin in a follow-up. M8's `rover doctor` will surface mismatches.
- DNS-rebinding-resistant fetching (design supplement §2.4 — v2 work).
- Streaming MCP tool responses. `rmcp` doesn't natively stream tool output; we
  return collected `String`s. Revisit in M7 if summarization latency demands it.

## Dependencies added in M3

- `rmcp = { version = "1", features = ["server", "macros", "transport-io"] }`
- `tokenizers` (HuggingFace; latest stable at plan time)
- `hf-hub` (sync feature; latest stable at plan time)
- `schemars` — likely pulled transitively by `rmcp` with the `macros` feature; add explicitly if not.
- `tokio` `signal` feature (extend the existing dep).

Dev-deps: `rmcp` with the `client` + `transport-child-process` features for
`tests/mcp_smoke.rs`.

## Forward-looking notes for later milestones

- **M6 (tasks).** Will add `owner_pid` columns on `tasks` and a task-orphan
  scanner that joins `tasks` against `servers`. The `servers` table M3 ships
  is the foundation; no schema changes needed in M6 for the server side.
- **M7 (summarization).** Will reuse the `Tokenizer` registry to size
  abstractive prompts and to count cache-key inputs in `summary_cache`.
- **M8 (`rover doctor`).** Will add per-tokenizer "downloaded and parses"
  checks, plus a `servers` table sanity check.
- **M9 (`local-inference`).** Model download UX should reuse the same
  spawn-blocking + XDG + tracing pattern as `tokenizer::download`. Lift into
  a shared `download` module if a second consumer appears.

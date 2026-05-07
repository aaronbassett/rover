# Rover — Design Supplement

> Architectural decisions, PRD corrections, and implementation conventions for Rover v1.

**Status:** draft, awaiting review.
**Date:** 2026-05-07.
**Companion:** [`../prd/2026-05-07-rover-prd.md`](../prd/2026-05-07-rover-prd.md) — the PRD remains canonical for product surface, scope, and feature behavior. This supplement records architectural choices the PRD left open or got slightly wrong.

---

## 1. Status & Relationship to PRD

The PRD is the source of truth for **what Rover does**. This supplement is the source of truth for **how it is built** at the level of cross-cutting decisions.

When the PRD and this document conflict, this document wins for the items it explicitly addresses (listed in §3 Corrections). The PRD wins for everything else.

Whenever a decision recorded here is reversed, edit this document and bump its date — do not let the PRD and design drift apart silently.

---

## 2. Cross-Cutting Decisions

These were resolved during the brainstorming session that produced this document. Each subsection records the decision, the rationale, and what other design surfaces it touches.

### 2.1 Async access to SQLite

**Decision:** use `tokio-rusqlite`.

A single connection actor (a worker thread fed by an MPMC channel) owns the database file. All call sites await on a typed request, get back a typed response. No `spawn_blocking` ceremony at every call site. No connection-pool plumbing. No build-time SQL checks (we accept that — schema changes are rare and migrations are tested).

The `storage` module exposes a thin async API surface (`pages::get_by_canonical(...)`, `tasks::insert(...)`, etc.) that hides the actor. Consumers never touch `rusqlite` types directly.

**Implications:**
- Long-running queries (anything > a few ms) must still avoid blocking the actor for the whole connection — chunk where appropriate, or open a second short-lived connection if a heavy read shouldn't compete with writes.
- For very hot read paths (cache lookups), consider a second read-only connection in WAL mode. Defer until profiling justifies it.
- Schema migrations run inline at startup, on the actor thread, before serving requests.

### 2.2 MCP transport

**Decision:** stdio only.

Claude Code launches MCP servers as subprocesses over stdio; that's our primary client and the entire integration is well-served by it. No TCP listener, no auth, no port management.

**Implications:**
- `tracing` output goes to **stderr**, never stdout (stdio MCP transport owns stdout). The `tracing-subscriber` setup hard-codes stderr when `rover mcp` runs.
- Optional file logging via `[debug] log_path = "..."`. When set, an additional `tracing` layer writes there; stderr layer continues.
- Adding HTTP/SSE later is a v2 decision and would require coordinating with the `[server]` config block (bind address, auth).

### 2.3 Instance model: multi-instance with PID-tagged heartbeats

**Decision:** allow multiple `rover mcp` processes to run concurrently. Track each as a row in a new `servers` table; tasks carry the PID of the server that owns them; CLI liveness checks "is *any* server alive in the last N seconds."

This is a deviation from PRD §3.3, which assumes a single writer. The PRD assumption breaks the moment a user has two Claude Code sessions open, since each launches its own subprocess.

**Schema delta vs PRD §8.1:**

```sql
-- Replaces 'last_heartbeat' row inside `system`
CREATE TABLE servers (
    pid           INTEGER PRIMARY KEY,
    started_at    INTEGER NOT NULL,
    last_heartbeat INTEGER NOT NULL,
    version       TEXT NOT NULL          -- crate version string, for diagnostics
);

-- New column on `tasks`
ALTER TABLE tasks ADD COLUMN owner_pid INTEGER;  -- nullable; NULL = not yet claimed
CREATE INDEX tasks_owner ON tasks(owner_pid, status);
```

**Server lifecycle:**
- On startup: insert row with own PID. If a row with this PID already exists (rare — PID reuse), update.
- Every ~5s: `UPDATE servers SET last_heartbeat = ? WHERE pid = ?`.
- On clean shutdown (SIGTERM/SIGINT): `DELETE FROM servers WHERE pid = ?`. On crash: row stays until reaped.
- On startup, opportunistic reap: delete `servers` rows where `last_heartbeat < now() - 60s`. For each reaped PID, mark its `running` tasks `failed` with `error='owner_died'` (or rebroadcast for resumable kinds — see below).

**Task ownership and resumption:**
- New tasks claim themselves: `INSERT INTO tasks (..., owner_pid) VALUES (..., $own_pid)`.
- A task is "orphaned" iff its `owner_pid` does not appear in `servers`. Because the startup reap deletes any server row whose `last_heartbeat` is older than 60s (and clean shutdowns delete their row immediately), this check is the single source of truth — no separate timestamp on the task is needed.
- Live servers periodically scan for orphans. Per task kind:
  - `batch_fetch`, `retry`, `revalidate`: any live server may claim it (CAS-style: `UPDATE tasks SET owner_pid = $own_pid WHERE id = ? AND owner_pid = $orphaned_pid`). Resume from persisted progress.
  - `summarize`: mark `failed` with `error='owner_died'`, let the agent re-request.

**CLI liveness check (replaces PRD §9.4):**

```sql
SELECT MAX(last_heartbeat) FROM servers;
```

If `now() - max_heartbeat > 30s` and the task is `running`, warn:

> ⚠ Task is marked `running` but no `rover mcp` process appears to be alive.
> Start one with `rover mcp` to resume work.

### 2.4 DNS rebinding protection: deferred to v2

**Decision:** do not implement DNS-rebinding-resistant fetching in v1. The SSRF policy still validates the resolved IP at request time, but we do not pin the resolved IP through the connection. This is an explicit deviation from PRD §5.5.

**Rationale:** the PRD's stated approach (resolve, validate IP, then connect by IP with `Host` header) is incompatible with HTTPS — TLS SNI and certificate verification both target the hostname, and presenting a cert for `example.com` while connecting to a raw IP fails verification. The clean fix (`reqwest::ClientBuilder::resolve` to pin a hostname → IP for the connection while keeping SNI/cert intact) is doable but adds enough surface area that we'd rather get the rest of v1 shipping.

For the single-user-local target, the realistic exposure is a malicious link causing a TOCTOU between the SSRF check and the connection. We accept that for v1.

**Implementation requirement (must do in v1):**
- Resolve hostname via `tokio::net::lookup_host` (or `reqwest`'s default resolver), validate every returned address against the active SSRF policy, reject if any address is disallowed. The actual connection then uses the system resolver again — TOCTOU window exists.
- Document this in `docs/security.md` under "known v1 limitations."

**v2 follow-up:** implement the `reqwest::ClientBuilder::resolve` approach, document, remove the limitation note.

### 2.5 Charset detection pipeline: as PRD §5.1

**Decision:** the PRD's pipeline is correct as written. No changes.

`readabilityrs` accepts `&str` and expects pre-decoded UTF-8, so there is no double-decode concern. We run charset detection on the raw bytes from `reqwest::Response::bytes()`, decode to a `String`, then hand that to `readabilityrs::Readability::new(html, Some(url), Some(options))`.

The diagnostic log line "HTTP-declared charset differs from detected" is a `tracing` `info!` event under the `rover::fetcher::charset` target.

### 2.6 Default backend lives under `[summarization]`

**Decision:** introduce a `[summarization]` config section. Move `default_backend` there.

**Corrected config snippet (replaces PRD §7.1 trailing `default_backend = ...` and the duplicate at the bottom of PRD §12):**

```toml
[summarization]
default_backend = "default"

[backends.default]
kind = "extractive"

[backends.openai]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"

# ... other named backends
```

**Rationale:** placing `default_backend = "..."` after `[backends.openai]` (as PRD shows) silently parses as `backends.openai.default_backend`, which is wrong. A bare top-level key works only if hoisted above all tables — fragile. A semantic section avoids the trap and gives future summarization-wide defaults a home (e.g., `default_mode`, `default_style`).

### 2.7 `summarize` MCP tool on cache miss

**Decision:** on cache miss, internally call `fetch` with default options, then summarize. No extra tool arguments.

This makes `summarize` a one-call shortcut for the common case. If an agent needs control over fetch-side options (headless, tables mode, metadata preset), the documented path is "call `fetch` first, then call `summarize`."

The tool docstring must spell this out:

> If the URL is not in the cache, Rover will fetch it with default options and then summarize. For control over the fetch step, call `fetch` first.

### 2.8 Output paths: single `output_dir` with subdirs

**Decision:** the `[server]` section's `output_dir` is the single root for all sidecar files. Within it:

```
$output_dir/tables/<host>/<sha8>.csv
$output_dir/images/<host>/<sha8>.<ext>
```

`<sha8>` is the first 8 hex characters of `sha256(absolute_url_of_the_table_or_image)`. Subdirectories are created on first write.

**Per-call overrides:**
- `tables.csv_dir: PathBuf | null` — overrides the default `tables/` subdir.
- `images.download_dir: PathBuf | null` — overrides the default `images/` subdir (this exists in PRD; we just give it a coherent default).

**Frontmatter:** `csv_path` and image `src` rewrites use **absolute** paths. This eliminates the "relative to what" ambiguity in the PRD example.

```yaml
tables_transformed:
  - id: t2
    original_rows: 89
    applied_mode: csv_file
    csv_path: /home/user/rover-output/tables/example.com/3a7f1c2e.csv
```

---

## 3. PRD Corrections

Small, targeted fixes for inconsistencies in the PRD. None of these change the product surface meaningfully — they just pick a single answer where the PRD picked two.

### 3.1 Task ID format

PRD shows both `batch_abc123` (PRD §4.2) and `abc123` (PRD §9.1). PRD §13 says UUIDv7.

**Pick:** UUIDv7 strings (e.g., `0190bf68-3a4c-7000-8000-...`), no kind prefix. The kind is a separate column on the `tasks` row. CLI commands take the bare UUID.

### 3.2 Task kind naming

PRD §3.3 references the kind `summarization`; PRD §8.1 lists `summarize`. **Pick:** `summarize` everywhere.

### 3.3 `revalidation_task_id` envelope

PRD §4.1 mentions `revalidation_task_id` is "included for optional monitoring" but does not spell out the envelope shape. Other deferred-task returns (PRD §4.2, §9.1) use `{task_id, monitor_command, poll_command, hint}`.

**Pick:** consistency wins. Stale-served fetch returns:

```json
{
  "content": "...",
  "metadata": { ... },
  "cache_status": "stale_served",
  "revalidation": {
    "task_id": "0190bf68-...",
    "monitor_command": "rover task 0190bf68-... --monitor",
    "poll_command": "rover task 0190bf68-...",
    "hint": "Optional. Revalidation runs in the background regardless."
  }
}
```

### 3.4 Heartbeat row → `servers` table

Per §2.3 above. The `system` table no longer carries `last_heartbeat`. It still carries `schema_version` and any other future scalar metadata.

### 3.5 `summary_cache.params_hash` includes the backend identity

PRD §8.4 says the params hash covers `(model, mode, target, focus, preserve, style)`. **Add:** the backend's *name* (the config key, e.g., `"fast"`, `"smart"`). Two backends pointing at the same model can produce different output (system prompts, sampling params), and the cache should reflect that.

### 3.6 Cache `min_ttl` semantics: clarification, not change

PRD §8.2 step 2 reads "If `min_ttl` is configured, ensure final TTL ≥ `min_ttl` (this is the only way `no-store` can be overridden)." Read literally, that suggests `min_ttl` overrides `no-store` automatically. **Clarification:** `min_ttl` only floors the TTL when the entry would otherwise be cached. `no-store` skips caching unless `override_no_store` (global or per-domain) is also true. If both `override_no_store` and `min_ttl` apply, the entry is cached with at least `min_ttl`.

---

## 4. Implementation Conventions

### 4.1 Logging

- Crate: `tracing` + `tracing-subscriber` with `EnvFilter`.
- Default filter: `info,rover=debug` (the binary is verbose about its own internals; deps stay at info).
- Override via `RUST_LOG` env var or `[debug] log_level` config.
- `rover mcp` writes `tracing` output to **stderr** (stdio MCP transport owns stdout). Optional second writer to `[debug] log_path` (a file) when configured.
- For non-`mcp` subcommands, primary output goes to **stdout** — Markdown for `fetch`, human snapshots or NDJSON for `batch`/`task`, listings for `cache *`. `tracing` output goes to **stderr**. This lets `rover fetch ... | jq` and `rover batch ... --monitor | tee` work cleanly.
- Span on every fetch: `fetch.url`, `fetch.host`, `fetch.cache_status`, `fetch.duration_ms`. Redact query parameters matching `api_key`, `token`, `secret`, `password` (case-insensitive substring match) before logging.

### 4.2 Schema migrations

- Embedded SQL files under `crates/rover-storage/migrations/NNN_description.sql`.
- On startup, the storage actor:
  1. Reads `system.schema_version` (default 0 if missing).
  2. Applies each migration with a higher number in order, inside a transaction per file.
  3. Updates `system.schema_version` after each.
- No down-migrations in v1. If a release ever needs one, it ships a one-shot tool, not in-band rollback.
- `rover doctor` reports the schema version.

### 4.3 Configuration layering

Precedence (highest wins):

1. CLI flags (per-invocation).
2. Environment variables — used **only** for API keys (per `genai` conventions: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) and `RUST_LOG`. We deliberately do not allow `ROVER_*` env vars to override arbitrary config — too easy to accidentally diverge from a checked-in config.
3. Config file (`$XDG_CONFIG_HOME/rover/config.toml` or `--config <path>`).
4. Built-in defaults.

`rover config show` prints the merged effective config with a comment indicating where each value came from.

### 4.4 Error model

- Per-module error enums via `thiserror`.
- `anyhow` only at the binary boundary (the `main()` and the CLI subcommand handlers).
- MCP errors returned as structured JSON:

  ```json
  {
    "error": {
      "code": "ssrf_blocked",
      "message": "URL resolves to a private IP (10.0.0.1) which is not allowed under the current SSRF level (strict).",
      "details": { "url": "...", "resolved_ip": "10.0.0.1", "level": "strict" }
    }
  }
  ```

  Codes are stable string identifiers (snake_case). The `details` payload varies per code and is documented in `docs/mcp-tools.md`.

### 4.5 Test strategy

- **Unit tests** colocated with module sources for pure logic (charset detection, URL canonicalization, TextRank, TTL computation).
- **Integration tests** under `tests/`:
  - HTTP behavior against `wiremock` (in-process). Covers redirects, conditional requests, rate limiting, charset, robots.txt.
  - SQLite behavior against an ephemeral DB per test (use `tempfile`).
  - End-to-end MCP transport via spawning `rover mcp` as a subprocess and speaking stdio JSON-RPC. One smoke test, not exhaustive.
- **No live-network tests in CI.** `rover doctor` exists for ad-hoc smoke testing against the real internet.

---

## 5. Module / Crate Layout

Single binary crate `rover` with the following internal modules. Promote to a workspace later if any of these grow significantly or become reusable.

```
crates/
  rover/
    src/
      main.rs                 # CLI entry, clap derive, subcommand dispatch
      config.rs               # TOML loading, layering, `config show`/`set`
      cli/
        fetch.rs              # `rover fetch`
        batch.rs              # `rover batch <id>` (snapshot + --monitor)
        task.rs               # `rover task <id>`
        cache.rs              # `rover cache *`
        doctor.rs             # `rover doctor`
        mcp.rs                # `rover mcp` (entry into MCP server)
      mcp/
        server.rs             # rmcp wiring, transport setup
        tools/
          fetch.rs
          batch_fetch.rs
          summarize.rs
          get_metadata.rs
          count_tokens.rs
        envelope.rs           # task/result envelope types shared by tools
      storage/
        mod.rs                # tokio-rusqlite actor + public API surface
        migrations/           # NNN_description.sql files
        pages.rs
        summaries.rs
        robots.rs
        tasks.rs
        events.rs
        servers.rs            # the new servers table per §2.3
      fetcher/
        client.rs             # reqwest setup, redirect policy
        charset.rs            # the §5.1 pipeline
        ssrf.rs               # policy enforcement
        rate_limit.rs         # per-domain token buckets
        robots.rs             # robotxt integration + cache lookup
        retry.rs              # backoff, retry-after parsing
        headless.rs           # behind `headless` feature
        har.rs                # behind always-on (cheap when disabled)
      extractor/
        pipeline.rs           # bytes -> readabilityrs -> postprocess
        links.rs              # relative -> absolute, srcset, base href
        tables.rs             # transform modes
        images.rs             # transform modes (incl. VLM call site)
        metadata.rs           # JSON-LD, OG, Twitter, microdata
        frontmatter.rs        # YAML envelope writer
      summarizer/
        mod.rs                # SummarizerBackend trait, registry
        extractive.rs         # TextRank
        cloud.rs              # genai wrapper
        local.rs              # mistral.rs wrapper, behind `local-inference`
        prompts.rs            # abstractive prompt templates
      tokenizer/
        mod.rs                # registry, in-memory cache
        openai.rs             # tiktoken-rs
        hf.rs                 # tokenizers (Claude, Llama, Qwen)
        estimate.rs           # heuristic predictions for summary_*
      tasks/
        mod.rs                # task scheduler / claim loop
        batch_fetch.rs        # the batch worker
        revalidate.rs         # SWR worker
        summarize.rs          # summarize worker
        retry.rs              # deferred retries
      doctor/
        mod.rs                # checks, reporting
      vlm/                    # behind `vlm` feature
        mod.rs                # SmolVLM wrapper
```

The `crates/` prefix is forward-looking — initially everything ships under `crates/rover/`. If we ever extract a reusable piece (e.g., `rover-extractor`), the workspace structure is already in place.

---

## 6. Open Items Deferred to writing-plans

These are *implementation-planning* concerns rather than *design* concerns. The implementation plan should resolve them as it sequences the milestones:

- Exact `rmcp` API patterns (depends on the SDK version pinned at planning time).
- Tokio runtime configuration (worker count, blocking pool size). Default to defaults until a profile says otherwise.
- Whether to start with a single `crates/rover/` and split later, or open with a workspace from day one. Lean toward single-crate-first.
- Choice of `jiff` vs `chrono` for time. PRD says prefer `jiff`; planning should confirm `jiff` plays well with `rusqlite` row codecs and `tracing` field rendering.
- Which sentence segmenter to ship in the extractive summarizer (`unicode-segmentation` vs `icu_segmenter`). Lean toward `unicode-segmentation` for binary-size budget.
- Which HTML parser for metadata extraction (`scraper` vs `kuchiki`). Lean toward `scraper` (smaller, more current).
- Concrete CI matrix for the cross-platform target list.
- Release pipeline (cargo-dist? hand-rolled GitHub Actions? cargo-binstall feed?).

---

## 7. Decision Log

| # | Decision | Date |
| --- | --- | --- |
| 1 | tokio-rusqlite for async DB | 2026-05-07 |
| 2 | stdio-only MCP transport | 2026-05-07 |
| 3 | Multi-instance with PID-tagged heartbeats | 2026-05-07 |
| 4 | DNS rebinding protection deferred to v2 | 2026-05-07 |
| 5 | Charset pipeline as PRD §5.1 (no change) | 2026-05-07 |
| 6 | `[summarization] default_backend = "..."` | 2026-05-07 |
| 7 | `summarize` on cache miss → fetch with defaults | 2026-05-07 |
| 8 | Single `output_dir` with `tables/` and `images/` subdirs, absolute paths in frontmatter | 2026-05-07 |

Append new decisions here as they happen. Each entry should also update the relevant section above.

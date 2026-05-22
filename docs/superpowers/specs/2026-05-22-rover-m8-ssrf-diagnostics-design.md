# Rover M8 — SSRF Levels, Diagnostics, Polish — Design

> Status: design complete, awaiting implementation plan.
>
> Prerequisites: M1 (fetcher + Strict SSRF), M2 (cache + storage actor), M3 (MCP server, tokenizer infra), M4 (extracted Markdown + tables/images sidecars), M5 (rate-limited fetcher + robots), M6 (task scheduler + batching), M7 (summarization + summary cache).
>
> Canonical references:
> - PRD §5.5 (SSRF levels), §5.7 (headless — feature-gated; M9), §11 (debug & diagnostics), §12 (configuration), §13 (recommended crate deps), §16 (security).
> - Design supplement §2.4 (DNS rebinding deferred to v2).
> - Milestone manifest §M8 (file layout, open questions, deferrals), §M6 deferral #2 (cross-process new-task notify), §M7 follow-up `TODO(m8)` (per-table summarize parallelization).

---

## 1. Scope and Goals

M8 ships the diagnostics, configuration tooling, and full SSRF level matrix that prior milestones intentionally deferred. It also clears two carry-over items: the M6 cross-process new-task notify channel and the M7 per-table summarization parallelization marker.

**SSRF**
1. Extend `SsrfLevel` from `Strict`-only to the full PRD §5.5 matrix: `Strict`, `Loopback`, `Project`, `Lan`, `None`. Retire the M1 `TestLoopback` variant; tests switch to `Loopback`.
2. `file://` URL support at `Project`-and-above levels with canonicalization + symlink-resolved descendant check against `[ssrf] project_root`.
3. DNS-rebinding protection is **explicitly deferred to v2** per design supplement §2.4. Document the limitation in `docs/security.md` and note `reqwest::ClientBuilder::resolve` as the v2 implementation path. This is a knowing divergence from PRD §5.5's "implementation requirements regardless of level" clause — the divergence is the deliberate scoping decision recorded in the manifest.

**Diagnostics**
4. HAR recorder (PRD §11.1). New `src/fetcher/har.rs` module wired into the fetcher when `[debug] har_path` is set. Body size cap configurable (`[debug] har_body_cap`, default 64 KiB).
5. `rover doctor` (PRD §11.2). New `src/cli/doctor.rs` + `src/doctor/mod.rs` running a battery of checks: SQLite open + WAL + schema version, network reachability, output dir writable, configured cloud backends authenticate. Human + `--format=ndjson` output. Exit 0/1.
6. Secret redaction. New `src/telemetry/redact.rs` adds a `tracing` layer that scrubs URL query-string values for keys matching `api_key|token|secret|password` (case-insensitive substring).

**Configuration tooling**
7. `rover config show` — prints the merged effective config as TOML with per-key provenance comments (`# from: defaults | file | env`). Single merged view.
8. `rover config set <key> <value>` — parses `value` against the destination field's type, writes via `toml_edit` (preserves user comments + ordering), then validates by round-tripping the file through `Config::deserialize`. Refuses to write on parse/validate failure.

**Carry-overs cleared**
9. Cross-process new-task notify (M6 deferral #2). The scheduler registers a SQLite `update_hook` on the `tasks` table; the hook signals the scheduler's existing `Notify`. The 10s polling fallback stays as the cross-machine safety net. Same-host latency drops from ~10 s to <50 ms.
10. Per-table summarization parallelism (M7 `TODO(m8)`). `extractor::tables::apply_with_summarizer` switches the sequential per-table await loop to `futures::stream::iter(...).buffered(N)` with `N = 4`. Output order preserved by `buffered` (vs `buffer_unordered`).

**Docs**
11. Five documentation deliverables authored alongside the code, per PRD §17: `docs/configuration.md`, `docs/cli.md`, `docs/mcp-tools.md`, `docs/security.md`, `docs/backends.md`.

**Acceptance (PRD §14, M8).** `rover doctor` passes on a clean install; HAR files import cleanly into Chrome DevTools.

---

## 2. Decisions Inherited from Open-Question Round

| Question | Decision |
| --- | --- |
| `har` crate version | Pin during planning to latest stable on crates.io. `har` ≥ 0.8 supports HAR 1.2 schema; verify at task time. |
| `rover config show` format | TOML output with `# from: <source>` provenance comments on each scalar key; nested sections grouped under their header. Single merged view (not layered diff). |
| `rover config set` validation | Round-trip through `Config::deserialize` after the `toml_edit` write. On failure, restore original file content (read into memory before write) and return a typed error. |
| `TestLoopback` retirement | Retire. Tests change from `--features test-loopback`-gated `SsrfLevel::TestLoopback` to `SsrfLevel::Loopback`. The `test-loopback` cargo feature is **kept** as a marker for tests that need to bind to wiremock-style loopback servers, but it no longer alters SSRF behavior. This isolates the test-only opt-in from production-shipped levels. |
| Notify mechanism | SQLite `update_hook`. Rusqlite exposes `Connection::update_hook` which fires synchronously on row insert/update/delete. The actor runs the hook on the storage thread and forwards via the existing `Notify`. Portable across Linux/macOS. |
| Per-table parallelism `N` | Constant `N = 4`. Config exposure deferred until a user requests it. |
| `file://` symlink handling | Always resolve symlinks via `std::fs::canonicalize` then verify the resolved absolute path is a descendant of the resolved `project_root`. Reject if not. No way to opt out — PRD §5.5 mandates this for `Project`. |
| Doctor "trivial completion" for backends | Send a single-token prompt (`"ping"`, `target_tokens = 1`) against each configured cloud backend. Skip the call if `api_key_env` resolves to an empty string (so unconfigured-but-defined backends don't false-fail). Extractive backends are exercised by a synthesis check (input → non-empty output). |
| Config provenance source granularity | Per *leaf* (scalar) key. Nested sections (e.g. `[backends.fast]`) carry per-field provenance, not per-section. |
| `--format=ndjson` shape | One JSON object per line, fields `{check, status, detail?}`. `status ∈ {"ok","fail","skip"}`. Doctor exits 0 iff every check status ≠ `"fail"` (skip is allowed). |
| Redaction key list | Hardcoded `["api_key", "token", "secret", "password"]` (case-insensitive substring match). Config exposure deferred. |

---

## 3. Architecture

### 3.1 Module layout

```
src/
  cli/
    config.rs                # NEW: `rover config show`, `rover config set`
    doctor.rs                # NEW: `rover doctor`
  doctor/
    mod.rs                   # NEW: check trait + registry
    checks.rs                # NEW: built-in checks
  fetcher/
    ssrf.rs                  # EXTENDED: Loopback, Project, Lan, None
    har.rs                   # NEW: HAR recorder middleware
    cached.rs                # MODIFIED: routes through HAR recorder when enabled
  config.rs                  # MODIFIED: `[debug]` section + provenance tracking
  config/
    provenance.rs            # NEW: helpers for `config show` (split if config.rs >1100 LOC)
    edit.rs                  # NEW: `config set` via toml_edit
  telemetry/
    redact.rs                # NEW: tracing layer
    mod.rs                   # MODIFIED: install redaction layer in init
  tasks/
    scheduler.rs             # MODIFIED: register sqlite update_hook
  storage/
    mod.rs                   # MODIFIED: expose update_hook registration on the actor
  extractor/
    tables.rs                # MODIFIED: buffered(N) parallelization

tests/
  ssrf_levels.rs             # NEW: matrix coverage per level
  ssrf_project_file.rs       # NEW: file:// at Project level + symlink traversal reject
  har_output.rs              # NEW: HAR file round-trips through Chrome DevTools schema
  cli_doctor.rs              # NEW: doctor checks via spawned subprocess
  cli_config.rs              # NEW: config show / set + toml_edit comment preservation
  redact_logs.rs             # NEW: tracing redaction unit + integration
  cross_process_notify.rs    # NEW: M6 carry-over — second-process insert observed via update_hook
  tables_summarize_parallel.rs # NEW: 8-table fixture, asserts speedup vs sequential baseline

docs/
  configuration.md           # NEW
  cli.md                     # NEW
  mcp-tools.md               # NEW
  security.md                # NEW (incl. DNS rebinding v2 limitation)
  backends.md                # NEW
```

### 3.2 Component boundaries

Each new module has a single responsibility and a small interface:

- **`fetcher::ssrf`** — adds variants to `SsrfLevel` and `validate_addresses`. No new types. Public surface unchanged: callers still pass a level enum into the existing validator.
- **`fetcher::har`** — exposes one type, `HarRecorder`, with `new(path, body_cap)` and `record(request, response)`. The recorder is held by the fetcher's HTTP client wrapper and called once per round-trip. Independent of all other M8 modules.
- **`doctor`** — exposes `Check` trait + `run_all(config, db) -> Vec<CheckReport>`. Built-in checks live in `doctor::checks`. Easy to extend in M9 for feature-gated checks.
- **`config::provenance`** + **`config::edit`** — only consumed by `cli::config`. The rest of the codebase keeps reading `Config` as today.
- **`telemetry::redact`** — one tracing layer struct. Wires in at `telemetry::init`.

### 3.3 Data flow — `rover config show`

```
load config file path
    └─> read file bytes
    └─> deserialize into Config (full validation)
    └─> walk Config + the raw toml::Value to produce a Vec<ProvenanceRow>
            (each row: dotted key, value, source)
    └─> render as TOML with `# from: <source>` comment on each scalar
    └─> print to stdout
```

Source order, lowest to highest:
1. Defaults (struct `Default` impls)
2. File (`$XDG_CONFIG_HOME/rover/config.toml` or `--config <path>`)
3. Env vars (e.g. `ROVER_OUTPUT_DIR`, `ROVER_LOG_LEVEL`)
4. CLI flags — **not shown** by `config show`; `show` reflects persisted state, not ephemeral overrides

### 3.4 Data flow — `rover config set`

```
parse args: <dotted.key> <value>
    └─> read file into String (preserve as `original`)
    └─> resolve dotted key into a typed target (use a small reflection over Config schema;
        the typed schema is the source of truth)
    └─> parse `value` against the target type (bool, integer, string, enum, list)
    └─> open file with toml_edit::Document
    └─> mutate the target key — auto-create missing parent tables
    └─> serialize back to TOML (preserves user comments + key ordering)
    └─> ATTEMPT validation: round-trip through Config::deserialize
        ├─> on Ok: write to disk
        └─> on Err: discard, return ConfigError::SetValidation { key, value, source }
```

The reflection-over-Config-schema is the trickiest piece. For v1 the spec accepts a **whitelist of settable keys** in a `static SETTABLE_KEYS: &[SettableKey]` table — about 25 keys covering everything from `ssrf.level` to `cache.default_ttl`. Each entry carries its parser. Generic full-schema reflection is out of scope (would require either schemars-based introspection or proc-macros). The whitelist is sufficient for M8 and easy to extend in M9+.

### 3.5 Cross-process notify

The storage actor (`tokio-rusqlite` `Connection`) is extended at open time to register an `update_hook` callback. When any row is inserted into the `tasks` table, the hook calls a clonable `Notify` (held in the scheduler) to wake it. The scheduler's existing 10-second poll loop becomes the slow path — it still runs to catch cross-machine inserts (NFS-backed DB, separate hosts), but `update_hook` is now the primary same-host path.

`update_hook` runs synchronously on the SQLite thread, so the callback **must not block**. It only calls `Notify::notify_one()` (lock-free).

### 3.6 Per-table parallelization

`apply_with_summarizer` currently drains a `Vec<OwnedEvent>` sequentially, awaiting each table hook. Replace the drain loop with:

```rust
use futures::stream::{self, StreamExt};

let results = stream::iter(events.into_iter().enumerate())
    .map(|(idx, ev)| async move {
        match ev {
            OwnedEvent::Line(s) => (idx, OutSlot::Line(s)),
            OwnedEvent::Table(rows, ord) => {
                let table_text = rows.join("\n");
                let r = hook(&table_text).await;
                (idx, OutSlot::Table { rows, ord, result: r })
            }
        }
    })
    .buffered(4)
    .collect::<Vec<_>>()
    .await;
// Re-sort by `idx` (buffered preserves order, but explicit is robust).
```

`buffered(N)` returns futures in input order, so the resulting `Vec` matches document order without an explicit sort — but the sort is cheap insurance against future refactors. Concurrency bounded to 4 to avoid hammering cloud backends.

---

## 4. Schema and Config

### 4.1 Schema

No new migrations. M8 is feature work over existing tables.

### 4.2 Config additions

```toml
[ssrf]
level = "strict"          # was: implicit. Now: "strict" | "loopback" | "project" | "lan" | "none"
project_root = "."        # used when level = "project". Resolved relative to the config file.

[debug]
har_path = ""             # empty = disabled
har_body_cap = "64KiB"    # parsed by humantime-like crate or hand-rolled
log_level = "info"        # mirrors existing env override
```

Both `[ssrf]` and `[debug]` are new sections. Today's `SsrfLevel` is constructed in code without a config path; M8 introduces the section and threads it through to the fetcher. Robots config stays under `[robots]` — no conflict.

`[debug] har_body_cap` defaults to 64 KiB. The HAR spec doesn't mandate a cap, but full bodies for large pages would balloon the file. Truncated bodies are marked with HAR's standard `comment` field.

### 4.3 SSRF level semantics

| Level | Allows |
| --- | --- |
| `strict` | Public IPs only; `http`/`https` only. (Current M1 behavior.) |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs whose canonicalized path is a descendant of `[ssrf] project_root` (also canonicalized). |
| `lan` | Project + RFC1918 (`10/8`, `172.16/12`, `192.168/16`) + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. Log a `WARN` line at startup. Document the risks in `security.md`. |

**Always blocked, every level:** link-local (`169.254.0.0/16`, `fe80::/10`), multicast, broadcast, `0.0.0.0`, `255.255.255.255`.

`file://` scheme accepted only when level ≥ `Project`. For higher levels (`Lan`, `None`) the same canonicalize+descendant check applies — `Lan` doesn't widen file paths, only IP ranges.

---

## 5. SSRF Implementation Notes

The current `validate_addresses` walks the address list and accepts or rejects each per the level. M8 keeps that shape — new variants are added to the `match` arms.

`Loopback` is the simplest extension: accept `127.0.0.0/8` and `::1` in addition to `Strict`'s public-only rule.

`Project` adds the `file://` scheme branch in `validate_url` (which currently only allows `http`/`https`). Path canonicalization uses `std::fs::canonicalize` — this follows symlinks. The resolved path is compared via `Path::starts_with` against the canonicalized `project_root`. If `project_root` is missing or unset and `level == Project`, return `SsrfError::ProjectRootMissing` at startup (not at fetch time).

`Lan` adds the RFC1918 and ULA ranges. `IpAddr::is_private` covers RFC1918 for IPv4; for IPv6 ULA we check the `fc00::/7` mask explicitly (Rust std doesn't expose an `is_unique_local` on stable).

`None` short-circuits the validator to `Ok(())` after the always-blocked check. (Always-blocked addresses are blocked regardless — even at `None`. This is the safety floor.)

The M1 `TestLoopback` variant is removed. Tests previously using it are updated to construct `SsrfLevel::Loopback` directly. The `test-loopback` cargo feature stays in `Cargo.toml` as a marker that gates wiremock setup helpers; it no longer affects the SSRF enum.

---

## 6. HAR Recorder

`har` crate's types map directly onto reqwest's request/response — no hand-rolled schema. The recorder owns a `Mutex<har::Har>` accumulator and an opt-in `tokio::sync::Mutex<File>` for incremental flushing.

**Lifecycle:**
- At server/CLI startup: if `[debug] har_path` is set and non-empty, instantiate a `HarRecorder` and register it with the fetcher.
- Per fetch: after the response body is read, append an `har::v1_2::Entries` entry. Bodies > `har_body_cap` are truncated and tagged with HAR's `comment` field.
- At shutdown: flush. For long-running MCP server use, flush every N seconds (default 5) on a background interval.

**What's recorded:** request URL/method/headers/body, response status/headers/body (capped), timings (DNS, connect, send, wait, receive — best-effort; reqwest doesn't expose all phases).

**What's NOT recorded:** internal cache hits (HAR represents network round-trips; cached pages would mislead). A separate log line at INFO level marks the cache hit.

**Open question for planning:** the existing fetcher in `src/fetcher/cached.rs` wraps `reqwest::Client::execute`. Decide whether the recorder hooks via (a) a reqwest middleware (`reqwest-middleware` crate) or (b) a thin manual wrapper around the existing fetch function. Recommendation: (b) — fewer deps, the call sites are already centralized.

---

## 7. `rover doctor`

### 7.1 Check trait

```rust
pub trait Check: Send {
    fn name(&self) -> &'static str;
    fn run(&self, ctx: &CheckCtx) -> CheckReport;
}

pub struct CheckCtx { pub config: Arc<Config>, pub db: Db }

pub struct CheckReport { pub check: &'static str, pub status: CheckStatus, pub detail: Option<String> }

pub enum CheckStatus { Ok, Fail, Skip }
```

### 7.2 Built-in checks

1. **`sqlite_open`** — `Db::open()` succeeds. Detail: db path.
2. **`sqlite_wal_mode`** — `PRAGMA journal_mode` returns `wal`.
3. **`sqlite_schema_version`** — current schema version matches the latest migration.
4. **`network_reachable`** — `HEAD https://example.com` returns 2xx within 5 s. Skipped if `[debug] offline = true` (future flag, accept-no-op for M8).
5. **`output_dir_writable`** — touch + delete a probe file in resolved output dir.
6. **`backends_authenticate`** — for each cloud backend with a non-empty `api_key_env`, run a `target_tokens=1` summarization against a literal string. Skip if `api_key_env` resolves empty.
7. **`extractive_synthesis`** — synthesize a one-paragraph input through the extractive backend; assert non-empty output.

### 7.3 Output

Default: human-readable, one line per check with green/red marker. `--format=ndjson`: one `CheckReport` per line.

Exit code: 0 iff no `Fail`. `Skip` is allowed.

```text
$ rover doctor
✓ sqlite_open
✓ sqlite_wal_mode
✓ sqlite_schema_version
✓ network_reachable
✓ output_dir_writable
- backends_authenticate (skipped: no configured cloud backends)
✓ extractive_synthesis
all checks ok
```

---

## 8. `rover config`

### 8.1 `show`

```text
$ rover config show
# rover effective configuration
# defaults | file (~/.config/rover/config.toml) | env

[server]
data_dir = "~/.local/share/rover"          # from: defaults
output_dir = "./rover-output"              # from: file

[ssrf]
level = "loopback"                         # from: file
project_root = "."                         # from: defaults

[debug]
har_path = ""                              # from: defaults
log_level = "info"                         # from: env ROVER_LOG_LEVEL
```

### 8.2 `set`

```text
$ rover config set ssrf.level loopback
✓ ssrf.level = "loopback"  (wrote ~/.config/rover/config.toml)
```

```text
$ rover config set ssrf.level bogus
error: invalid value for ssrf.level
  expected one of: strict, loopback, project, lan, none
  got: bogus
file unchanged
```

### 8.3 Settable-key whitelist

A `static SETTABLE: &[SettableSpec]` registers ~25 keys. Each entry carries:
- Dotted key (`"ssrf.level"`)
- Parser fn (`fn(&str) -> Result<toml::Value, SetError>`)
- Optional enum-of-valid-values (for nicer error messages)

Adding a settable key in M9+ is a one-line addition.

Out of scope for M8: setting list-valued keys (e.g. `robots.ignore_domains`). The error message tells the user to edit the file directly.

### 8.4 Comment preservation

`toml_edit::Document` is used end-to-end. The write path NEVER reaches for `toml::ser`. Validation after write reads the file fresh through `Config::deserialize` (which uses `toml::de`).

---

## 9. Secret Redaction

A `tracing_subscriber::Layer` that intercepts every `tracing::Event`, walks its fields, and substitutes URL query values for redaction-keyed parameters.

**Algorithm per field value (cheap, allocation-free path):**
1. If the value contains neither `=` nor `?`, pass through unchanged.
2. Otherwise, attempt to parse as a URL. If parse fails, pass through (don't pretend to handle non-URL strings).
3. Walk `query_pairs()`. For each pair where the key lowercased contains one of the trigger substrings (`api_key`, `token`, `secret`, `password`), replace the value with `"<redacted>"`.
4. Re-serialize the URL.

Keys not in the trigger list flow through unchanged.

**Performance:** the parse only fires on values containing `=` or `?`. URL parsing of a malformed string is cheap (returns `Err` fast). Real-world impact is bounded.

**What's NOT redacted:** authorization headers, response bodies, env-var values. The layer is scoped to event field values — usually URLs.

---

## 10. Cross-Process Notify (M6 carry-over)

The current scheduler polls `tasks` every 10 s for newly-eligible rows. This is fine same-machine but slow for the SWR revalidate latency target (PRD §9.1).

`tokio-rusqlite`'s `Connection` doesn't directly expose `update_hook` because the hook fires synchronously on the SQLite thread inside the actor's `call` closures. The fix:

1. Open a second, dedicated connection in read-only mode at scheduler startup whose sole job is to host the `update_hook`. The hook captures a `tokio::sync::Notify::Arc` clone and calls `notify_one()` on `Insert`/`Update` for table `tasks`.
2. The scheduler's run-loop selects on `notify.notified()` OR `tokio::time::sleep(POLL_INTERVAL)`. Either path wakes it.

`POLL_INTERVAL` stays at 10 s; the notify path becomes the fast path.

**Bound on hook work:** `Notify::notify_one` is lock-free (atomic CAS). The hook does no I/O, no allocation. Safe to call synchronously from the SQLite thread.

**Concurrency:** spurious wakeups are fine — the scheduler re-queries on every wake, and the storage actor already serializes reads.

---

## 11. Test Strategy

### 11.1 New integration tests

| Test | Asserts |
| --- | --- |
| `ssrf_levels::strict_blocks_loopback` | `127.0.0.1` rejected at `Strict`. |
| `ssrf_levels::loopback_allows_loopback` | `127.0.0.1` accepted at `Loopback`. |
| `ssrf_levels::project_rejects_lan_addr` | RFC1918 rejected at `Project`. |
| `ssrf_levels::lan_allows_rfc1918` | `10.0.0.1` accepted at `Lan`. |
| `ssrf_levels::none_allows_arbitrary_ip` | Random public IP accepted at `None`. |
| `ssrf_levels::none_still_blocks_zero_address` | `0.0.0.0` rejected at every level. |
| `ssrf_project_file::file_inside_root_ok` | `file:///<root>/x.txt` resolves, fetch succeeds. |
| `ssrf_project_file::file_outside_root_rejected` | `file:///etc/passwd` rejected. |
| `ssrf_project_file::symlink_traversal_rejected` | Symlink from inside root → outside root: rejected after canonicalization. |
| `har_output::har_file_imports_into_devtools_schema` | Generate a HAR, parse it back via `har` crate, assert at least one `Entries` entry, mandatory fields present. |
| `cli_doctor::clean_install_exits_zero` | Spawn `rover doctor` against a fresh data dir, no config; expect exit 0. |
| `cli_doctor::missing_wal_exits_one` | Open DB without WAL, run doctor, expect exit 1. |
| `cli_doctor::ndjson_format_one_json_per_line` | `--format=ndjson` output: every line parses as `CheckReport`. |
| `cli_config::show_marks_provenance_correctly` | Write a partial file, run `show`, assert `# from: file` on overridden keys and `# from: defaults` on rest. |
| `cli_config::set_preserves_comments` | Pre-write a file with comments, `config set` a key, assert comments still present. |
| `cli_config::set_validates_enum` | `config set ssrf.level bogus` returns nonzero exit, file unchanged. |
| `redact_logs::query_param_redacted` | Emit `tracing::info!(url = "https://x/?api_key=AKIA")`; captured log contains `api_key=<redacted>`. |
| `redact_logs::unrelated_param_passes` | `?page=2` not touched. |
| `cross_process_notify::insert_observed_within_100ms` | Spawn two scheduler instances against the same DB; insert a task in one, assert the other wakes within 100 ms. |
| `tables_summarize_parallel::8_tables_speedup` | 8-table fixture with 100 ms mocked hook. Sequential baseline ~800 ms; parallel `buffered(4)` ~250 ms. Assert wall-clock < 400 ms. |

### 11.2 Existing tests touched

- `tests/fetcher_*.rs` — switch `SsrfLevel::TestLoopback` → `SsrfLevel::Loopback` (~6 files).
- `tests/tables_summarize_mode.rs` — verify still passes (parallelization preserves output content + order).

### 11.3 Doctor's network check in tests

`cli_doctor::clean_install_exits_zero` may fail in offline CI. Gate the `network_reachable` check behind a `[debug] offline = true` toggle, default `false`, that the test sets before invoking. Sandbox-friendly.

---

## 12. Crate Dependencies Added

| Crate | Why | Notes |
| --- | --- | --- |
| `har` | HAR file format | Pin at planning. ≥ 0.8 for HAR 1.2 schema. |
| `toml_edit` | `config set` comment-preserving writes | Already transitively present? Verify; if not, add. |
| `futures` | `stream::buffered` | Already in tree via reqwest's stack. Verify direct dep. |

No new deps for redaction, doctor, notify — all are stdlib + existing deps.

---

## 13. Error Model

New variants on `ConfigError`:

```rust
#[error("invalid value for {key}: expected {expected}, got {value}")]
SetParse { key: String, value: String, expected: String },

#[error("validation failed after writing {key} = {value}: {source}")]
SetValidation { key: String, value: String, source: Box<ConfigError> },

#[error("key `{key}` is not settable via `rover config set`; edit the file directly")]
UnsettableKey { key: String },

#[error("project_root is required when ssrf.level = project")]
ProjectRootMissing,
```

New variants on `SsrfError`:

```rust
#[error("file:// URLs are not allowed at level {level:?}")]
FileSchemeNotAllowed { level: SsrfLevel },

#[error("file path {path} is not a descendant of project_root {root}")]
FileOutsideProjectRoot { path: PathBuf, root: PathBuf },

#[error("file path {path} could not be canonicalized: {source}")]
FileCanonicalize { path: PathBuf, source: std::io::Error },
```

New top-level `DoctorError` enum. New module-level `HarError`.

All errors map through the standard `RoverError` codes added in M3+M7 — new stable codes:

```
config_set_parse
config_set_validation
config_set_unsettable
ssrf_file_not_allowed
ssrf_file_outside_root
ssrf_project_root_missing
har_write_failed
```

---

## 14. Documentation Deliverables (PRD §17)

| File | Contents |
| --- | --- |
| `docs/configuration.md` | Full `[section]` reference. Every key, type, default, valid range. Examples per section. |
| `docs/cli.md` | Every subcommand with synopsis, args, flags, examples. Includes `rover doctor` and `rover config`. |
| `docs/mcp-tools.md` | Every MCP tool: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens`. Args, response shape, examples. |
| `docs/security.md` | SSRF level matrix table with full semantics. DNS rebinding v2 limitation (manifest §2.4 + `reqwest::ClientBuilder::resolve` v2 path). Cache poisoning consideration (PRD §16). `file://` symlink resolution. Secret redaction key list. |
| `docs/backends.md` | Backend kinds (`extractive`, `cloud`), provider list (genai-supported + `openai_compat`), `[backends.<name>]` field reference, examples for OpenAI / Anthropic / LM Studio / Ollama. |

All five authored alongside the code in this milestone.

---

## 15. Acceptance Criteria

1. ✅ `rover doctor` on a fresh install (no backends, default config) exits 0. Tested by `cli_doctor::clean_install_exits_zero`.
2. ✅ A HAR file generated by setting `[debug] har_path` round-trips through the `har` crate's parser with at least one `Entries` entry containing the expected fields. Tested by `har_output::har_file_imports_into_devtools_schema`. (Direct Chrome DevTools verification is a manual smoke during planning, not an automated test.)
3. ✅ `rover config set ssrf.level loopback` mutates the on-disk file and preserves any user comments. A subsequent `rover config show` reports `# from: file` on that key. Tested by `cli_config::set_preserves_comments` + `cli_config::show_marks_provenance_correctly`.
4. ✅ `rover config set ssrf.level bogus` returns non-zero exit. File contents unchanged. Tested by `cli_config::set_validates_enum`.
5. ✅ Each SSRF level enforces its allow/deny matrix. Tested by the `ssrf_levels::*` suite.
6. ✅ `file://` outside `project_root` rejected even when reached via symlink. Tested by `ssrf_project_file::symlink_traversal_rejected`.
7. ✅ A URL with `?api_key=...` query is redacted in tracing output. Tested by `redact_logs::query_param_redacted`.
8. ✅ A task inserted from a second process is observed by a running scheduler within 100 ms. Tested by `cross_process_notify::insert_observed_within_100ms`.
9. ✅ Per-table summarization of an 8-table fixture completes in <400 ms with a 100 ms mocked hook (vs ~800 ms sequential). Tested by `tables_summarize_parallel::8_tables_speedup`.
10. ✅ All pre-M8 tests pass. The `--features test-loopback` cargo feature is still recognized (kept as a marker) but no longer alters SSRF behavior. Spot-check: M5 robots tests, M6 task lifecycle tests, M7 summarization tests.
11. ✅ Five docs files exist with the contents listed in §14. Lint-level check: each file has the expected top-level `##` sections. Manual review on PR.

---

## 16. Open Items Deferred to Writing-Plans

These don't change the design but need concrete answers when the plan is written:

1. **`har` crate version pin.** Confirm latest stable on crates.io at planning time. Verify it exposes HAR 1.2 (the format Chrome DevTools imports).
2. **HAR middleware mechanism.** Recommendation in §6: a manual wrapper around the existing `fetcher::cached::fetch_with_cache` rather than `reqwest-middleware`. Confirm during plan.
3. **Settable-key whitelist contents.** Enumerate the exact 25-ish keys for M8. Suggested starter list: `ssrf.level`, `ssrf.project_root`, `fetch.user_agent`, `fetch.timeout`, `fetch.max_redirects`, `cache.default_ttl`, `cache.min_ttl`, `cache.max_ttl`, `cache.store_raw_html`, `robots.respect`, `robots.default_ttl`, `rate_limit.requests_per_minute_per_domain`, `rate_limit.per_domain_concurrency`, `rate_limit.global_concurrency`, `tokenizer.default`, `output.dir`, `summarization.default_backend`, `summarization.default_mode`, `summarization.default_style`, `summarization.fallback_to_extractive`, `summarization.tables.target_tokens`, `summarization.tables.focus`, `debug.har_path`, `debug.har_body_cap`, `debug.log_level`. Finalize during plan.
4. **`update_hook` connection management.** Confirm `rusqlite::Connection::update_hook` semantics: does the hook persist across statement resets? Does it fire on `INSERT OR REPLACE`? Verify with a smoke test before writing the production code.
5. **Provenance tracking for `[backends.<name>]` blocks.** Backend definitions are user-defined; provenance is always `file`. Confirm `show` handles missing default cleanly (no `# from: defaults` synthesized for user-defined sections).
6. **Doctor: scrubbed output paths.** Detail messages must not leak full filesystem paths in the default human format (only relative or `~/` form). NDJSON format keeps absolute paths for scripting clients.
7. **`futures::stream::buffered` order guarantee.** Re-confirm before writing the table parallelization that `buffered(N)` preserves input order; the explicit re-sort in §3.6 is the safety net.

---

## 17. Decision Log

| Date | Decision | Rationale |
| --- | --- | --- |
| 2026-05-22 | DNS rebinding deferred to v2 | Aligns with manifest §2.4; PRD §5.5 mandate noted as knowing divergence. `reqwest::ClientBuilder::resolve` is the v2 path. |
| 2026-05-22 | Retire `TestLoopback` variant | Once `Loopback` is a real level, having two enum variants for "allow loopback" is a maintenance hazard. The `test-loopback` cargo feature stays as a marker. |
| 2026-05-22 | `config set` uses whitelisted keys | Full schema reflection would require macros or schemars introspection. A whitelist is sufficient for M8 and extends in one-line PRs. |
| 2026-05-22 | Single merged `config show` (not layered) | PRD §12 calls for "merged effective config (file + defaults)". Layered diff view is nice-to-have for M9+. |
| 2026-05-22 | SQLite `update_hook` for cross-process notify | Portable (Linux/macOS), in-tree (rusqlite already a dep), no extra IPC primitives. |
| 2026-05-22 | Per-table parallel `N = 4` constant | Bounded to avoid hammering cloud APIs; config exposure deferred until a user asks. |
| 2026-05-22 | Doctor skip semantics | `Skip` is non-failing. Allows offline / unconfigured backends to not false-fail the check. |
| 2026-05-22 | Redaction key list hardcoded | Stable set covers the obvious cases; expanding to user-config is M9+ work and risks bypass via custom param naming. |

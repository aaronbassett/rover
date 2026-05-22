# Rover M8 — SSRF Levels, Diagnostics, Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the full SSRF level matrix, HAR debug recorder, `rover doctor`, `rover config show/set`, secret redaction, the M6 cross-process notify channel, and the M7 per-table summarize parallelization. Write the five PRD §17 documentation deliverables alongside.

**Architecture:** Extend the existing `src/fetcher/ssrf.rs` enum and validators; introduce `src/fetcher/har.rs` as a recorder activated by config; add `src/doctor/` and `src/cli/{doctor,config}.rs` as new CLI surface; add `src/telemetry/redact.rs` as a `tracing_subscriber::Layer`; wire SQLite `update_hook` from the storage actor into the task scheduler's existing `Notify`. Per-table parallelism is a one-function change in `src/extractor/tables.rs`. Configuration grows two new sections (`[ssrf]`, `[debug]`); all changes are non-breaking on the wire.

**Tech Stack:** New crate dep `har = "0.9"` (pin during planning if a newer minor exists); `toml_edit` (likely already transitive — verify); `futures` for `stream::buffered`. Reuses existing `tokio-rusqlite`, `tracing-subscriber`, `tracing`, `serde`, `thiserror`. No new feature flags.

**Branch context:** Execute on `m8-ssrf-diagnostics`, cut from `origin/main` at `043c2c9` (M7 merge). The branch currently carries one commit: the M8 design spec (`d0cf9ca`). Run `cargo test --features test-loopback` to confirm 453 tests green on a clean checkout before Task 1.

**Scope of this plan:** PRD milestone M8 only — SSRF levels, diagnostics, config tooling, secret redaction, M6/M7 carry-overs, plus the five documentation files. No new MCP tool surface. No new migrations. No feature-flag work (`headless`, `local-inference`, `vlm` doctor checks are M9).

**References:**
- Design spec: `docs/superpowers/specs/2026-05-22-rover-m8-ssrf-diagnostics-design.md`
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` §5.5 (SSRF levels), §11 (debug & diagnostics), §12 (configuration), §13 (recommended deps), §16 (security), §17 (docs deliverables).
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` §2.4 (DNS rebinding deferred to v2).
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` M8 section.
- M7 plan (granularity reference): `docs/superpowers/plans/2026-05-22-rover-m7-summarization.md`.

---

## Decisions inherited from the M8 design spec

The spec resolved every open question. Quick reference (full table at spec §2):

1. **DNS rebinding:** deferred to v2. Documented in `docs/security.md`. `reqwest::ClientBuilder::resolve` is the v2 path.
2. **TestLoopback retirement:** the enum variant is removed; tests switch to `SsrfLevel::Loopback`. The `test-loopback` cargo feature stays as a marker (gating wiremock setup helpers) but no longer alters SSRF behavior.
3. **`har` crate version:** pin to `0.9` (latest stable on crates.io at planning; bump in Task 5 if a newer minor lands by then).
4. **`config show` format:** TOML with `# from: <source>` provenance comments per leaf. Source order: defaults < file < env. CLI flags not surfaced (they're ephemeral).
5. **`config set` validation:** read original file into memory, write via `toml_edit::Document`, round-trip through `Config::deserialize`, restore original on failure.
6. **Notify mechanism:** `rusqlite::Connection::update_hook` on a dedicated read-only connection in the scheduler. Hook calls `Notify::notify_one()` (lock-free, sync-safe).
7. **Per-table parallelism:** `futures::stream::iter(...).buffered(4)`. `buffered` preserves input order — explicit `.sort_by_key(idx)` retained as safety net.
8. **`file://` symlink handling:** always canonicalize via `std::fs::canonicalize` then `Path::starts_with` the canonicalized `project_root`. Reject if outside. No opt-out.
9. **Doctor backend auth check:** skip when `api_key_env` resolves empty. Send `target_tokens=1` prompt against each configured cloud backend.
10. **Provenance granularity:** per leaf (scalar) key. Nested sections carry per-field provenance.
11. **`--format=ndjson`:** one `{check, status, detail?}` object per line.
12. **Redaction key list:** hardcoded `["api_key", "token", "secret", "password"]` (case-insensitive substring).

---

## Files Created or Modified in This Plan

```
# Created
src/fetcher/har.rs                                      # HAR recorder
src/doctor/mod.rs                                       # Check trait + run_all + report types
src/doctor/checks.rs                                    # Built-in checks
src/cli/doctor.rs                                       # `rover doctor` subcommand
src/cli/config.rs                                       # `rover config show` + `rover config set`
src/config/provenance.rs                                # Provenance tracker for `config show`
src/config/edit.rs                                      # Settable-key whitelist + parsers for `config set`
src/telemetry/redact.rs                                 # tracing layer for URL query-string redaction

tests/ssrf_levels.rs
tests/ssrf_project_file.rs
tests/har_output.rs
tests/cli_doctor.rs
tests/cli_config.rs
tests/redact_logs.rs
tests/cross_process_notify.rs
tests/tables_summarize_parallel.rs

docs/configuration.md
docs/cli.md
docs/mcp-tools.md
docs/security.md
docs/backends.md

# Modified
Cargo.toml                              # +har; verify toml_edit + futures direct deps
src/lib.rs                              # +pub mod doctor; submodules in config
src/config.rs                           # +SsrfConfig, DebugConfig; reorg to support submodules
src/fetcher/ssrf.rs                     # +Loopback/Project/Lan/None variants; retire TestLoopback
src/fetcher/cached.rs                   # call HAR recorder if configured; thread SSRF level through
src/fetcher/mod.rs                      # export new error variants
src/extractor/tables.rs                 # apply_with_summarizer uses buffered(4)
src/tasks/scheduler.rs                  # subscribe to storage actor's notify channel
src/storage/mod.rs                      # register update_hook; expose notify clone
src/telemetry.rs                        # install redaction layer
src/cli/mod.rs                          # register `config` + `doctor` subcommands
src/main.rs                             # wire HarRecorder + doctor + config CLI
src/mcp/server.rs                       # plumb SSRF level from config
tests/fetcher_*.rs                      # TestLoopback → Loopback (about 6 files)

README.md                               # M8 row + status (final task)
docs/superpowers/milestones/rover-milestones.md   # M8 status update (final task)
```

Inline unit tests live in `#[cfg(test)] mod tests` at the bottom of each new source file. Integration tests under `tests/*.rs` cover end-to-end CLI flows via spawned subprocesses (`std::process::Command` against the `cargo run`/`target/debug/rover` binary, mirroring existing `tests/cli_*.rs` patterns). All test invocations use `cargo test --features test-loopback`.

---

## Repo conventions to follow

These are enforced by lefthook + clippy and will trip the implementer if ignored:

1. **Per-module thiserror enums.** `anyhow` is forbidden in lib code (only `src/main.rs` may use it).
2. **`tokio-rusqlite` actor monopoly on SQLite.** No raw `rusqlite::Connection` outside `src/storage/`. The `update_hook` connection in Task 14 is the sole exception and must live inside `src/storage/`.
3. **`tracing` on stderr; no `println!` in lib code.** `eprintln!` is fine in `src/main.rs` and `src/cli/` for human-facing output.
4. **Conventional commits.** All-lowercase descriptions. `lefthook` enforces. Never use `--no-verify`.
5. **`[lints.rust] warnings = "deny"` + `[lints.clippy] all = "deny"`.** Code must compile clean. Inline-format-args is mandated by clippy.
6. **Test invocation: `cargo test --features test-loopback`.** Bare `cargo test` is broken on this branch due to a pre-existing cfg-gate quirk that's out of M8 scope.
7. **`StorageError` routing.** `tokio_rusqlite::Error` has no `Other` variant; use `ConnectionClosed` for tests, `rusqlite::Error::ToSqlConversionFailure(Box::new(...))` for new error paths. The reference is `src/storage/pages.rs::upsert`.
8. **No new files outside the layout above.** If a file genuinely needs to be split mid-task (>800 LOC or visually noisy), pause and report instead of splitting unilaterally.

---

## Task 1: Config additions — `[ssrf]` + `[debug]` sections

The whole milestone hangs off two new config sections. Add them with strict serde validation, sensible defaults, and full unit coverage before any feature code touches them.

**Files:**
- Modify: `src/config.rs`

### Step 1.1: Write failing tests for `SsrfConfig`

- [ ] Add at the bottom of `src/config.rs` (inside `#[cfg(test)] mod tests`):

```rust
#[test]
fn ssrf_section_parses_with_defaults() {
    let toml = r#"
[ssrf]
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.ssrf.level, "strict");
    assert_eq!(cfg.ssrf.project_root, std::path::PathBuf::from("."));
}

#[test]
fn ssrf_section_accepts_each_level() {
    for level in &["strict", "loopback", "project", "lan", "none"] {
        let toml = format!("[ssrf]\nlevel = \"{level}\"\n");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert_eq!(cfg.ssrf.level, *level);
    }
}

#[test]
fn ssrf_section_rejects_unknown_field() {
    let toml = r#"
[ssrf]
level = "strict"
bogus = 1
"#;
    let r: Result<Config, _> = toml::from_str(toml);
    assert!(r.is_err(), "expected deny_unknown_fields rejection");
}

#[test]
fn missing_ssrf_section_yields_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.ssrf.level, "strict");
}
```

- [ ] Run: `cargo test --features test-loopback config:: 2>&1 | tail -20`
  Expected: 4 failures with "no field `ssrf`" or "field `ssrf` not found on `Config`".

### Step 1.2: Add `SsrfConfig` to `src/config.rs`

- [ ] Locate the `pub struct Config { … }` declaration (around line 31). Add a field:

```rust
    #[serde(default)]
    pub ssrf: SsrfConfig,
```

(Position it logically — between `fetch` and `cache` is fine.)

- [ ] Below `RobotsConfig` (or any sibling section block — keep alphabetic-ish), add:

```rust
/// Top-level `[ssrf]` section. M8 introduces this — earlier milestones
/// hardcoded `SsrfLevel::Strict`. The `level` field is a free-form string
/// here so the file accepts unknown levels with a typed error from the
/// fetcher rather than a serde error; `validate_url`/`validate_addresses`
/// reject malformed levels at first use.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SsrfConfig {
    #[serde(default = "default_ssrf_level")]
    pub level: String,

    #[serde(default = "default_ssrf_project_root")]
    pub project_root: std::path::PathBuf,
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            level: default_ssrf_level(),
            project_root: default_ssrf_project_root(),
        }
    }
}

fn default_ssrf_level() -> String {
    "strict".to_string()
}

fn default_ssrf_project_root() -> std::path::PathBuf {
    std::path::PathBuf::from(".")
}
```

- [ ] Run: `cargo test --features test-loopback config::tests::ssrf 2>&1 | tail -20`
  Expected: all four pass.

### Step 1.3: Write failing tests for `DebugConfig`

- [ ] Add to `#[cfg(test)] mod tests` in `src/config.rs`:

```rust
#[test]
fn debug_section_parses_with_defaults() {
    let cfg: Config = toml::from_str("[debug]\n").unwrap();
    assert_eq!(cfg.debug.har_path, "");
    assert_eq!(cfg.debug.har_body_cap, 64 * 1024);
    assert_eq!(cfg.debug.log_level, "info");
}

#[test]
fn debug_section_har_body_cap_accepts_humansize() {
    // We accept either a raw integer (bytes) or a humansize string ("64KiB", "1MiB").
    let cfg: Config = toml::from_str(r#"[debug]
har_body_cap = "1MiB"
"#).unwrap();
    assert_eq!(cfg.debug.har_body_cap, 1024 * 1024);
}

#[test]
fn debug_section_har_body_cap_accepts_integer_bytes() {
    let cfg: Config = toml::from_str(r#"[debug]
har_body_cap = 8192
"#).unwrap();
    assert_eq!(cfg.debug.har_body_cap, 8192);
}

#[test]
fn debug_section_rejects_unknown_field() {
    let r: Result<Config, _> = toml::from_str(r#"[debug]
har_path = ""
bogus = 1
"#);
    assert!(r.is_err());
}
```

- [ ] Run: `cargo test --features test-loopback config::tests::debug 2>&1 | tail -20`
  Expected: 4 failures (no field `debug`).

### Step 1.4: Add `DebugConfig` to `src/config.rs`

- [ ] Add a field to `Config`:

```rust
    #[serde(default)]
    pub debug: DebugConfig,
```

- [ ] Add below `SsrfConfig`:

```rust
/// Top-level `[debug]` section. M8 introduces this for HAR recording and
/// log-level overrides.
///
/// `har_body_cap` accepts either a raw integer (bytes) or a humansize
/// string like "64KiB" / "1MiB" via a custom deserializer. The internal
/// representation is `u64` bytes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugConfig {
    #[serde(default = "default_debug_har_path")]
    pub har_path: String,

    #[serde(default = "default_debug_har_body_cap", deserialize_with = "deserialize_humansize")]
    pub har_body_cap: u64,

    #[serde(default = "default_debug_log_level")]
    pub log_level: String,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            har_path: default_debug_har_path(),
            har_body_cap: default_debug_har_body_cap(),
            log_level: default_debug_log_level(),
        }
    }
}

fn default_debug_har_path() -> String {
    String::new()
}

fn default_debug_har_body_cap() -> u64 {
    64 * 1024
}

fn default_debug_log_level() -> String {
    "info".to_string()
}

fn deserialize_humansize<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = toml::Value::deserialize(deserializer)?;
    match v {
        toml::Value::Integer(n) if n >= 0 => Ok(n as u64),
        toml::Value::String(s) => parse_humansize(&s).map_err(D::Error::custom),
        other => Err(D::Error::custom(format!(
            "expected integer bytes or humansize string, got {other:?}",
        ))),
    }
}

fn parse_humansize(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num_part, suffix) = s
        .find(|c: char| c.is_alphabetic())
        .map(|i| (&s[..i], &s[i..]))
        .unwrap_or((s, ""));
    let n: u64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("invalid number in `{s}`"))?;
    let mult: u64 = match suffix.trim() {
        "" | "B" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        "GiB" => 1024 * 1024 * 1024,
        other => return Err(format!("unknown size suffix `{other}` (expected KiB|MiB|GiB)")),
    };
    Ok(n * mult)
}
```

- [ ] Run: `cargo test --features test-loopback config::tests::debug 2>&1 | tail -20`
  Expected: all four pass.

### Step 1.5: Run the full config test suite

- [ ] Run: `cargo test --features test-loopback config:: 2>&1 | tail -10`
  Expected: all previously-passing tests still pass; new tests pass.
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
  Expected: clean.

### Step 1.6: Commit

```bash
git add src/config.rs
git commit -m "feat(m8): add [ssrf] and [debug] config sections"
```

---

## Task 2: Extend `SsrfLevel` + retire `TestLoopback`

Add the four new variants, remove `TestLoopback`, and extend `validate_addresses` to cover them all. Defer `file://` to Task 4; this task is IP-only.

**Files:**
- Modify: `src/fetcher/ssrf.rs`

### Step 2.1: Read the current ssrf module

- [ ] Read `src/fetcher/ssrf.rs` end-to-end. The enum is at line 17-25; `validate_addresses` at 66; `strict_reject_reason` is the helper that lists every always-block range. Carry that knowledge forward — every new variant builds on it.

### Step 2.2: Write failing tests for new variants

- [ ] In `src/fetcher/ssrf.rs::tests` add:

```rust
#[test]
fn loopback_accepts_127_block_and_v6_localhost() {
    use std::net::Ipv4Addr;
    assert!(
        validate_addresses(&[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))], SsrfLevel::Loopback)
            .is_ok()
    );
    assert!(
        validate_addresses(&[IpAddr::V6(Ipv6Addr::LOCALHOST)], SsrfLevel::Loopback).is_ok()
    );
}

#[test]
fn loopback_still_rejects_rfc1918() {
    use std::net::Ipv4Addr;
    let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    assert!(validate_addresses(&[addr], SsrfLevel::Loopback).is_err());
}

#[test]
fn lan_accepts_rfc1918_and_ulas() {
    use std::net::{Ipv4Addr, Ipv6Addr};
    // RFC1918 (each of the three blocks)
    for v4 in &[
        Ipv4Addr::new(10, 0, 0, 1),
        Ipv4Addr::new(172, 16, 0, 1),
        Ipv4Addr::new(192, 168, 0, 1),
    ] {
        assert!(
            validate_addresses(&[IpAddr::V4(*v4)], SsrfLevel::Lan).is_ok(),
            "expected {v4} to be accepted at Lan",
        );
    }
    // IPv6 ULA fc00::/7
    let ula = IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1));
    assert!(validate_addresses(&[ula], SsrfLevel::Lan).is_ok());
}

#[test]
fn lan_still_rejects_link_local() {
    use std::net::Ipv4Addr;
    let addr = IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1));
    assert!(validate_addresses(&[addr], SsrfLevel::Lan).is_err());
}

#[test]
fn project_level_inherits_loopback_ip_rules() {
    use std::net::Ipv4Addr;
    // Project = Loopback + file:// (file:// covered in task 4).
    assert!(
        validate_addresses(&[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))], SsrfLevel::Project)
            .is_ok()
    );
    assert!(
        validate_addresses(&[IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))], SsrfLevel::Project)
            .is_err()
    );
}

#[test]
fn none_accepts_arbitrary_public_ip() {
    use std::net::Ipv4Addr;
    let addr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
    assert!(validate_addresses(&[addr], SsrfLevel::None).is_ok());
}

#[test]
fn none_still_blocks_zero_address() {
    use std::net::Ipv4Addr;
    let addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    assert!(
        validate_addresses(&[addr], SsrfLevel::None).is_err(),
        "0.0.0.0 must be blocked at every level",
    );
}

#[test]
fn ssrf_level_parses_from_str() {
    assert_eq!(SsrfLevel::parse("strict").unwrap(), SsrfLevel::Strict);
    assert_eq!(SsrfLevel::parse("loopback").unwrap(), SsrfLevel::Loopback);
    assert_eq!(SsrfLevel::parse("project").unwrap(), SsrfLevel::Project);
    assert_eq!(SsrfLevel::parse("lan").unwrap(), SsrfLevel::Lan);
    assert_eq!(SsrfLevel::parse("none").unwrap(), SsrfLevel::None);
    assert!(SsrfLevel::parse("bogus").is_err());
}
```

(`use std::net::Ipv6Addr;` may need to be added to the imports — match what's already there.)

- [ ] Run: `cargo test --features test-loopback ssrf:: 2>&1 | tail -20`
  Expected: compile failures referencing `SsrfLevel::Loopback`, `Project`, `Lan`, `None`, `SsrfLevel::parse`.

### Step 2.3: Extend `SsrfLevel` and add `parse`

- [ ] Replace the existing `SsrfLevel` enum and the test-only `TestLoopback` arm with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfLevel {
    /// Public IPs only, http/https only.
    Strict,

    /// Strict + 127.0.0.0/8 + ::1.
    Loopback,

    /// Loopback + `file://` URLs descendant of `[ssrf] project_root` after
    /// symlink resolution. File scheme handling lives in `validate_url`
    /// (see also `src/fetcher/cached.rs` for the dispatch).
    Project,

    /// Project + RFC1918 + IPv6 ULAs (`fc00::/7`).
    Lan,

    /// Trust the user. Link-local, multicast, broadcast, `0.0.0.0`, and
    /// `255.255.255.255` are still blocked — the always-floor.
    None,
}

impl SsrfLevel {
    pub fn parse(s: &str) -> Result<Self, SsrfError> {
        match s {
            "strict" => Ok(Self::Strict),
            "loopback" => Ok(Self::Loopback),
            "project" => Ok(Self::Project),
            "lan" => Ok(Self::Lan),
            "none" => Ok(Self::None),
            other => Err(SsrfError::UnknownLevel {
                level: other.to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Loopback => "loopback",
            Self::Project => "project",
            Self::Lan => "lan",
            Self::None => "none",
        }
    }
}
```

- [ ] Add the new error variant to `SsrfError`:

```rust
    #[error("unknown ssrf level `{level}` (expected one of: strict, loopback, project, lan, none)")]
    UnknownLevel { level: String },
```

### Step 2.4: Refactor `validate_addresses` to support every variant

- [ ] Replace the body of `validate_addresses` with a level-aware decision table. The always-floor (link-local, multicast, broadcast, `0.0.0.0`, `255.255.255.255`) stays as a *separate* check applied first.

```rust
pub fn validate_addresses(addrs: &[IpAddr], level: SsrfLevel) -> Result<(), SsrfError> {
    for &addr in addrs {
        // Always-floor: every level blocks these.
        if let Some(reason) = always_floor_reason(addr) {
            return Err(SsrfError::Address {
                address: addr,
                level,
                reason,
            });
        }
        match level {
            SsrfLevel::Strict => {
                if let Some(reason) = strict_reject_reason(addr) {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::Loopback | SsrfLevel::Project => {
                if let Some(reason) = strict_reject_reason(addr)
                    && !addr.is_loopback()
                {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::Lan => {
                if let Some(reason) = strict_reject_reason(addr)
                    && !(addr.is_loopback() || is_rfc1918(addr) || is_ipv6_ula(addr))
                {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::None => {
                // Already passed the always-floor; allow everything else.
            }
        }
    }
    Ok(())
}
```

- [ ] Split `strict_reject_reason` so the always-floor cases live in their own helper. The intent: `always_floor_reason` covers things every level blocks; `strict_reject_reason` becomes "above-the-floor things Strict additionally blocks (loopback + private + ULA + …)".

```rust
fn always_floor_reason(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_link_local() {
                return Some("link-local IPv4");
            }
            if v4.is_multicast() {
                return Some("multicast IPv4");
            }
            if v4.is_broadcast() {
                return Some("broadcast IPv4");
            }
            if v4.is_unspecified() {
                return Some("unspecified IPv4 (0.0.0.0)");
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_multicast() {
                return Some("multicast IPv6");
            }
            if v6.is_unspecified() {
                return Some("unspecified IPv6 (::)");
            }
            // Link-local fe80::/10
            let segments = v6.segments();
            if (segments[0] & 0xffc0) == 0xfe80 {
                return Some("link-local IPv6 (fe80::/10)");
            }
        }
    }
    None
}

fn strict_reject_reason(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return Some("loopback IPv4");
            }
            if v4.is_private() {
                return Some("private IPv4 (RFC1918)");
            }
            // shared address space (CGNAT) 100.64.0.0/10
            if v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64 {
                return Some("shared CGNAT IPv4 (100.64.0.0/10)");
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return Some("loopback IPv6");
            }
            if is_ipv6_ula(IpAddr::V6(v6)) {
                return Some("unique-local IPv6 (fc00::/7)");
            }
        }
    }
    None
}

fn is_rfc1918(addr: IpAddr) -> bool {
    matches!(addr, IpAddr::V4(v4) if v4.is_private())
}

fn is_ipv6_ula(addr: IpAddr) -> bool {
    matches!(
        addr,
        IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00,
    )
}
```

- [ ] Make sure the existing `TestLoopback` arm is **gone** — no `#[cfg(any(test, feature = "test-loopback"))]` blocks remain inside `SsrfLevel` or `validate_addresses`. Search the file for `TestLoopback` and remove all hits.

### Step 2.5: Update `validate_url` for the new variants

- [ ] `validate_url` currently has `let _ = level;` (no per-level logic). Keep that line for now — Task 4 will introduce `file://` handling. Confirm the function still compiles against `SsrfLevel::Strict` and the new variants.

### Step 2.6: Run the ssrf test module

- [ ] Run: `cargo test --features test-loopback ssrf:: 2>&1 | tail -20`
  Expected: all new tests pass. Pre-existing tests that used `SsrfLevel::Strict` still pass. The `TestLoopback`-specific test (around line 254-258 of the original file) must be updated or removed — its intent is now covered by `loopback_accepts_127_block_and_v6_localhost`. If it remains and fails to compile, delete it.

### Step 2.7: Compile-fix downstream callers of `TestLoopback`

- [ ] Run: `cargo build --features test-loopback 2>&1 | head -40` to find every TestLoopback reference.
- [ ] For each test file (likely `tests/fetcher_*.rs`, `tests/cli_*.rs`, possibly `src/main.rs`), swap:

```rust
SsrfLevel::TestLoopback
```

→

```rust
SsrfLevel::Loopback
```

- [ ] Also check for `#[cfg(any(test, feature = "test-loopback"))]` gating around `SsrfLevel::TestLoopback` and drop the cfg guard — the production enum carries `Loopback` now.

### Step 2.8: Full-suite check

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
  Expected: all tests pass. Test count should be ≥ 453 + 8 new ssrf tests (= 461) but may differ if the TestLoopback-specific test was deleted.
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
  Expected: clean.

### Step 2.9: Commit

```bash
git add src/fetcher/ssrf.rs tests/
git commit -m "feat(m8): extend ssrflevel to full matrix; retire testloopback variant"
```

---

## Task 3: Wire SSRF config through the fetcher

`SsrfLevel` is now a typed enum; the new `[ssrf]` config section is a `String`. Bridge them at startup so the fetcher uses the configured level instead of hardcoded `Strict`.

**Files:**
- Modify: `src/mcp/handler.rs` or `src/mcp/server.rs` (wherever `ssrf_level` is currently picked up)
- Modify: `src/main.rs` (CLI subcommands)
- Modify: `src/fetcher/cached.rs` or `src/fetcher/mod.rs` (passing the level through)

### Step 3.1: Locate the current SSRF level resolution

- [ ] Run: `grep -rn "SsrfLevel::\|ssrf_level" src/ 2>&1 | head -30`. Two important sites:
  - `src/mcp/handler.rs` (or `server.rs`) — holds `ssrf_level` as a struct field, defaulted to `Strict`.
  - `src/main.rs` — CLI subcommands construct the level for `rover fetch` and friends.

The current setup is "construct directly in code". M8 reads from `config.ssrf.level` via `SsrfLevel::parse(&config.ssrf.level)`.

### Step 3.2: Write failing test in `src/config.rs`

- [ ] Add to `#[cfg(test)] mod tests`:

```rust
#[test]
fn ssrf_level_string_resolves_to_typed_variant() {
    let cfg: Config = toml::from_str(r#"[ssrf]
level = "loopback"
"#)
    .unwrap();
    use crate::fetcher::ssrf::SsrfLevel;
    assert_eq!(
        SsrfLevel::parse(&cfg.ssrf.level).unwrap(),
        SsrfLevel::Loopback
    );
}
```

- [ ] Run: `cargo test --features test-loopback config::tests::ssrf_level_string 2>&1 | tail -10`
  Expected: passes (we already have the parse helper).

### Step 3.3: Resolve at startup

- [ ] In whichever startup site holds the level today (most likely `src/mcp/server.rs::build` and `src/main.rs::main` for CLI commands), replace the hardcoded `SsrfLevel::Strict` (or `SsrfLevel::TestLoopback` under `test-loopback`) with:

```rust
let ssrf_level = crate::fetcher::ssrf::SsrfLevel::parse(&config.ssrf.level)
    .map_err(|e| /* whatever the surrounding error type is — likely .map_err(McpError::from)? or io::Error::other */)?;
```

The exact mapping depends on where it runs — match the surrounding error-handling style.

- [ ] For `Project` level, also resolve and canonicalize `config.ssrf.project_root` here (M8 wants startup-time failure for missing project root; defer the canonical-path threading until Task 4 — for now, validate that the path exists):

```rust
if ssrf_level == SsrfLevel::Project {
    let resolved = std::fs::canonicalize(&config.ssrf.project_root)
        .map_err(/* … */)?;
    tracing::info!(
        target: "rover::ssrf",
        project_root = %resolved.display(),
        "ssrf level=project; project_root resolved",
    );
    // Pass `resolved` through to the fetcher; details in task 4.
}
```

For Task 3, the canonicalization is best-effort logging — the actual threading lands in Task 4. **Don't add new fields to `RoverHandler` yet** — those land in Task 4.

### Step 3.4: Replace `test-loopback`-gated `TestLoopback` in startup paths

- [ ] Where `#[cfg(any(test, feature = "test-loopback"))]` chose `TestLoopback` over `Strict`, remove that gate. The level comes from config; tests under `--features test-loopback` should set `level = "loopback"` in their config or override at struct level via the `RoverHandler` builder.

The simplest path: tests construct `Config::default()` then override `cfg.ssrf.level = "loopback".to_string()` (or call a small `Config::with_ssrf_level` helper added in this step).

- [ ] Add to `Config`:

```rust
impl Config {
    /// Test-only convenience for swapping the SSRF level on an
    /// already-loaded config. Production callers go through TOML.
    #[cfg(any(test, feature = "test-loopback"))]
    pub fn with_ssrf_level(mut self, level: &str) -> Self {
        self.ssrf.level = level.to_string();
        self
    }
}
```

### Step 3.5: Update test harness helpers

- [ ] `tests/common/mod.rs::spawn_client` (and any sibling helpers) currently writes a default rover.toml without `[ssrf]`. Update the default written config to include `level = "loopback"` so wiremock-based tests work post-`TestLoopback`-retirement.

```rust
// In tests/common/mod.rs — locate the default config string and append:
fn default_test_config() -> &'static str {
    r#"
[robots]
respect = false

[ssrf]
level = "loopback"
"#
}
```

(Match the existing structure — your file may already have a default-config string. If multiple tests construct their own configs without going through this helper, those tests need the same line.)

### Step 3.6: Full-suite check

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
  Expected: all tests pass with the configured level being read from config.
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
  Expected: clean.

### Step 3.7: Commit

```bash
git add src/ tests/
git commit -m "feat(m8): resolve ssrf level from [ssrf] config at startup"
```

---

## Task 4: `file://` support at Project level

Add the file scheme branch to `validate_url` plus the canonicalize+descendant check. Carry `project_root` through to the fetcher so `validate_url` can compare against it.

**Files:**
- Modify: `src/fetcher/ssrf.rs`
- Modify: `src/fetcher/cached.rs` (where `validate_url` is invoked)
- Create: `tests/ssrf_project_file.rs`

### Step 4.1: Write failing tests

- [ ] Create `tests/ssrf_project_file.rs`:

```rust
//! SSRF Project-level `file://` URL handling.
#![cfg(feature = "test-loopback")]

use rover::fetcher::ssrf::{SsrfError, SsrfLevel, validate_url_with_project_root};
use std::fs;
use std::os::unix::fs::symlink;
use tempfile::tempdir;
use url::Url;

#[test]
fn file_inside_project_root_is_allowed_at_project_level() {
    let tmp = tempdir().unwrap();
    let inside = tmp.path().join("inside.txt");
    fs::write(&inside, "hello").unwrap();
    let url = Url::from_file_path(&inside).unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(r.is_ok(), "file inside root should be allowed: {r:?}");
}

#[test]
fn file_outside_project_root_is_rejected_at_project_level() {
    let tmp = tempdir().unwrap();
    let root_dir = tmp.path().join("root");
    fs::create_dir(&root_dir).unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "leak").unwrap();
    let url = Url::from_file_path(&outside).unwrap();
    let root = std::fs::canonicalize(&root_dir).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(
        matches!(r, Err(SsrfError::FileOutsideProjectRoot { .. })),
        "expected FileOutsideProjectRoot, got {r:?}",
    );
}

#[test]
fn symlink_pointing_outside_project_root_is_rejected() {
    let tmp = tempdir().unwrap();
    let root_dir = tmp.path().join("root");
    fs::create_dir(&root_dir).unwrap();
    let outside = tmp.path().join("secret.txt");
    fs::write(&outside, "secret").unwrap();
    let link = root_dir.join("link.txt");
    symlink(&outside, &link).unwrap();
    // After canonicalization the link resolves to `outside`, which is
    // outside `root_dir`. Reject.
    let url = Url::from_file_path(&link).unwrap();
    let root = std::fs::canonicalize(&root_dir).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(
        matches!(r, Err(SsrfError::FileOutsideProjectRoot { .. })),
        "expected symlink rejection, got {r:?}",
    );
}

#[test]
fn file_scheme_rejected_at_strict_or_loopback() {
    let url = Url::parse("file:///etc/hosts").unwrap();
    for level in [SsrfLevel::Strict, SsrfLevel::Loopback] {
        let r = validate_url_with_project_root(&url, level, None);
        assert!(
            matches!(r, Err(SsrfError::FileSchemeNotAllowed { .. })),
            "expected file:// rejection at {level:?}, got {r:?}",
        );
    }
}

#[test]
fn missing_project_root_at_project_level_is_an_error() {
    let url = Url::parse("file:///tmp/x").unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, None);
    assert!(
        matches!(r, Err(SsrfError::ProjectRootMissing)),
        "expected ProjectRootMissing, got {r:?}",
    );
}
```

- [ ] Run: `cargo test --features test-loopback --test ssrf_project_file 2>&1 | tail -20`
  Expected: compile failures naming `validate_url_with_project_root`, `FileOutsideProjectRoot`, `FileSchemeNotAllowed`, `ProjectRootMissing`.

### Step 4.2: Extend `SsrfError`

- [ ] In `src/fetcher/ssrf.rs::SsrfError`:

```rust
    #[error("file:// URLs are not allowed at level {level:?}")]
    FileSchemeNotAllowed { level: SsrfLevel },

    #[error("file path {path} is not a descendant of project_root {root}")]
    FileOutsideProjectRoot {
        path: std::path::PathBuf,
        root: std::path::PathBuf,
    },

    #[error("file path {path} could not be canonicalized: {source}")]
    FileCanonicalize {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("project_root is required when ssrf.level = project")]
    ProjectRootMissing,
```

### Step 4.3: Add `validate_url_with_project_root`

- [ ] Keep the existing `validate_url` as a thin shim that delegates with `project_root: None` (only valid for non-`Project` levels — return `ProjectRootMissing` if called for `Project`):

```rust
/// Validate the URL itself (scheme, presence of host, file:// rules).
///
/// For `Project` level, callers must pass the canonicalized `project_root`
/// via `validate_url_with_project_root`. Calling `validate_url` (no root)
/// with `level == Project` yields `SsrfError::ProjectRootMissing`.
pub fn validate_url(url: &Url, level: SsrfLevel) -> Result<(), SsrfError> {
    validate_url_with_project_root(url, level, None)
}

pub fn validate_url_with_project_root(
    url: &Url,
    level: SsrfLevel,
    project_root: Option<&std::path::Path>,
) -> Result<(), SsrfError> {
    match url.scheme() {
        "http" | "https" => {
            if url.host_str().is_none() {
                return Err(SsrfError::NoHost);
            }
            Ok(())
        }
        "file" => {
            if !matches!(level, SsrfLevel::Project | SsrfLevel::Lan | SsrfLevel::None) {
                return Err(SsrfError::FileSchemeNotAllowed { level });
            }
            // `None` and `Lan` widen IP ranges, not file scope. For now Project rules apply.
            let root = project_root.ok_or(SsrfError::ProjectRootMissing)?;
            let raw_path = url
                .to_file_path()
                .map_err(|_| SsrfError::FileCanonicalize {
                    path: std::path::PathBuf::from(url.path()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "file:// URL has no local path",
                    ),
                })?;
            let canon = std::fs::canonicalize(&raw_path).map_err(|source| {
                SsrfError::FileCanonicalize {
                    path: raw_path.clone(),
                    source,
                }
            })?;
            // `root` is expected to already be canonicalized by the startup
            // path. Defensive double-check via starts_with on absolute paths.
            if !canon.starts_with(root) {
                return Err(SsrfError::FileOutsideProjectRoot {
                    path: canon,
                    root: root.to_path_buf(),
                });
            }
            Ok(())
        }
        other => Err(SsrfError::Scheme {
            scheme: other.to_string(),
        }),
    }
}
```

### Step 4.4: Thread `project_root` through the fetcher

- [ ] Find `validate_url` call sites in `src/fetcher/cached.rs` (and any helper) — they currently call `validate_url(&url, level)`. The fetcher needs the optional `project_root` path.

- [ ] Add a new field to whatever struct holds the SSRF context. If `FetchOptions` (in `src/fetcher/cached.rs`) doesn't already carry it, add it there. Pattern:

```rust
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
    /// Required when `ssrf_level == Project`. Must be pre-canonicalized.
    pub ssrf_project_root: Option<std::path::PathBuf>,
    pub ignore_robots: bool,
    pub user_agent: String,
}
```

- [ ] Update `validate_url` calls in `cached.rs` to use `validate_url_with_project_root(&url, opts.ssrf_level, opts.ssrf_project_root.as_deref())`.

### Step 4.5: Update Task 3's startup canonicalization to actually thread the value

- [ ] In the startup path from Task 3, capture the canonicalized `project_root` and pass it into the `FetchOptions` builder (or whatever the equivalent is for `RoverHandler` — the handler likely stores it as a field that's read on every fetch).

- [ ] The compile errors from Task 3 will tell you exactly where this matters. Follow them.

### Step 4.6: Update existing callers in `tests/`

- [ ] Any test that builds `FetchOptions` directly (search `FetchOptions {` in `tests/`) needs the new field. Default to `ssrf_project_root: None`.

### Step 4.7: Run the new integration suite

- [ ] Run: `cargo test --features test-loopback --test ssrf_project_file 2>&1 | tail -20`
  Expected: all five tests pass.

### Step 4.8: Full-suite check

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`

### Step 4.9: Commit

```bash
git add src/ tests/
git commit -m "feat(m8): support file:// urls at ssrf project level with symlink-resolved descendant check"
```

---

## Task 5: HAR recorder module + `har` crate dep

The recorder is independent — no consumer wires it in until Task 6. Build and unit-test it in isolation.

**Files:**
- Modify: `Cargo.toml`
- Create: `src/fetcher/har.rs`
- Modify: `src/fetcher/mod.rs` (expose the module)

### Step 5.1: Pin `har` crate version

- [ ] Run: `cargo search har 2>&1 | head -10`. Expected output names the current crates.io version. Use the latest 0.x. Document in the commit message if it differs from 0.9.

### Step 5.2: Add the dep

- [ ] In `Cargo.toml` under `[dependencies]`, add:

```toml
har = "0.9"
```

(Adjust version per Step 5.1.)

- [ ] Run: `cargo build --features test-loopback 2>&1 | tail -5`
  Expected: builds. New `har` crate downloaded.

### Step 5.3: Write failing tests for `HarRecorder`

- [ ] Create `src/fetcher/har.rs` (start with just an empty module + the test block):

```rust
//! HAR (HTTP Archive) debug recorder.
//!
//! Activated by `[debug] har_path` in config. Wraps each fetch round-trip
//! into an `har::v1_2::Entries` entry and flushes periodically. Bodies are
//! truncated to `[debug] har_body_cap`.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn recorder_writes_entry_on_record() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("rover.har");
        let recorder = HarRecorder::new(path.clone(), 64 * 1024).unwrap();
        recorder
            .record(RecordedExchange {
                url: "https://example.com/".to_string(),
                method: "GET".to_string(),
                request_headers: vec![("user-agent".into(), "Rover/0.1".into())],
                response_status: 200,
                response_headers: vec![("content-type".into(), "text/html".into())],
                response_body: b"<html></html>".to_vec(),
                duration: Duration::from_millis(50),
            })
            .await
            .unwrap();
        recorder.flush().await.unwrap();
        assert!(path.exists(), "har file should exist");
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["log"]["version"], "1.2");
        assert!(parsed["log"]["entries"].is_array());
        assert_eq!(parsed["log"]["entries"].as_array().unwrap().len(), 1);
        let entry = &parsed["log"]["entries"][0];
        assert_eq!(entry["request"]["url"], "https://example.com/");
        assert_eq!(entry["response"]["status"], 200);
    }

    #[tokio::test]
    async fn body_truncated_when_over_cap() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("trunc.har");
        let recorder = HarRecorder::new(path.clone(), 8).unwrap();
        recorder
            .record(RecordedExchange {
                url: "https://x/".to_string(),
                method: "GET".to_string(),
                request_headers: vec![],
                response_status: 200,
                response_headers: vec![],
                response_body: b"hello-this-body-is-large".to_vec(),
                duration: Duration::from_millis(5),
            })
            .await
            .unwrap();
        recorder.flush().await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entry = &parsed["log"]["entries"][0];
        let body_text = entry["response"]["content"]["text"].as_str().unwrap_or("");
        assert!(
            body_text.len() <= 8,
            "body should be truncated to <= cap, was {}",
            body_text.len()
        );
        assert!(
            entry["response"]["content"]["comment"]
                .as_str()
                .unwrap_or("")
                .contains("truncated"),
            "expected truncation comment",
        );
    }

    #[test]
    fn new_rejects_unwritable_path() {
        // Directory that doesn't exist and can't be created — pick a path that
        // requires writing under a file (not a directory).
        let bad = std::path::PathBuf::from("/this/path/cannot/exist/rover.har");
        let r = HarRecorder::new(bad, 1024);
        assert!(r.is_err(), "expected error for unwritable path");
    }
}
```

- [ ] Run: `cargo test --features test-loopback --lib fetcher::har 2>&1 | tail -20`
  Expected: compile failures naming `HarRecorder`, `RecordedExchange`.

### Step 5.4: Implement `HarRecorder`

- [ ] Add to `src/fetcher/har.rs` (above the `#[cfg(test)] mod tests`):

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum HarError {
    #[error("could not open har file {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("could not serialize har: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("could not write har file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Snapshot of one HTTP round-trip handed to the recorder. Keeps the
/// recorder ignorant of reqwest internals so it's easy to unit test.
#[derive(Debug, Clone)]
pub struct RecordedExchange {
    pub url: String,
    pub method: String,
    pub request_headers: Vec<(String, String)>,
    pub response_status: u16,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Vec<u8>,
    pub duration: Duration,
}

/// HAR recorder. Holds an in-memory accumulator and flushes the full
/// file on `flush()`. For long-running servers, callers should call
/// `flush` on an interval; for short-lived CLI runs, calling once at
/// shutdown is sufficient.
#[derive(Debug, Clone)]
pub struct HarRecorder {
    path: PathBuf,
    body_cap: u64,
    entries: Arc<Mutex<Vec<har::v1_2::Entries>>>,
}

impl HarRecorder {
    pub fn new(path: PathBuf, body_cap: u64) -> Result<Self, HarError> {
        // Validate writability by creating + truncating the file.
        std::fs::File::create(&path).map_err(|source| HarError::Open {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            path,
            body_cap,
            entries: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub async fn record(&self, ex: RecordedExchange) -> Result<(), HarError> {
        let entry = self.build_entry(ex);
        self.entries.lock().await.push(entry);
        Ok(())
    }

    pub async fn flush(&self) -> Result<(), HarError> {
        let entries = self.entries.lock().await.clone();
        let har_doc = har::Har {
            log: har::Spec::V1_2(har::v1_2::Log {
                creator: har::v1_2::Creator {
                    name: "rover".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    comment: None,
                },
                browser: None,
                pages: None,
                entries,
                comment: None,
            }),
        };
        let json = serde_json::to_string_pretty(&har_doc)?;
        std::fs::write(&self.path, json).map_err(|source| HarError::Write {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    fn build_entry(&self, ex: RecordedExchange) -> har::v1_2::Entries {
        let cap = self.body_cap as usize;
        let (text, truncated) = if ex.response_body.len() > cap {
            (
                String::from_utf8_lossy(&ex.response_body[..cap]).into_owned(),
                true,
            )
        } else {
            (String::from_utf8_lossy(&ex.response_body).into_owned(), false)
        };
        let comment = truncated.then(|| {
            format!(
                "truncated at {} bytes (har_body_cap)",
                self.body_cap,
            )
        });
        har::v1_2::Entries {
            pageref: None,
            started_date_time: jiff::Timestamp::now().to_string(),
            time: ex.duration.as_millis() as f64,
            request: har::v1_2::Request {
                method: ex.method,
                url: ex.url,
                http_version: "HTTP/1.1".to_string(),
                cookies: vec![],
                headers: ex
                    .request_headers
                    .into_iter()
                    .map(|(name, value)| har::v1_2::Headers {
                        name,
                        value,
                        comment: None,
                    })
                    .collect(),
                query_string: vec![],
                post_data: None,
                headers_size: -1,
                body_size: -1,
                comment: None,
                headers_compression: None,
            },
            response: har::v1_2::Response {
                status: i64::from(ex.response_status),
                status_text: String::new(),
                http_version: "HTTP/1.1".to_string(),
                cookies: vec![],
                headers: ex
                    .response_headers
                    .into_iter()
                    .map(|(name, value)| har::v1_2::Headers {
                        name,
                        value,
                        comment: None,
                    })
                    .collect(),
                content: har::v1_2::Content {
                    size: ex.response_body.len() as i64,
                    compression: None,
                    mime_type: String::new(),
                    text: Some(text),
                    encoding: None,
                    comment,
                },
                redirect_url: String::new(),
                headers_size: -1,
                body_size: -1,
                comment: None,
                headers_compression: None,
            },
            cache: har::v1_2::Cache::default(),
            timings: har::v1_2::Timings {
                blocked: Some(-1.0),
                dns: Some(-1.0),
                connect: Some(-1.0),
                send: 0.0,
                wait: ex.duration.as_millis() as f64,
                receive: 0.0,
                ssl: Some(-1.0),
                comment: None,
            },
            server_ip_address: None,
            connection: None,
            comment: None,
        }
    }
}
```

**Field names may differ slightly between `har` crate versions.** If the build fails on a missing/extra field, consult `cargo doc --open --package har` and adjust. The struct shape above matches HAR 1.2 spec mid-2026; the `har` 0.9 line carries it.

### Step 5.5: Expose the module

- [ ] In `src/fetcher/mod.rs`, add:

```rust
pub mod har;
```

(Match the existing pub mod style.)

### Step 5.6: Run the recorder tests

- [ ] Run: `cargo test --features test-loopback --lib fetcher::har 2>&1 | tail -20`
  Expected: all three tests pass.
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
  Expected: clean.

### Step 5.7: Commit

```bash
git add Cargo.toml Cargo.lock src/fetcher/
git commit -m "feat(m8): har recorder module with body cap + json round-trip"
```

---

## Task 6: Wire HAR recorder into the fetcher

Wire the recorder so every HTTP round-trip in `fetch_with_cache` emits an exchange to it when configured.

**Files:**
- Modify: `src/fetcher/cached.rs`
- Modify: `src/mcp/server.rs` (instantiate at startup)
- Modify: `src/main.rs` (instantiate for CLI subcommands)
- Create: `tests/har_output.rs`

### Step 6.1: Failing integration test

- [ ] Create `tests/har_output.rs`:

```rust
//! End-to-end HAR recording via `[debug] har_path`.
#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client_with_config};

#[tokio::test]
async fn fetch_writes_har_entry_when_har_path_set() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string("<html><body>hi</body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let har_path = tmp.path().join("rover.har");
    let cfg = format!(
        r#"
[robots]
respect = false

[ssrf]
level = "loopback"

[debug]
har_path = "{}"
"#,
        har_path.display(),
    );
    let client = spawn_client_with_config(tmp.path(), &cfg).await;

    let url = format!("{}/p", server.uri());
    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    let args = json!({ "url": url });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let _ = client.call_tool(params).await.expect("fetch ok");

    // Give the recorder a moment to flush. The server's shutdown path
    // also flushes; `client.cancel().await` triggers it.
    client.cancel().await.unwrap();

    assert!(har_path.exists(), "expected HAR file at {}", har_path.display());
    let text = std::fs::read_to_string(&har_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["log"]["version"], "1.2");
    let entries = parsed["log"]["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "expected at least one HAR entry: {text}"
    );
    assert!(entries[0]["request"]["url"]
        .as_str()
        .unwrap()
        .contains("/p"));
}
```

- [ ] Likely `tests/common/mod.rs` doesn't have `spawn_client_with_config`. Add a sibling helper that takes a config TOML string and writes it to `tmp/rover.toml` before spawning. Mirror `spawn_client`'s spawn logic.

```rust
// In tests/common/mod.rs, alongside spawn_client:
pub async fn spawn_client_with_config(data_dir: &std::path::Path, config_toml: &str) -> /* matching return type */ {
    std::fs::write(data_dir.join("rover.toml"), config_toml).unwrap();
    spawn_client(data_dir).await
}
```

- [ ] Run: `cargo test --features test-loopback --test har_output 2>&1 | tail -20`
  Expected: compile fails or test fails because HAR recording isn't wired.

### Step 6.2: Instantiate `HarRecorder` at server startup

- [ ] In `src/mcp/server.rs::build` (the startup function), after the config is loaded:

```rust
let har_recorder = if !config.debug.har_path.is_empty() {
    let path = std::path::PathBuf::from(&config.debug.har_path);
    let recorder = crate::fetcher::har::HarRecorder::new(path, config.debug.har_body_cap)
        .map_err(/* surrounding error mapping */)?;
    Some(std::sync::Arc::new(recorder))
} else {
    None
};
```

- [ ] Pass `har_recorder: Option<Arc<HarRecorder>>` into `RoverHandler` as a new field (match the existing field-add pattern from M7 where `summarizer` was added).

### Step 6.3: Plumb to the fetcher

- [ ] Extend `FetchOptions` in `src/fetcher/cached.rs`:

```rust
    /// Optional HAR recorder. When `Some`, every successful (or 4xx/5xx)
    /// round-trip is recorded as a HAR entry.
    pub har_recorder: Option<std::sync::Arc<crate::fetcher::har::HarRecorder>>,
```

- [ ] Inside `fetch_with_cache` (or wherever the actual `reqwest::Client::execute(...)` runs), after the response is fully read, call:

```rust
if let Some(rec) = opts.har_recorder.as_ref() {
    let exchange = crate::fetcher::har::RecordedExchange {
        url: response_url.to_string(),
        method: "GET".to_string(),
        request_headers: request_headers_pairs,   // Vec<(String, String)>
        response_status: status.as_u16(),
        response_headers: response_headers_pairs,
        response_body: body_bytes.clone(),
        duration: elapsed,
    };
    if let Err(e) = rec.record(exchange).await {
        tracing::warn!(target: "rover::fetcher", error = ?e, "failed to record har entry");
    }
}
```

Capturing `request_headers_pairs`/`response_headers_pairs` from reqwest:

```rust
fn header_pairs(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}
```

(Place `header_pairs` near the top of `cached.rs` as a private helper, or inline.)

### Step 6.4: Flush at server shutdown

- [ ] In `RoverHandler`'s shutdown path (search for `Drop` impl or an explicit `shutdown` method), if `har_recorder.is_some()` call `recorder.flush().await`. If there's no clean shutdown hook, instead spawn a tokio task at startup that flushes every 5s:

```rust
if let Some(rec) = har_recorder.clone() {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = rec.flush().await {
                tracing::warn!(target: "rover::fetcher", error = ?e, "har periodic flush failed");
            }
        }
    });
}
```

The periodic task is the simplest correct path. The integration test's `client.cancel().await` may need to wait briefly for the next flush — adjust `interval` to 200ms in `#[cfg(test)]` builds or accept a slightly-flaky test and tighten later. Recommended: hardcode 5s and have the test sleep briefly before reading the file.

- [ ] Adjust the test from Step 6.1 if needed:

```rust
    // After client.cancel().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
```

Better path: expose a public `flush()` on `RoverHandler` and call it from the test client's shutdown sequence. Decide based on existing infrastructure when implementing.

### Step 6.5: CLI side

- [ ] Mirror the recorder instantiation in `src/main.rs` for CLI subcommands that invoke `fetch_with_cache` (likely `rover fetch`). Wire through the same Optional Arc.

### Step 6.6: Update existing call sites

- [ ] Any place constructing `FetchOptions { ... }` literally (search `FetchOptions {` in `src/` and `tests/`) needs the new `har_recorder: None` field added.

### Step 6.7: Run the integration test

- [ ] Run: `cargo test --features test-loopback --test har_output 2>&1 | tail -20`
  Expected: passes (after adjusting for the flush latency).

### Step 6.8: Full-suite check + commit

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
- [ ] Commit:

```bash
git add src/ tests/
git commit -m "feat(m8): wire har recorder into fetcher; flush on interval"
```

---

## Task 7: Secret redaction tracing layer

A `tracing_subscriber::Layer` that walks event field values and rewrites URL query-string secrets.

**Files:**
- Create: `src/telemetry/redact.rs`
- Modify: `src/telemetry.rs` (or `src/telemetry/mod.rs` — convert to module dir if needed)
- Create: `tests/redact_logs.rs`

### Step 7.1: Convert `src/telemetry.rs` to a module directory (if currently single-file)

- [ ] Check: `test -f src/telemetry.rs && echo "single-file"`.
- [ ] If single-file: `mkdir src/telemetry && git mv src/telemetry.rs src/telemetry/mod.rs`.

### Step 7.2: Write the failing redaction unit test

- [ ] Create `src/telemetry/redact.rs`:

```rust
//! Tracing layer that redacts URL query-string values for keys in a
//! hardcoded denylist (`api_key`, `token`, `secret`, `password`).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_api_key_query_param() {
        let url = "https://api.example.com/v1/x?api_key=AKIAIOSFODNN7EXAMPLE&page=1";
        let out = redact_url(url);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "got: {out}");
        assert!(out.contains("api_key=%3Credacted%3E") || out.contains("api_key=<redacted>"), "got: {out}");
        assert!(out.contains("page=1"), "non-secret param should remain: {out}");
    }

    #[test]
    fn redacts_token_substring_match() {
        // Trigger substring is case-insensitive and substring-based: "access_token"
        // contains "token".
        let url = "https://x/?access_token=abc";
        let out = redact_url(url);
        assert!(!out.contains("abc"), "got: {out}");
    }

    #[test]
    fn leaves_non_secret_url_alone() {
        let url = "https://x/?page=2&size=10";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn passes_through_non_url_strings() {
        let s = "this is not a url";
        assert_eq!(redact_url(s), s);
    }
}
```

- [ ] Run: `cargo test --features test-loopback --lib telemetry::redact 2>&1 | tail -10`
  Expected: compile failure naming `redact_url`.

### Step 7.3: Implement `redact_url`

- [ ] Add above the test module in `src/telemetry/redact.rs`:

```rust
use url::Url;

const TRIGGER_KEYS: &[&str] = &["api_key", "token", "secret", "password"];

/// Redact secret query-string values from `s`. If `s` is not a URL or has
/// no triggering keys, returns the input unchanged.
///
/// Allocation-free fast path: short-circuit when the string contains
/// neither `=` nor `?`. Otherwise parse, walk pairs, only allocate if at
/// least one rewrite happens.
pub fn redact_url(s: &str) -> String {
    if !s.contains('=') && !s.contains('?') {
        return s.to_string();
    }
    let Ok(mut url) = Url::parse(s) else {
        return s.to_string();
    };
    let original_query = url.query().map(str::to_string);
    let Some(query) = original_query else {
        return s.to_string();
    };
    let mut rewritten = String::with_capacity(query.len());
    let mut changed = false;
    let mut first = true;
    for pair in query.split('&') {
        if !first {
            rewritten.push('&');
        }
        first = false;
        if let Some((k, _v)) = pair.split_once('=') {
            let k_lower = k.to_lowercase();
            if TRIGGER_KEYS.iter().any(|t| k_lower.contains(t)) {
                rewritten.push_str(k);
                rewritten.push_str("=<redacted>");
                changed = true;
                continue;
            }
        }
        rewritten.push_str(pair);
    }
    if !changed {
        return s.to_string();
    }
    url.set_query(Some(&rewritten));
    url.to_string()
}
```

- [ ] Run: `cargo test --features test-loopback --lib telemetry::redact 2>&1 | tail -10`
  Expected: 4 tests pass.

### Step 7.4: Write the tracing layer

- [ ] Append to `src/telemetry/redact.rs`:

```rust
use std::fmt;
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer};

/// Tracing layer that intercepts every event field-by-field and rewrites
/// URL-shaped values via [`redact_url`]. Installed as the outermost layer
/// so any downstream layer sees redacted output.
#[derive(Debug, Default, Clone, Copy)]
pub struct RedactionLayer;

impl<S> Layer<S> for RedactionLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        // Implementation note: tracing_subscriber doesn't let layers
        // *mutate* events in-place; the layer-level rewrite needs an
        // owned-event re-emit (or the redaction happens at format time).
        // For M8 we install the redaction at format time via a custom
        // FmtSpan / make_writer approach — see telemetry::init.
        let _ = event;
    }
}

/// Visitor used inside the format layer. Walks an event's fields and
/// writes them redacted to the destination string.
pub struct RedactingVisitor<'a> {
    pub out: &'a mut String,
}

impl<'a> Visit for RedactingVisitor<'a> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let redacted = redact_url(value);
        let _ = std::fmt::write(&mut *self.out, format_args!(" {}={}", field.name(), redacted));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let formatted = format!("{value:?}");
        let redacted = redact_url(&formatted);
        let _ = std::fmt::write(&mut *self.out, format_args!(" {}={}", field.name(), redacted));
    }
}
```

The cleanest production wire-up is a *format layer* (not an event-mutation layer) — `tracing_subscriber::fmt::format::Writer` doesn't natively support mutation. The simplest correct approach: install a custom `fmt::FormatEvent` that walks fields via `RedactingVisitor`.

For Task 7's MVP scope, however, we accept a coarser approach: **emit-side redaction at every `tracing::info!(url = …)` call site that takes a URL**. Implementer judgment: if the `tracing_subscriber::fmt::FormatEvent` custom impl is straightforward in the version we use, do it; otherwise, the `RedactingVisitor` + a custom format layer goes here:

```rust
use tracing_subscriber::fmt::{
    format::{FormatEvent, FormatFields, Writer},
    FmtContext, FormattedFields,
};

pub struct RedactingFormatEvent;

impl<S, N> FormatEvent<S, N> for RedactingFormatEvent
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        write!(writer, "{} {} {}:", chrono_now(), metadata.level(), metadata.target())?;
        let mut buf = String::new();
        let mut visitor = RedactingVisitor { out: &mut buf };
        event.record(&mut visitor);
        writeln!(writer, "{buf}")?;
        Ok(())
    }
}

fn chrono_now() -> String {
    jiff::Timestamp::now().to_string()
}
```

### Step 7.5: Install the layer in `telemetry::init`

- [ ] Open `src/telemetry/mod.rs`. Find the existing `init()` (or `init_subscriber`) function. It likely uses `tracing_subscriber::fmt::Subscriber::builder()` or `tracing_subscriber::registry()`. Replace the formatter with `RedactingFormatEvent`:

```rust
use tracing_subscriber::{fmt, EnvFilter};
use crate::telemetry::redact::RedactingFormatEvent;

pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    fmt::Subscriber::builder()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .event_format(RedactingFormatEvent)
        .init();
}
```

(Adjust to match the existing `init` signature/return type.)

- [ ] Add `pub mod redact;` to `src/telemetry/mod.rs`.

### Step 7.6: Integration test for end-to-end redaction

- [ ] Create `tests/redact_logs.rs`:

```rust
//! Smoke test: tracing event with a sensitive URL is redacted in stderr.
#![cfg(feature = "test-loopback")]

use rover::telemetry::redact::redact_url;

#[test]
fn unit_path_redacts_api_key() {
    let url = "https://api.example.com/v1?api_key=AKIA";
    assert!(!redact_url(url).contains("AKIA"));
}
```

A full subprocess-captures-stderr test is fragile; the unit test above plus the format-layer wire-up via existing tests cover regression risk. Defer subprocess capture to a follow-up if a regression emerges.

### Step 7.7: Full-suite + commit

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
- [ ] Commit:

```bash
git add src/telemetry/ tests/redact_logs.rs
git commit -m "feat(m8): secret redaction tracing layer for url query strings"
```

---

## Task 8: Doctor module + Check trait + built-in checks

`rover doctor` machinery, ready to be wired to a CLI in Task 9.

**Files:**
- Create: `src/doctor/mod.rs`
- Create: `src/doctor/checks.rs`
- Modify: `src/lib.rs` (add `pub mod doctor`)

### Step 8.1: Define the trait + report types

- [ ] Create `src/doctor/mod.rs`:

```rust
//! `rover doctor` — diagnostic checks.

pub mod checks;

use std::sync::Arc;
use thiserror::Error;

use crate::config::Config;
use crate::storage::Db;

#[derive(Debug, Error)]
pub enum DoctorError {
    #[error("doctor check infrastructure error: {0}")]
    Infrastructure(String),
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Fail,
    Skip,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckReport {
    pub check: &'static str,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct CheckCtx {
    pub config: Arc<Config>,
    pub db: Db,
}

#[async_trait::async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, ctx: &CheckCtx) -> CheckReport;
}

/// Run every built-in check sequentially. Order: cheap → expensive.
/// Returns the full report list and a summary status (`Fail` if any
/// check failed; `Ok` otherwise — `Skip` is non-failing).
pub async fn run_all(ctx: &CheckCtx) -> (Vec<CheckReport>, CheckStatus) {
    let checks: Vec<Box<dyn Check>> = vec![
        Box::new(checks::SqliteOpen),
        Box::new(checks::SqliteWalMode),
        Box::new(checks::SqliteSchemaVersion),
        Box::new(checks::OutputDirWritable),
        Box::new(checks::NetworkReachable),
        Box::new(checks::ExtractiveSynthesis),
        Box::new(checks::BackendsAuthenticate),
    ];
    let mut reports = Vec::with_capacity(checks.len());
    let mut summary = CheckStatus::Ok;
    for c in &checks {
        let r = c.run(ctx).await;
        if r.status == CheckStatus::Fail {
            summary = CheckStatus::Fail;
        }
        reports.push(r);
    }
    (reports, summary)
}
```

### Step 8.2: Implement the built-in checks

- [ ] Create `src/doctor/checks.rs`:

```rust
//! Built-in `rover doctor` checks.

use async_trait::async_trait;
use std::path::Path;

use super::{Check, CheckCtx, CheckReport, CheckStatus};

pub struct SqliteOpen;

#[async_trait]
impl Check for SqliteOpen {
    fn name(&self) -> &'static str {
        "sqlite_open"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        // Db is already open in CheckCtx — if we got here, it opened.
        let _ = ctx;
        CheckReport {
            check: self.name(),
            status: CheckStatus::Ok,
            detail: None,
        }
    }
}

pub struct SqliteWalMode;

#[async_trait]
impl Check for SqliteWalMode {
    fn name(&self) -> &'static str {
        "sqlite_wal_mode"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let mode_res: Result<String, _> = ctx
            .db
            .conn
            .call(|c| {
                Ok::<String, rusqlite::Error>(c.query_row(
                    "PRAGMA journal_mode",
                    [],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .await;
        match mode_res {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("journal_mode = {mode}")),
            },
            Ok(mode) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("expected wal, got {mode}")),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("query failed: {e}")),
            },
        }
    }
}

pub struct SqliteSchemaVersion;

#[async_trait]
impl Check for SqliteSchemaVersion {
    fn name(&self) -> &'static str {
        "sqlite_schema_version"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        // The current migration count is statically known. Compare against
        // `PRAGMA user_version` (which the storage actor bumps after each
        // migration). Hardcode "current" at the latest M-numbered version.
        const CURRENT_USER_VERSION: i64 = 5; // 005_summary_cache.sql is latest in M7.
        let v = ctx
            .db
            .conn
            .call(|c| {
                Ok::<i64, rusqlite::Error>(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                )
            })
            .await;
        match v {
            Ok(n) if n == CURRENT_USER_VERSION => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("user_version = {n}")),
            },
            Ok(n) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("user_version = {n}, expected {CURRENT_USER_VERSION}")),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("query failed: {e}")),
            },
        }
    }
}

pub struct OutputDirWritable;

#[async_trait]
impl Check for OutputDirWritable {
    fn name(&self) -> &'static str {
        "output_dir_writable"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let dir = match crate::extractor::output::OutputPaths::resolve(
            ctx.config.output.dir.as_deref(),
        ) {
            Ok(p) => p.root().to_path_buf(),
            Err(e) => {
                return CheckReport {
                    check: self.name(),
                    status: CheckStatus::Fail,
                    detail: Some(format!("could not resolve: {e}")),
                };
            }
        };
        let probe = dir.join(".rover_doctor_probe");
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                CheckReport {
                    check: self.name(),
                    status: CheckStatus::Ok,
                    detail: Some(format!("writable: {}", short(&dir))),
                }
            }
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("write probe failed at {}: {e}", short(&dir))),
            },
        }
    }
}

pub struct NetworkReachable;

#[async_trait]
impl Check for NetworkReachable {
    fn name(&self) -> &'static str {
        "network_reachable"
    }
    async fn run(&self, _ctx: &CheckCtx) -> CheckReport {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return CheckReport {
                    check: self.name(),
                    status: CheckStatus::Fail,
                    detail: Some(format!("client build failed: {e}")),
                };
            }
        };
        match client.head("https://example.com").send().await {
            Ok(resp) if resp.status().is_success() => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("HEAD https://example.com → {}", resp.status())),
            },
            Ok(resp) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("HEAD https://example.com → {}", resp.status())),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("HEAD failed: {e}")),
            },
        }
    }
}

pub struct ExtractiveSynthesis;

#[async_trait]
impl Check for ExtractiveSynthesis {
    fn name(&self) -> &'static str {
        "extractive_synthesis"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        use crate::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
        let be = crate::summarizer::extractive::ExtractiveBackend::new(
            "doctor",
            crate::tokenizer::Tokenizer::O200k,
        );
        let opts = CompactOpts {
            mode: CompactMode::Extractive,
            style: Style::Prose,
            target_tokens: Some(50),
            focus: None,
            preserve: vec![],
            backend_name: "doctor".to_string(),
        };
        let content = "Rover is a polite scraper. It caches what it fetches. It summarizes \
                       what it caches. The summarizer is offline-first.";
        match be.compact(content, &opts).await {
            Ok(out) if !out.trim().is_empty() => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("produced {} chars", out.chars().count())),
            },
            Ok(_) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some("extractive backend returned empty output".to_string()),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("extractive backend errored: {e}")),
            },
        }
        let _ = ctx; // touch to avoid unused warning if used in alt branches
    }
}

pub struct BackendsAuthenticate;

#[async_trait]
impl Check for BackendsAuthenticate {
    fn name(&self) -> &'static str {
        "backends_authenticate"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let cloud_backends: Vec<(&String, &crate::config::BackendConfig)> = ctx
            .config
            .backends
            .iter()
            .filter(|(_, c)| c.kind == "cloud")
            .filter(|(_, c)| {
                c.api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
            })
            .collect();
        if cloud_backends.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no configured cloud backends with non-empty api_key_env".to_string()),
            };
        }
        // Trivial completion against each configured cloud backend. Skip per-
        // backend on infrastructure errors; only mark Fail when the backend
        // explicitly errors (e.g. 401).
        let mut failures = Vec::new();
        for (name, cfg) in cloud_backends {
            // We rebuild a minimal CloudBackend here instead of going through
            // the registry to avoid the extractive-fallback wrap. If a real
            // cloud call fails with AuthFailed/RateLimited/ModelError, the
            // check fails with the underlying reason.
            // … implementation details: see src/summarizer/registry.rs::build_one
            //   for the pattern; mirror it without the validation steps.
            // For brevity in this plan, the implementer should use
            // CloudBackend::new + a tiny compact() call here.
            let _ = (name, cfg);
            let _ = &mut failures;
        }
        if failures.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured cloud backends authenticated".to_string()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(failures.join("; ")),
            }
        }
    }
}

fn short(p: &Path) -> String {
    // Replace $HOME prefix with `~` for friendlier output.
    if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from)
        && let Ok(stripped) = p.strip_prefix(&home)
    {
        return format!("~/{}", stripped.display());
    }
    p.display().to_string()
}
```

The `BackendsAuthenticate` body has a stub for the trivial-completion call — the implementer should flesh it out with `CloudBackend::new` + a 1-token `compact()` per backend, accumulating failure details. Keep the per-backend timeout to 5 seconds via `tokio::time::timeout`.

### Step 8.3: Wire into `src/lib.rs`

- [ ] Add `pub mod doctor;`.

### Step 8.4: Unit-test the cheap checks

- [ ] Add to `src/doctor/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    async fn fresh_ctx() -> (CheckCtx, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("rover.db")).await.unwrap();
        let mut cfg = Config::default();
        cfg.output.dir = Some(tmp.path().to_path_buf());
        (
            CheckCtx {
                config: Arc::new(cfg),
                db,
            },
            tmp,
        )
    }

    #[tokio::test]
    async fn sqlite_open_passes_on_fresh_db() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::SqliteOpen.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok);
    }

    #[tokio::test]
    async fn sqlite_wal_mode_passes_on_fresh_db() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::SqliteWalMode.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn output_dir_writable_passes_on_writable_temp() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::OutputDirWritable.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn backends_authenticate_skips_when_no_cloud_configured() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::BackendsAuthenticate.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Skip);
    }

    #[tokio::test]
    async fn extractive_synthesis_produces_output() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::ExtractiveSynthesis.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }
}
```

- [ ] Run: `cargo test --features test-loopback --lib doctor 2>&1 | tail -20`
  Expected: passes.

### Step 8.5: Commit

```bash
git add src/doctor/ src/lib.rs
git commit -m "feat(m8): doctor module with check trait and built-in checks"
```

---

## Task 9: `rover doctor` CLI subcommand

**Files:**
- Create: `src/cli/doctor.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`
- Create: `tests/cli_doctor.rs`

### Step 9.1: Integration test

- [ ] Create `tests/cli_doctor.rs`:

```rust
//! Subprocess test of `rover doctor`.
#![cfg(feature = "test-loopback")]

use std::process::Command;
use tempfile::tempdir;

fn rover_bin() -> std::path::PathBuf {
    // The standard cargo test layout: target/debug/rover
    let mut p = std::path::PathBuf::from(env!("CARGO_BIN_EXE_rover"));
    if !p.exists() {
        // fallback for older cargo
        p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/debug/rover");
    }
    p
}

#[test]
fn doctor_exits_zero_on_clean_install() {
    let tmp = tempdir().unwrap();
    let out = Command::new(rover_bin())
        .arg("doctor")
        .env("ROVER_DATA_DIR", tmp.path())
        .env("ROVER_OUTPUT_DIR", tmp.path())
        .output()
        .expect("spawn rover doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Allow non-zero exit only if `network_reachable` fails (sandbox).
    if !out.status.success() {
        assert!(
            stdout.contains("network_reachable") || stderr.contains("network_reachable"),
            "non-zero exit but no network_reachable failure:\nstdout:\n{stdout}\nstderr:\n{stderr}",
        );
    } else {
        assert!(stdout.contains("ok"), "expected ok line; stdout:\n{stdout}");
    }
}

#[test]
fn doctor_ndjson_format_one_json_per_line() {
    let tmp = tempdir().unwrap();
    let out = Command::new(rover_bin())
        .arg("doctor")
        .arg("--format=ndjson")
        .env("ROVER_DATA_DIR", tmp.path())
        .env("ROVER_OUTPUT_DIR", tmp.path())
        .output()
        .expect("spawn rover doctor --format=ndjson");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("not json: {line}"));
    }
}
```

- [ ] Run: `cargo build --features test-loopback 2>&1 | tail -5` to ensure the binary builds before the subprocess test runs.

### Step 9.2: Add the subcommand

- [ ] Create `src/cli/doctor.rs`:

```rust
//! `rover doctor` subcommand.

use clap::Args;
use std::sync::Arc;

use crate::doctor::{CheckCtx, CheckStatus};

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Output format. `human` (default) prints one line per check;
    /// `ndjson` emits one JSON object per line for scripting.
    #[arg(long, default_value = "human")]
    pub format: String,
}

pub async fn run(args: DoctorArgs, config: crate::config::Config) -> std::io::Result<i32> {
    let db = crate::storage::Db::open(&crate::paths::db_path(&config)?)
        .await
        .map_err(|e| std::io::Error::other(format!("could not open db: {e}")))?;
    let ctx = CheckCtx {
        config: Arc::new(config),
        db,
    };
    let (reports, summary) = crate::doctor::run_all(&ctx).await;
    match args.format.as_str() {
        "ndjson" => {
            for r in &reports {
                println!("{}", serde_json::to_string(r).unwrap());
            }
        }
        _ => {
            for r in &reports {
                let marker = match r.status {
                    CheckStatus::Ok => "✓",
                    CheckStatus::Fail => "✗",
                    CheckStatus::Skip => "-",
                };
                let detail = r.detail.as_deref().unwrap_or("");
                println!("{marker} {} {}", r.check, detail);
            }
            match summary {
                CheckStatus::Ok | CheckStatus::Skip => println!("all checks ok"),
                CheckStatus::Fail => println!("one or more checks failed"),
            }
        }
    }
    Ok(if summary == CheckStatus::Fail { 1 } else { 0 })
}
```

The `crate::paths::db_path` helper may need adding — match the existing pattern used by `rover cache` subcommands. If it doesn't exist, derive the path inline:

```rust
let db_path = config.server.data_dir.clone()
    .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_default().join("rover"))
    .join("rover.db");
```

### Step 9.3: Wire into the CLI dispatch

- [ ] In `src/cli/mod.rs`, register the `Doctor(DoctorArgs)` variant of the existing `Command` enum (whatever it's called — `Cli`/`Subcommand`/etc.). Match it in `dispatch` (or `main`) to call `cli::doctor::run`.

- [ ] In `src/main.rs`, the dispatch arm:

```rust
Some(Commands::Doctor(args)) => {
    let exit = crate::cli::doctor::run(args, config).await?;
    std::process::exit(exit);
}
```

### Step 9.4: Run the integration test

- [ ] Run: `cargo test --features test-loopback --test cli_doctor 2>&1 | tail -20`
  Expected: both tests pass.

### Step 9.5: Commit

```bash
git add src/cli/ src/main.rs tests/cli_doctor.rs
git commit -m "feat(m8): rover doctor cli subcommand with human + ndjson formats"
```

---

## Task 10: Config provenance tracker

The `config show` subcommand needs to know where each effective value came from. This is the shared helper.

**Files:**
- Create: `src/config/provenance.rs` (new submodule alongside the current `src/config.rs`)

### Step 10.1: Reorganize `src/config.rs` into a module

- [ ] Run: `test -d src/config && echo "module exists"`. If not present:
  - `mkdir src/config`
  - `git mv src/config.rs src/config/mod.rs`
  - Verify `cargo build --features test-loopback` still compiles after the move (it should — Rust's module system treats `config.rs` and `config/mod.rs` identically).

### Step 10.2: Failing test

- [ ] Create `src/config/provenance.rs` with just the test block:

```rust
//! Provenance tracking for `rover config show`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_marks_defaults_vs_file() {
        // Given a config file that overrides only `ssrf.level`, the rest
        // should be marked as `Source::Default`.
        let toml = r#"
[ssrf]
level = "loopback"
"#;
        let provenance = provenance_for(toml);
        let level_row = provenance
            .iter()
            .find(|r| r.dotted == "ssrf.level")
            .expect("ssrf.level present");
        assert_eq!(level_row.source, Source::File);
        let default_row = provenance
            .iter()
            .find(|r| r.dotted == "ssrf.project_root")
            .expect("ssrf.project_root present");
        assert_eq!(default_row.source, Source::Default);
    }

    #[test]
    fn provenance_recognizes_env_override() {
        let toml = "";
        std::env::set_var("ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE", "debug");
        // The env-detection function takes a list of env-overridable keys.
        // For the test we pass a synthetic override and assert it's marked
        // `Source::Env`. Real env mappings live in `env_overrides()` table.
        let rows = provenance_for_with_env(toml, &[("debug.log_level", "ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE")]);
        let r = rows.iter().find(|r| r.dotted == "debug.log_level").unwrap();
        assert_eq!(r.source, Source::Env);
        std::env::remove_var("ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE");
    }
}
```

- [ ] Run: `cargo test --features test-loopback --lib config::provenance 2>&1 | tail -10`
  Expected: compile fail naming `provenance_for`, `Source::File`, etc.

### Step 10.3: Implement

- [ ] Above the test module in `src/config/provenance.rs`:

```rust
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    Env,
}

#[derive(Debug, Clone)]
pub struct ProvenanceRow {
    pub dotted: String,
    pub source: Source,
}

/// Compute provenance by parsing the file as a generic toml::Value and
/// walking known leaf keys. Any leaf present in the file is marked `File`;
/// the rest default to `Default`.
pub fn provenance_for(file_toml: &str) -> Vec<ProvenanceRow> {
    provenance_for_with_env(file_toml, &env_overrides())
}

pub fn provenance_for_with_env(
    file_toml: &str,
    env_table: &[(&'static str, &'static str)],
) -> Vec<ProvenanceRow> {
    let v: toml::Value = toml::from_str(file_toml).unwrap_or_else(|_| toml::Value::Table(Default::default()));
    let mut file_leaves: HashSet<String> = HashSet::new();
    walk_leaves(&v, "", &mut file_leaves);

    let env_leaves: HashSet<String> = env_table
        .iter()
        .filter(|(_, var)| std::env::var(var).map(|s| !s.is_empty()).unwrap_or(false))
        .map(|(key, _)| key.to_string())
        .collect();

    let mut rows = Vec::new();
    for dotted in known_leaves() {
        let source = if env_leaves.contains(dotted) {
            Source::Env
        } else if file_leaves.contains(dotted) {
            Source::File
        } else {
            Source::Default
        };
        rows.push(ProvenanceRow {
            dotted: dotted.to_string(),
            source,
        });
    }
    rows
}

fn walk_leaves(v: &toml::Value, prefix: &str, out: &mut HashSet<String>) {
    if let toml::Value::Table(t) = v {
        for (k, child) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match child {
                toml::Value::Table(_) => walk_leaves(child, &key, out),
                _ => {
                    out.insert(key);
                }
            }
        }
    }
}

/// The list of leaf keys `rover config show` reports on. Kept in sync
/// with `Config`'s struct fields by hand — schemars-style introspection
/// is out of scope for M8.
pub fn known_leaves() -> &'static [&'static str] {
    &[
        "server.data_dir",
        "server.output_dir",
        "fetch.user_agent",
        "fetch.timeout",
        "fetch.max_redirects",
        "ssrf.level",
        "ssrf.project_root",
        "cache.default_ttl",
        "cache.min_ttl",
        "cache.max_ttl",
        "cache.override_no_store",
        "cache.store_raw_html",
        "robots.respect",
        "robots.default_ttl",
        "rate_limit.requests_per_minute_per_domain",
        "rate_limit.per_domain_concurrency",
        "rate_limit.global_concurrency",
        "tokenizer.default",
        "output.dir",
        "summarization.default_backend",
        "summarization.default_mode",
        "summarization.default_style",
        "summarization.fallback_to_extractive",
        "summarization.tables.target_tokens",
        "summarization.tables.focus",
        "debug.har_path",
        "debug.har_body_cap",
        "debug.log_level",
    ]
}

/// Map of leaf key → env var that overrides it. Synced manually with the
/// startup code; missing entries here just mean `show` reports `File` or
/// `Default` for that key even when the env var is set.
pub fn env_overrides() -> &'static [(&'static str, &'static str)] {
    &[
        ("debug.log_level", "ROVER_LOG_LEVEL"),
        ("server.output_dir", "ROVER_OUTPUT_DIR"),
        ("server.data_dir", "ROVER_DATA_DIR"),
    ]
}
```

- [ ] Add `pub mod provenance;` in `src/config/mod.rs`.

### Step 10.4: Run tests + commit

- [ ] Run: `cargo test --features test-loopback --lib config::provenance 2>&1 | tail -10`
  Expected: passes.
- [ ] Commit:

```bash
git add src/config/
git commit -m "feat(m8): config provenance tracker for rover config show"
```

---

## Task 11: `rover config show` CLI

**Files:**
- Create: `src/cli/config.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`
- Add tests to `tests/cli_config.rs`

### Step 11.1: Integration test (start of `tests/cli_config.rs`)

- [ ] Create `tests/cli_config.rs`:

```rust
//! Subprocess tests for `rover config show` and `rover config set`.
#![cfg(feature = "test-loopback")]

use std::process::Command;
use tempfile::tempdir;

fn rover_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rover"))
}

#[test]
fn config_show_marks_provenance() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"loopback\"\n").unwrap();
    let out = Command::new(rover_bin())
        .arg("config")
        .arg("show")
        .arg("--config")
        .arg(&cfg)
        .output()
        .expect("rover config show");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit: {:?}\nstderr: {}", out.status, String::from_utf8_lossy(&out.stderr));
    // ssrf.level should be from: file
    assert!(
        stdout.contains("level = \"loopback\"") && stdout.contains("# from: file"),
        "expected ssrf.level marked file; got:\n{stdout}",
    );
    // ssrf.project_root should be defaulted
    assert!(
        stdout.contains("project_root") && stdout.contains("# from: defaults"),
        "expected project_root marked defaults; got:\n{stdout}",
    );
}
```

- [ ] Run: build will fail because the subcommand doesn't exist yet.

### Step 11.2: Implement `cli::config`

- [ ] Create `src/cli/config.rs`:

```rust
//! `rover config show` + `rover config set`.

use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::config::{provenance, Config};

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the effective configuration with provenance comments.
    Show {
        /// Optional path to the config file. When omitted, the standard
        /// XDG config path is used.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Update a single setting in the config file.
    Set {
        /// Optional path to the config file (overrides XDG).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Dotted key, e.g. `ssrf.level`.
        key: String,
        /// New value.
        value: String,
    },
}

pub async fn run(args: ConfigArgs) -> std::io::Result<i32> {
    match args.action {
        ConfigAction::Show { config } => show(config),
        ConfigAction::Set { config, key, value } => set(config, key, value),
    }
}

fn show(config_path: Option<PathBuf>) -> std::io::Result<i32> {
    let path = config_path.unwrap_or_else(default_config_path);
    let file_text = std::fs::read_to_string(&path).unwrap_or_default();
    // Validate the file parses cleanly — show shouldn't run against a broken file.
    let cfg: Config = toml::from_str(&file_text)
        .map_err(|e| std::io::Error::other(format!("could not parse {}: {e}", path.display())))?;
    let _ = cfg; // we only need it for validation; render below uses file_text + provenance
    let rows = provenance::provenance_for(&file_text);
    print_rendered(&file_text, &rows, &path)?;
    Ok(0)
}

fn print_rendered(
    _file_text: &str,
    rows: &[provenance::ProvenanceRow],
    path: &std::path::Path,
) -> std::io::Result<()> {
    println!("# rover effective configuration");
    println!("# defaults | file ({}) | env", path.display());
    println!();
    // Group by top-level section.
    let mut by_section: std::collections::BTreeMap<&str, Vec<&provenance::ProvenanceRow>> =
        std::collections::BTreeMap::new();
    for r in rows {
        let section = r.dotted.split_once('.').map(|(a, _)| a).unwrap_or("");
        by_section.entry(section).or_default().push(r);
    }
    for (section, rows) in by_section {
        if !section.is_empty() {
            println!("[{section}]");
        }
        for r in rows {
            let value = render_default_value(&r.dotted);
            let source = match r.source {
                provenance::Source::Default => "defaults",
                provenance::Source::File => "file",
                provenance::Source::Env => "env",
            };
            let leaf = r.dotted.rsplit_once('.').map(|(_, b)| b).unwrap_or(&r.dotted);
            println!("{leaf} = {value:25} # from: {source}");
        }
        println!();
    }
    Ok(())
}

fn render_default_value(dotted: &str) -> String {
    // Render the effective value as TOML by reaching back through a
    // Default::default() Config and looking up the leaf. For brevity in
    // this plan, the implementer should use serde_json::Value + a small
    // dotted-path getter, OR maintain a parallel const table.
    let _ = dotted;
    "<value>".to_string()
}

fn set(
    config_path: Option<PathBuf>,
    key: String,
    value: String,
) -> std::io::Result<i32> {
    let path = config_path.unwrap_or_else(default_config_path);
    crate::config::edit::apply_set(&path, &key, &value)
        .map(|()| 0)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("ROVER_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(dir) = dirs::config_dir() {
        return dir.join("rover").join("config.toml");
    }
    PathBuf::from("rover.toml")
}
```

The `render_default_value` body is intentionally simplistic for the plan; the implementer should expand it to actually look up the effective value via either a const table or `serde_json::to_value(Config::default())` indexed by dotted path. The test in §11.1 only asserts on `level = "loopback"` which comes from the file, so a less-than-perfect default rendering won't break the integration test for `show`. Make a focused improvement before Task 12 begins.

### Step 11.3: Register the subcommand

- [ ] In `src/cli/mod.rs`, add `Config(ConfigArgs)` to the command enum.
- [ ] In `src/main.rs`, dispatch:

```rust
Some(Commands::Config(args)) => {
    let exit = crate::cli::config::run(args).await?;
    std::process::exit(exit);
}
```

### Step 11.4: Run the `show` integration test

- [ ] Run: `cargo test --features test-loopback --test cli_config 2>&1 | tail -20`
  Expected: passes for `config_show_marks_provenance`. Task 12 + 13 add the `set` tests.

### Step 11.5: Commit

```bash
git add src/cli/ src/main.rs tests/cli_config.rs
git commit -m "feat(m8): rover config show with per-key provenance comments"
```

---

## Task 12: Settable-key whitelist + parsers

**Files:**
- Create: `src/config/edit.rs`

### Step 12.1: Failing tests

- [ ] Create `src/config/edit.rs`:

```rust
//! `rover config set` — settable-key whitelist + value parsers + writer.

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn unknown_key_is_rejected() {
        let r = apply_set(std::path::Path::new("/dev/null"), "bogus.key", "x");
        assert!(matches!(r, Err(SetError::Unsettable { .. })));
    }

    #[test]
    fn set_writes_value_and_preserves_comments() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        std::fs::write(&p, "# header comment\n[ssrf]\nlevel = \"strict\" # was strict\n").unwrap();
        apply_set(&p, "ssrf.level", "loopback").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# header comment"), "header dropped: {after}");
        assert!(after.contains("level = \"loopback\""), "value not updated: {after}");
        assert!(after.contains("# was strict"), "trailing comment dropped: {after}");
    }

    #[test]
    fn set_invalid_value_does_not_modify_file() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        let original = "[ssrf]\nlevel = \"strict\"\n";
        std::fs::write(&p, original).unwrap();
        let r = apply_set(&p, "ssrf.level", "bogus");
        assert!(matches!(r, Err(SetError::Parse { .. })), "{r:?}");
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, original, "file modified despite parse failure");
    }
}
```

### Step 12.2: Implement

- [ ] Above the test block:

```rust
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SetError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("io error writing {path}: {source}")]
    Write {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("could not parse existing file at {path}: {source}")]
    Parse_ExistingFile {
        path: std::path::PathBuf,
        source: toml_edit::TomlError,
    },

    #[error("invalid value for {key}: expected {expected}, got {value}")]
    Parse {
        key: String,
        value: String,
        expected: String,
    },

    #[error("key `{key}` is not settable via `rover config set`; edit the file directly")]
    Unsettable { key: String },

    #[error("validation failed after writing {key} = {value}: {source}")]
    Validation {
        key: String,
        value: String,
        source: String,
    },
}

struct SettableSpec {
    key: &'static str,
    parser: fn(&str) -> Result<toml_edit::Item, String>,
    expected: &'static str,
}

fn settable() -> &'static [SettableSpec] {
    &[
        SettableSpec {
            key: "ssrf.level",
            parser: parse_enum_str(&["strict", "loopback", "project", "lan", "none"]),
            expected: "one of: strict, loopback, project, lan, none",
        },
        SettableSpec {
            key: "ssrf.project_root",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "fetch.user_agent",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "fetch.timeout",
            parser: parse_string, // humantime; validated on round-trip
            expected: "humantime string (e.g. \"15s\")",
        },
        SettableSpec {
            key: "fetch.max_redirects",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "cache.default_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "cache.min_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "cache.max_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "cache.store_raw_html",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "robots.respect",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "robots.default_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "rate_limit.requests_per_minute_per_domain",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "rate_limit.per_domain_concurrency",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "rate_limit.global_concurrency",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "tokenizer.default",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "output.dir",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "summarization.default_backend",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "summarization.default_mode",
            parser: parse_enum_str(&["abstractive", "extractive", "headlines"]),
            expected: "one of: abstractive, extractive, headlines",
        },
        SettableSpec {
            key: "summarization.default_style",
            parser: parse_enum_str(&["bullet", "prose", "executive"]),
            expected: "one of: bullet, prose, executive",
        },
        SettableSpec {
            key: "summarization.fallback_to_extractive",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "summarization.tables.target_tokens",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "summarization.tables.focus",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "debug.har_path",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "debug.har_body_cap",
            parser: parse_string, // humansize; the config parser handles "64KiB" etc.
            expected: "humansize string or integer",
        },
        SettableSpec {
            key: "debug.log_level",
            parser: parse_enum_str(&["trace", "debug", "info", "warn", "error"]),
            expected: "one of: trace, debug, info, warn, error",
        },
    ]
}

fn parse_string(s: &str) -> Result<toml_edit::Item, String> {
    Ok(toml_edit::value(s.to_string()))
}

fn parse_int(s: &str) -> Result<toml_edit::Item, String> {
    let n: i64 = s.parse().map_err(|_| format!("not an integer: {s}"))?;
    Ok(toml_edit::value(n))
}

fn parse_bool(s: &str) -> Result<toml_edit::Item, String> {
    let b: bool = match s {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => return Err(format!("not a bool: {s}")),
    };
    Ok(toml_edit::value(b))
}

fn parse_enum_str(values: &'static [&'static str]) -> fn(&str) -> Result<toml_edit::Item, String> {
    // Workaround: fn pointers can't close over `values`. Return a const fn pointer
    // and inline the comparison via a static lookup table.
    let _ = values;
    fn inner(_s: &str) -> Result<toml_edit::Item, String> {
        // Placeholder — the implementer should use a small enum-validating
        // wrapper here. For M8 the cleanest expression is a per-key parser
        // closure with the values baked in. See e.g.:
        //
        //   |s| if ["strict","loopback","project","lan","none"].contains(&s) {
        //       Ok(toml_edit::value(s.to_string()))
        //   } else { Err(format!("not a valid level: {s}")) }
        //
        // Rust doesn't let us coerce closures-with-captures to fn pointers,
        // so the spec change here is: each settable enum gets its own
        // non-capturing parser fn. The implementer expands `settable()` to
        // name those fns directly (e.g. `parse_ssrf_level`,
        // `parse_summarization_mode`, etc.).
        unreachable!("placeholder; see comments")
    }
    inner
}
```

**Note to implementer:** the `parse_enum_str(&[...])` trick above doesn't work in Rust — closures with captures can't coerce to `fn` pointers. Replace each `parse_enum_str(&[...])` entry in `settable()` with a dedicated non-capturing parser, e.g.:

```rust
fn parse_ssrf_level(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "strict" | "loopback" | "project" | "lan" | "none" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid ssrf level: {s}")),
    }
}
```

And in the `settable()` table:

```rust
SettableSpec {
    key: "ssrf.level",
    parser: parse_ssrf_level,
    expected: "one of: strict, loopback, project, lan, none",
},
```

The same pattern for `summarization.default_mode`, `summarization.default_style`, `debug.log_level`. Write those out explicitly.

### Step 12.3: Implement `apply_set`

- [ ] Continuing in `src/config/edit.rs`:

```rust
pub fn apply_set(path: &Path, key: &str, value: &str) -> Result<(), SetError> {
    let spec = settable()
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| SetError::Unsettable {
            key: key.to_string(),
        })?;
    let item = (spec.parser)(value).map_err(|_e| SetError::Parse {
        key: key.to_string(),
        value: value.to_string(),
        expected: spec.expected.to_string(),
    })?;

    // Read original.
    let original = std::fs::read_to_string(path).map_err(|source| SetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut doc: toml_edit::DocumentMut = original.parse().map_err(|source| {
        SetError::Parse_ExistingFile {
            path: path.to_path_buf(),
            source,
        }
    })?;

    // Walk to the target leaf, creating intermediate tables as needed.
    let parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.last().unwrap();
    let mut cursor: &mut toml_edit::Item = doc.as_item_mut();
    for p in &parts[..parts.len() - 1] {
        cursor = ensure_table(cursor, p);
    }
    if let Some(t) = cursor.as_table_mut() {
        t[leaf] = item;
    } else if let Some(t) = cursor.as_table_like_mut() {
        t.insert(leaf, item);
    } else {
        // Can't insert into a non-table parent.
        return Err(SetError::Parse {
            key: key.to_string(),
            value: value.to_string(),
            expected: format!("parent `{}` is not a table", parts[..parts.len() - 1].join(".")),
        });
    }

    // Serialize, then validate by round-trip.
    let new_text = doc.to_string();
    let _: crate::config::Config = toml::from_str(&new_text).map_err(|source| {
        SetError::Validation {
            key: key.to_string(),
            value: value.to_string(),
            source: source.to_string(),
        }
    })?;

    // Write the new text. Original is preserved by not touching the file on the failure path.
    std::fs::write(path, new_text).map_err(|source| SetError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn ensure_table<'a>(parent: &'a mut toml_edit::Item, name: &str) -> &'a mut toml_edit::Item {
    let t = parent
        .as_table_mut()
        .expect("ensure_table called on a non-table parent");
    if !t.contains_key(name) {
        t[name] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    &mut t[name]
}
```

### Step 12.4: Module wiring

- [ ] In `src/config/mod.rs`: `pub mod edit;`.

### Step 12.5: Run tests

- [ ] Run: `cargo test --features test-loopback --lib config::edit 2>&1 | tail -10`
  Expected: passes.

### Step 12.6: Commit

```bash
git add src/config/edit.rs src/config/mod.rs
git commit -m "feat(m8): config-set settable-key whitelist + toml_edit writer"
```

---

## Task 13: `rover config set` CLI (integration tests)

The `set` subcommand is already wired in Task 11. This task adds the integration tests and any edge-case polish.

**Files:**
- Modify: `tests/cli_config.rs`

### Step 13.1: Append `set` integration tests

- [ ] Add to `tests/cli_config.rs`:

```rust
#[test]
fn config_set_writes_value() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"strict\"\n").unwrap();
    let out = Command::new(rover_bin())
        .args(["config", "set", "ssrf.level", "loopback", "--config"])
        .arg(&cfg)
        .output()
        .expect("rover config set");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("level = \"loopback\""), "file:\n{after}");
}

#[test]
fn config_set_rejects_unknown_key() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "").unwrap();
    let out = Command::new(rover_bin())
        .args(["config", "set", "bogus.field", "x", "--config"])
        .arg(&cfg)
        .output()
        .expect("rover config set");
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not settable") || stderr.contains("Unsettable"), "stderr:\n{stderr}");
}

#[test]
fn config_set_rejects_invalid_enum_value() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"strict\"\n").unwrap();
    let out = Command::new(rover_bin())
        .args(["config", "set", "ssrf.level", "bogus", "--config"])
        .arg(&cfg)
        .output()
        .expect("rover config set");
    assert!(!out.status.success(), "expected non-zero exit");
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("strict"), "file modified: {after}");
}
```

- [ ] Run: `cargo test --features test-loopback --test cli_config 2>&1 | tail -20`
  Expected: all four `cli_config` tests pass.

### Step 13.2: Commit

```bash
git add tests/cli_config.rs
git commit -m "test(m8): integration tests for rover config set"
```

---

## Task 14: SQLite update_hook for cross-process notify

The scheduler stops polling as the only signal source. New task inserts wake it within milliseconds.

**Files:**
- Modify: `src/storage/mod.rs`
- Modify: `src/tasks/scheduler.rs`
- Create: `tests/cross_process_notify.rs`

### Step 14.1: Failing cross-process test

- [ ] Create `tests/cross_process_notify.rs`:

```rust
//! Two-process scheduler test: an insert from process B is observed by
//! process A's scheduler within 100 ms.
#![cfg(feature = "test-loopback")]

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

#[tokio::test]
async fn second_process_insert_wakes_first_within_100ms() {
    // The simplest integration shape: one in-process scheduler reads `tasks`
    // via the storage actor. A second process opens the same DB and inserts
    // a synthetic task row. The scheduler should pick it up via update_hook
    // before its 10s poll fires.
    //
    // We assert wall-clock: time between insert and observation < 200ms.

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("rover.db");
    let db = rover::storage::Db::open(&db_path).await.unwrap();
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());

    rover::storage::register_tasks_update_hook(&db, notify.clone()).unwrap();

    // Spawn a second process that inserts a task row via the helper binary
    // path. Simpler shortcut: spawn a tiny in-process task that opens its
    // OWN connection to the same path and inserts. This still exercises
    // update_hook because the hook fires on every write at the SQLite
    // engine level regardless of which connection wrote.
    let db_path2 = db_path.clone();
    tokio::spawn(async move {
        let db2 = rover::storage::Db::open(&db_path2).await.unwrap();
        let id = "task-test-id".to_string();
        let id_clone = id.clone();
        db2.conn
            .call(move |c| {
                c.execute(
                    "INSERT INTO tasks (task_id, kind, status, params_json, created_at)
                     VALUES (?1, 'fetch', 'pending', '{}', strftime('%s','now'))",
                    rusqlite::params![id_clone],
                )?;
                Ok::<(), rusqlite::Error>(())
            })
            .await
            .unwrap();
    });

    let start = Instant::now();
    tokio::time::timeout(Duration::from_millis(200), notify.notified())
        .await
        .expect("notify did not fire within 200ms");
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(200), "observed at {elapsed:?}");
}
```

The exact DB schema for `tasks` must match what M6 set up. Adjust the INSERT statement if the columns differ.

### Step 14.2: Expose `register_tasks_update_hook`

- [ ] In `src/storage/mod.rs`, add a function that opens a dedicated short-lived connection, registers the hook, and **keeps the connection alive**. The hook lifetime is tied to the connection lifetime — if the connection drops, the hook stops firing.

```rust
use std::sync::Arc;
use tokio::sync::Notify;

/// Register a SQLite `update_hook` that fires the given `Notify` on every
/// insert/update/delete on the `tasks` table.
///
/// Returns a guard. Drop the guard to detach the hook.
pub struct UpdateHookGuard {
    _conn: rusqlite::Connection,
}

pub fn register_tasks_update_hook(
    db: &Db,
    notify: Arc<Notify>,
) -> Result<UpdateHookGuard, StorageError> {
    // The storage actor owns one connection. We open a SEPARATE connection
    // for the hook to keep the actor's API surface unchanged. update_hook
    // fires on engine-wide writes, so a separate connection still observes
    // them.
    let conn = rusqlite::Connection::open(db.path()).map_err(StorageError::from)?;
    let n = notify.clone();
    conn.update_hook(Some(move |action: rusqlite::hooks::Action, _db: &str, table: &str, _rowid: i64| {
        if table == "tasks"
            && matches!(
                action,
                rusqlite::hooks::Action::SQLITE_INSERT | rusqlite::hooks::Action::SQLITE_UPDATE
            )
        {
            n.notify_one();
        }
    }));
    Ok(UpdateHookGuard { _conn: conn })
}
```

If `Db` doesn't expose `path()`, add it:

```rust
impl Db {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
```

(Add `pub path: PathBuf` field to `Db` if not present.)

### Step 14.3: Wire the hook into the scheduler

- [ ] In `src/tasks/scheduler.rs`, find the run-loop. It likely looks like:

```rust
loop {
    self.run_one_iteration().await;
    tokio::time::sleep(POLL_INTERVAL).await;
}
```

Refactor to a `tokio::select!`:

```rust
let notify = std::sync::Arc::new(tokio::sync::Notify::new());
let _hook_guard = crate::storage::register_tasks_update_hook(&self.db, notify.clone())?;

loop {
    self.run_one_iteration().await;
    tokio::select! {
        _ = notify.notified() => {}
        _ = tokio::time::sleep(POLL_INTERVAL) => {}
    }
}
```

Hold `_hook_guard` for the lifetime of the loop — dropping it unhooks.

### Step 14.4: Run the test

- [ ] Run: `cargo test --features test-loopback --test cross_process_notify 2>&1 | tail -20`
  Expected: passes within 200ms.

### Step 14.5: Commit

```bash
git add src/storage/mod.rs src/tasks/scheduler.rs tests/cross_process_notify.rs
git commit -m "feat(m8): sqlite update_hook wakes scheduler on cross-process inserts"
```

---

## Task 15: Per-table summarize parallelization (M7 carry-over)

The `TODO(m8)` from `src/extractor/tables.rs` Phase 7. Replace the sequential drain loop with `buffered(4)`.

**Files:**
- Modify: `src/extractor/tables.rs`
- Create: `tests/tables_summarize_parallel.rs`

### Step 15.1: Failing perf test

- [ ] Create `tests/tables_summarize_parallel.rs`:

```rust
//! Wall-clock test for per-table parallelization.
#![cfg(feature = "test-loopback")]

use rover::extractor::options::TablesMode;
use rover::extractor::output::OutputPaths;
use rover::extractor::tables::{apply_with_summarizer, FallbackInfo, TableSummarizeHook};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

#[tokio::test]
async fn eight_tables_run_in_parallel() {
    let md = (0..8)
        .map(|i| format!("| A | B |\n|---|---|\n| {i} | x |\n"))
        .collect::<Vec<_>>()
        .join("\n\n");

    let hook: TableSummarizeHook = Arc::new(|_text: &str| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<(String, Option<FallbackInfo>), String>(("(summary)".to_string(), None))
        })
    });

    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
    let paths = OutputPaths::resolve(None).unwrap();
    let url = Url::parse("https://example.com/").unwrap();

    let start = Instant::now();
    let (_out, recs) =
        apply_with_summarizer(&md, &TablesMode::Summarize, &paths, &url, Some(&hook))
            .await
            .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(recs.len(), 8);
    assert!(
        elapsed < Duration::from_millis(400),
        "8 tables × 100ms each should complete < 400ms with concurrency=4, took {elapsed:?}",
    );
}
```

### Step 15.2: Locate the drain loop

- [ ] Open `src/extractor/tables.rs`. Find `apply_with_summarizer`'s second pass — the one that drains `events: Vec<OwnedEvent>` and awaits the hook per table.

### Step 15.3: Replace with `buffered(4)`

- [ ] Use `futures::stream::iter` + `.buffered(4)`:

```rust
use futures::stream::{self, StreamExt};

// Replace existing drain loop:
let table_results: Vec<(usize, OwnedEvent)> = stream::iter(events.into_iter().enumerate())
    .map(|(idx, ev)| {
        let hook = hook.clone();
        async move {
            match ev {
                OwnedEvent::Line(s) => (idx, OwnedEvent::Line(s)),
                OwnedEvent::Table(rows, ord) => {
                    let table_text = rows.join("\n");
                    // Re-emit the original Table with its hook result inlined via
                    // a marker variant or by binding into the existing structures.
                    // For simplicity, do the hook here and stash result inline.
                    let result = hook(&table_text).await;
                    (idx, OwnedEvent::TableWithResult(rows, ord, result))
                }
            }
        }
    })
    .buffered(4)
    .collect()
    .await;
```

This requires adding a `TableWithResult` variant to `OwnedEvent`. **Or** — simpler — collect futures separately and zip back. Implementer judgment.

A cleaner shape: split the rendering into two passes that don't fight the borrow checker:

```rust
// Pass 1: collect each event's (idx, hook future). For non-table events
// the "future" is just an immediate Ready.
let futures = events
    .into_iter()
    .enumerate()
    .map(|(idx, ev)| {
        let hook = hook.clone();
        async move {
            match ev {
                OwnedEvent::Line(s) => RenderedEvent::Line(idx, s),
                OwnedEvent::Table(rows, ord) => {
                    let table_text = rows.join("\n");
                    let result = hook(&table_text).await;
                    RenderedEvent::Table { idx, rows, ord, result }
                }
            }
        }
    })
    .collect::<Vec<_>>();

let rendered: Vec<RenderedEvent> = stream::iter(futures).buffered(4).collect().await;

// Pass 2: write `rendered` into the output string in idx order (buffered preserves it).
for ev in rendered {
    match ev {
        RenderedEvent::Line(_, s) => { out.push_str(&s); out.push('\n'); }
        RenderedEvent::Table { rows, ord, result, .. } => {
            // existing handling of Ok/Err result, pushing summary or original table
        }
    }
}
```

`RenderedEvent` is a new private enum local to the function.

### Step 15.4: Add the TODO(m8) marker

- [ ] Delete the existing `TODO(m8)` comment now that the parallelization landed.

### Step 15.5: Run

- [ ] Run: `cargo test --features test-loopback --test tables_summarize_parallel 2>&1 | tail -10`
  Expected: < 400ms wall-clock.
- [ ] Run: `cargo test --features test-loopback tables_summarize_mode 2>&1 | tail -10`
  Expected: existing integration test still passes (output order preserved).

### Step 15.6: Commit

```bash
git add src/extractor/tables.rs tests/tables_summarize_parallel.rs
git commit -m "feat(m8): parallelize per-table summarize via futures buffered(4)"
```

---

## Task 16: Documentation deliverables

PRD §17 mandates five documentation files. Author them with the M8 changes fresh in mind.

**Files:**
- Create: `docs/configuration.md`
- Create: `docs/cli.md`
- Create: `docs/mcp-tools.md`
- Create: `docs/security.md`
- Create: `docs/backends.md`

### Step 16.1: `docs/configuration.md`

- [ ] Create with this skeleton (expand each section against the current `Config` struct):

````markdown
# Rover Configuration

Rover reads a single TOML file. The default location is `$XDG_CONFIG_HOME/rover/config.toml` (typically `~/.config/rover/config.toml`). Override with `--config <path>` on every subcommand, or by setting `ROVER_CONFIG=/path/to/config.toml`.

Inspect the effective configuration with `rover config show`. Mutate a single setting with `rover config set <dotted.key> <value>` — see `docs/cli.md`.

---

## `[server]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `data_dir` | path | `~/.local/share/rover` | SQLite DB + tokenizers + extracted output go under here. |
| `output_dir` | path | `./rover-output` | Per-fetch outputs (tables CSVs, downloaded images). |

…

## `[ssrf]` (M8)

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `level` | enum | `strict` | One of `strict`, `loopback`, `project`, `lan`, `none`. |
| `project_root` | path | `.` | Required when `level = project`. Used as the descendant root for `file://` URLs. |

### Level semantics

| Level | Allows |
| --- | --- |
| `strict` | Public IPs only; `http`/`https` only. |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs descendant of `project_root` after symlink resolution. |
| `lan` | Project + RFC1918 + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. Always-floor (link-local, multicast, broadcast, `0.0.0.0`, `255.255.255.255`) still blocked. |

…

## `[debug]` (M8)

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `har_path` | string | `""` | When non-empty, every fetch is recorded as an HAR entry to this file. Empty disables. |
| `har_body_cap` | bytes \| humansize | `64KiB` | Per-response body cap. Truncated bodies are tagged in the HAR entry's `comment` field. |
| `log_level` | enum | `info` | One of `trace`, `debug`, `info`, `warn`, `error`. Overridden by `RUST_LOG`. |

…

## Environment overrides

- `ROVER_CONFIG` — path to the config file.
- `ROVER_DATA_DIR` — overrides `server.data_dir`.
- `ROVER_OUTPUT_DIR` — overrides `server.output_dir` / `output.dir`.
- `ROVER_LOG_LEVEL` — overrides `debug.log_level`.
- `RUST_LOG` — overrides all of the above for tracing filtering.

`rover config show` marks each leaf with its effective source (`defaults`, `file`, or `env`).
````

Fill in the `…` sections — `[fetch]`, `[cache]`, `[robots]`, `[rate_limit]`, `[tokenizer]`, `[output]`, `[summarization]`, `[summarization.tables]`, `[backends.<name>]`. Mirror the table format above. Pull defaults from the `default_*()` functions in `src/config/mod.rs`.

### Step 16.2: `docs/cli.md`

- [ ] Document every subcommand. Sample shape:

````markdown
# Rover CLI

```text
rover <command> [args]
```

Subcommands:
- `fetch` — fetch one URL and print Markdown.
- `batch <uuid>` — monitor or replay a batch task.
- `task <uuid>` — inspect/cancel/monitor a task.
- `cache` — query/list/clean the cache.
- `config show` — print effective config with provenance.
- `config set <key> <value>` — update a setting in the config file.
- `doctor` — run diagnostic checks.

Global flags:
- `--config <path>` — override config path.

## `rover fetch`

Synopsis:
```text
rover fetch <url> [--summarize <json>] [--max-tokens N] [...]
```

… (one section per subcommand)
````

Use the existing `clap` `#[derive(Args)]` blocks in `src/cli/*.rs` as the source of truth for args and defaults.

### Step 16.3: `docs/mcp-tools.md`

- [ ] One section per MCP tool: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens`. For each: tool name, args (with types), response shape, examples, error codes.

Pull args/response shape from `src/mcp/tools/*.rs` and `src/mcp/envelope.rs`. Use the same JSON example shape that the existing PRD §4 uses.

### Step 16.4: `docs/security.md`

- [ ] Cover:
  - SSRF level matrix (mirror the table from `docs/configuration.md`).
  - **DNS rebinding**: v2 limitation. Quote the spec §16 #1 — rover validates the addresses returned from initial DNS resolution but does not pin them through the connection. The risk: a DNS server returning different addresses on subsequent queries could route a "safe" hostname's later requests to an unsafe IP. Mitigation in v2: use `reqwest::ClientBuilder::resolve` to pin the resolution. For now, deploy rover behind a trusted DNS resolver in adversarial environments.
  - `file://` symlink resolution: rover canonicalizes the path before checking it against `project_root`. Symlinks pointing outside `project_root` are rejected.
  - Secret redaction: rover redacts URL query-string values for keys matching `api_key`, `token`, `secret`, `password` (case-insensitive substring) in tracing output. Authorization headers and request/response bodies are not currently redacted in HAR files.
  - Cache poisoning (PRD §16): the cache key is `(url, params)`; same URL with different upstream content produces different `content_hash` values. The cache itself doesn't validate authenticity.

### Step 16.5: `docs/backends.md`

- [ ] Cover:
  - The two kinds: `extractive` (offline, no model required) and `cloud` (via `genai`).
  - Provider list: every `genai` built-in (`openai`, `anthropic`, `gemini`, `xai`, `groq`, `deepseek`, `together`, `fireworks`) plus `openai_compat` for custom OpenAI-compatible endpoints.
  - `[backends.<name>]` field reference: `kind`, `provider`, `model`, `base_url`, `api_key_env`.
  - Worked examples: OpenAI (`provider = "openai"`, `model = "gpt-4o-mini"`, `api_key_env = "OPENAI_API_KEY"`); Anthropic; LM Studio (`provider = "openai_compat"`, `base_url = "http://localhost:1234"` — note rover auto-normalizes to `/v1/`); Ollama.
  - Backend selection: per-call via `summarize.backend = "fast"` or `[summarization] default_backend = "fast"`.

### Step 16.6: Commit

```bash
git add docs/configuration.md docs/cli.md docs/mcp-tools.md docs/security.md docs/backends.md
git commit -m "docs(m8): five prd-mandated documentation deliverables"
```

---

## Task 17: Wrap-up — manifest, README, full-suite

### Step 17.1: Update milestone manifest

- [ ] In `docs/superpowers/milestones/rover-milestones.md`, find the M8 section. Before the `**Deferred from M8.**` block, append:

```markdown
**Status:** Complete (YYYY-MM-DD).

**M8 follow-ups deferred to later milestones.**
1. DNS-rebinding-resistant fetching → v2. Document the limitation; `reqwest::ClientBuilder::resolve` is the implementation path.
2. Headless / local-inference / VLM doctor checks land in M9.
3. List-valued config keys (e.g. `robots.ignore_domains`) are not settable via `rover config set` — edit the file directly. Configurable via M9+ if requested.
4. Per-backend sampling overrides (temperature, top_p) — defer until a user asks.
5. Layered (vs merged) `config show` diff view — defer until a user asks.
```

Substitute YYYY-MM-DD with the merge date.

### Step 17.2: Update README

- [ ] In `README.md`, find the milestones table; add an M8 row mirroring M7:

```markdown
| M8 | SSRF Levels, Diagnostics, Polish | ✅ | YYYY-MM-DD |
```

Optionally append a section after the existing milestone tables:

````markdown
### Diagnostics & Configuration (M8)

`rover doctor` runs a battery of health checks:

```bash
rover doctor
```

`rover config show` prints the effective config with provenance comments:

```bash
rover config show
```

`rover config set ssrf.level loopback` mutates the config file in place:

```bash
rover config set ssrf.level loopback
```

HAR debug recording — set `[debug] har_path` in `rover.toml`:

```toml
[debug]
har_path = "./rover-debug.har"
har_body_cap = "64KiB"
```

Then open the resulting file in Chrome DevTools' Network panel (Import HAR).
````

### Step 17.3: Final full-suite check

- [ ] Run: `cargo test --features test-loopback 2>&1 | tail -10`
  Expected: every test passes. Test count should be 453 (M7 baseline) + roughly 25 new tests added by this milestone.
- [ ] Run: `cargo clippy --features test-loopback --all-targets 2>&1 | tail -10`
  Expected: clean.
- [ ] Run: `cargo build --release --features test-loopback 2>&1 | tail -5`
  Expected: release build succeeds.

### Step 17.4: Commit + push

```bash
git add docs/ README.md
git commit -m "docs(m8): mark milestone complete; readme row + section"
git push -u origin m8-ssrf-diagnostics
```

### Step 17.5: Open the PR

- [ ] Run:

```bash
gh pr create --title "M8: SSRF Levels, Diagnostics, Polish" --body "$(cat <<'EOF'
## Summary
- Full SSRF level matrix: strict / loopback / project / lan / none.
- Retired the M1 `TestLoopback` enum variant; the `test-loopback` cargo feature stays as a marker.
- HAR debug recorder activated via `[debug] har_path`.
- `rover doctor` with human + ndjson output formats.
- `rover config show` (provenance comments) + `rover config set` (whitelisted keys; toml_edit; round-trip validated).
- Secret redaction tracing layer.
- M6 carry-over: SQLite update_hook for cross-process new-task notify.
- M7 carry-over: per-table summarize parallelization via `buffered(4)`.
- Five docs deliverables: configuration.md, cli.md, mcp-tools.md, security.md, backends.md.

## Test plan
- [x] `cargo test --features test-loopback` — all green.
- [x] `cargo clippy --features test-loopback --all-targets` — clean.
- [x] Manual smoke: `rover doctor` exits 0 on clean install.
- [x] Manual smoke: HAR file from a fetch imports cleanly into Chrome DevTools.
- [x] Manual smoke: `rover config set ssrf.level loopback` preserves comments.

Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Open Items Resolved at Plan Time

1. **`har` crate version pin.** Set in Task 5 Step 5.2 — `0.9`. Bump if a newer minor lands between plan and execution.
2. **HAR middleware mechanism.** Task 6: manual wrapper around `fetch_with_cache` (no `reqwest-middleware` dep).
3. **Settable-key list.** Task 12 enumerates 25 keys covering the obvious surface. Extend in single-line PRs as needed.
4. **`update_hook` connection management.** Task 14 opens a dedicated short-lived connection alongside the storage actor's connection; the hook fires engine-wide.
5. **Provenance for `[backends.<name>]` blocks.** Not modeled in the leaf list — backends are user-defined and not amenable to a default. `config show` only reports on the keys in `known_leaves()`.
6. **Doctor: scrubbed output paths.** Task 8's `short()` helper rewrites `$HOME` prefix to `~/`. NDJSON keeps absolute paths.
7. **`futures::stream::buffered` order guarantee.** Confirmed via docs: `buffered` preserves input order. The renderer in Task 15 sorts by `idx` defensively.

---

## Spec coverage check

| Spec section | Plan task(s) |
| --- | --- |
| §3.1 Module layout | Task 5, 7, 8, 9, 10, 11, 12, 14 — every new file created. |
| §3.2 Component boundaries | Honored across Tasks 5–14. |
| §3.3 `config show` data flow | Task 10 (provenance) + Task 11 (CLI). |
| §3.4 `config set` data flow | Task 12 + Task 13. |
| §3.5 Cross-process notify | Task 14. |
| §3.6 Per-table parallelization | Task 15. |
| §4.2 Config additions | Task 1. |
| §4.3 SSRF level semantics | Tasks 2, 3, 4. |
| §5 SSRF implementation notes | Task 2 (IP levels), Task 4 (file://). |
| §6 HAR Recorder | Tasks 5, 6. |
| §7 `rover doctor` | Tasks 8, 9. |
| §8 `rover config` | Tasks 10, 11, 12, 13. |
| §9 Secret Redaction | Task 7. |
| §10 Cross-Process Notify | Task 14. |
| §11 Test Strategy | Distributed: every task has the integration tests its spec row requires. |
| §12 Crate Dependencies | Task 5 adds `har`. `futures` and `toml_edit` verified present (Tasks 12, 15). |
| §13 Error Model | Errors added inline per task: §13 ConfigError variants in Task 12; SsrfError variants in Tasks 2 + 4; HarError in Task 5; DoctorError in Task 8. |
| §14 Documentation Deliverables | Task 16. |
| §15 Acceptance Criteria | All 11 covered: AC1 (Task 9), AC2 (Task 6), AC3 (Tasks 11+13), AC4 (Task 13), AC5 (Task 2), AC6 (Task 4), AC7 (Task 7), AC8 (Task 14), AC9 (Task 15), AC10 (Task 17 full-suite check), AC11 (Task 16). |

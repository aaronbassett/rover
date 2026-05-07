# Rover M1 — Single-URL Fetch Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `rover fetch <url>` end-to-end: fetch the URL, detect charset, decode to UTF-8, extract main content via `readabilityrs`, wrap with a YAML frontmatter envelope, print Markdown to stdout. No cache, no MCP server, no batching — that's M2 and M3.

**Architecture:** Single binary crate `rover` with module layout per the design supplement §5. `tokio` async throughout. `reqwest` with `rustls-tls`. Charset pipeline runs on raw bytes before handing decoded UTF-8 to `readabilityrs`. SSRF enforcement runs at the URL/IP level before any connection. Frontmatter is hand-rolled (we only emit a fixed schema, never parse it). Logging via `tracing` to stderr.

**Tech Stack:** Rust 2024 edition. `tokio`, `reqwest` (rustls), `url`, `encoding_rs`, `chardetng`, `scraper`, `clap` (derive), `toml`, `sha2`, `jiff`, `serde`, `tracing` + `tracing-subscriber`, `readabilityrs`, `regex`. Dev: `wiremock`, `assert_cmd`, `predicates`.

**Scope of this plan:** PRD milestone M1 only. Subsequent milestones (M2 caching, M3 MCP server, etc.) get their own plans once M1 lands.

**References:**
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md`
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md`

---

## Files Created in This Plan

```
Cargo.toml                          # crate manifest
.gitignore                          # extend with /target etc. (already present, may extend)
src/main.rs                         # clap entry, dispatch
src/lib.rs                          # public module roots
src/telemetry.rs                    # tracing-subscriber init
src/error.rs                        # crate-wide error type
src/config.rs                       # TOML loading, M1 subset
src/cli/mod.rs                      # cli module root
src/cli/fetch.rs                    # `rover fetch` body
src/fetcher/mod.rs                  # fetcher integration entry
src/fetcher/client.rs               # reqwest::Client builder
src/fetcher/charset.rs              # PRD §5.1 detection pipeline
src/fetcher/ssrf.rs                 # SSRF policy enforcement
src/fetcher/canonical.rs            # canonical URL extraction
src/extractor/mod.rs                # extractor module root
src/extractor/pipeline.rs           # readabilityrs wrapper
src/extractor/frontmatter.rs        # YAML envelope writer (hand-rolled)
tests/cli_fetch.rs                  # end-to-end CLI test
tests/fetcher_integration.rs        # fetcher against wiremock
README.md                           # quickstart (already present, replace)
```

Inline unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each source file. Cross-module integration tests live in `tests/`.

---

## Task 1: Project scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Modify: `.gitignore` (extend if needed)

This task lays down the crate manifest, a hello-world `main.rs`, an empty `lib.rs` wired up so all subsequent modules attach there, and a passing baseline `cargo build`.

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "rover"
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
description = "An MCP server for fetching and prepping web content for LLM agents."
repository = "https://github.com/aaronbassett/rover"
readme = "README.md"
rust-version = "1.85"

[lib]
name = "rover"
path = "src/lib.rs"

[[bin]]
name = "rover"
path = "src/main.rs"

[dependencies]
anyhow = "1"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "fs", "signal"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls-tls", "stream", "charset"] }
url = "2"
encoding_rs = "0.8"
chardetng = "1"
scraper = "0.26"
clap = { version = "4", features = ["derive"] }
toml = "1"
sha2 = "0.11"
jiff = { version = "0.2", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
readabilityrs = "0.1"
regex = "1"

[dev-dependencies]
wiremock = "0.6"
assert_cmd = "2"
predicates = "3"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "fs", "signal", "test-util"] }
```

- [ ] **Step 2: Create `src/lib.rs`**

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
pub mod telemetry;
```

- [ ] **Step 3: Create `src/main.rs`**

```rust
fn main() {
    println!("rover scaffold ok");
}
```

This temporary entry point goes away in Task 2.

- [ ] **Step 4: Stub the modules referenced by `lib.rs`**

`lib.rs` declares modules that don't exist yet. Create empty stubs so the build passes; later tasks fill them in.

```rust
// src/cli/mod.rs
//! CLI command implementations.
```

```rust
// src/config.rs
//! Configuration loading.
```

```rust
// src/error.rs
//! Crate-wide error type.
```

```rust
// src/extractor/mod.rs
//! Content extraction pipeline.
```

```rust
// src/fetcher/mod.rs
//! HTTP fetching, charset detection, SSRF enforcement.
```

```rust
// src/telemetry.rs
//! Tracing initialization.
```

- [ ] **Step 5: Run `cargo build`**

```bash
cargo build
```

Expected: compiles with warnings about unused modules; no errors.

- [ ] **Step 6: Run `cargo run`**

```bash
cargo run
```

Expected output: `rover scaffold ok`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "feat(scaffold): initial crate manifest and module stubs"
```

---

## Task 2: Telemetry initialization

**Files:**
- Modify: `src/telemetry.rs` (replace stub)

A small `init()` that configures `tracing-subscriber` to write to stderr with `EnvFilter`. Used by `main` once we add it.

- [ ] **Step 1: Write the failing test**

Append to `src/telemetry.rs`:

```rust
//! Tracing initialization.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// Initialize tracing.
///
/// Writes structured logs to stderr. Honors the `RUST_LOG` env var; falls back
/// to `default_filter` (typically "info,rover=debug") when unset.
///
/// Calling this more than once in the same process is a no-op (subsequent
/// calls return without re-initializing).
pub fn init(default_filter: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    let layer = fmt::layer().with_writer(std::io::stderr).with_target(true);

    // try_init: if already initialized (e.g. in tests), this is a no-op.
    let _ = tracing_subscriber::registry().with(filter).with(layer).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init("info");
        init("debug");
        // No assertion needed: the second call must not panic.
    }
}
```

- [ ] **Step 2: Run the test to verify it passes (first call) or compiles**

```bash
cargo test --lib telemetry
```

Expected: 1 test passed (`init_is_idempotent`).

- [ ] **Step 3: Commit**

```bash
git add src/telemetry.rs
git commit -m "feat(telemetry): tracing-subscriber init to stderr"
```

---

## Task 3: Crate-wide error type

**Files:**
- Modify: `src/error.rs` (replace stub)

Establishes the `Error` and `Result` aliases used across the binary. We use `thiserror` per module for variant errors and re-export a top-level `Error` that wraps them. Per the design supplement §4.4, `anyhow` is reserved for the binary boundary; library code returns `crate::Result<T>`.

- [ ] **Step 1: Write the type**

Replace `src/error.rs`:

```rust
//! Crate-wide error type.
//!
//! Per design supplement §4.4: per-module error enums via `thiserror`,
//! `anyhow` only at the binary boundary. This `Error` enum is the
//! library-facing top-level type that wraps domain-specific errors.

use thiserror::Error;

/// Top-level error.
#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] crate::fetcher::FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] crate::extractor::ExtractorError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

This will not compile yet — `ConfigError`, `FetcherError`, `ExtractorError` don't exist. We'll add them in their respective modules in subsequent tasks. **For now**, comment out the variants that don't exist:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    // Variants are added as their respective modules introduce error types.
    // See tasks 4 (Config), 5+ (Fetcher), 9+ (Extractor).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 2: Run `cargo build`**

```bash
cargo build
```

Expected: compiles cleanly. (We'll add the wrapped variants as their modules land.)

- [ ] **Step 3: Commit**

```bash
git add src/error.rs
git commit -m "feat(error): crate-wide Error/Result with placeholder variants"
```

---

## Task 4: Config loader (M1 subset)

**Files:**
- Modify: `src/config.rs` (replace stub)
- Modify: `src/error.rs` (un-comment Config variant)

Loads a TOML config from `--config <path>` or returns defaults. M1 only consumes `fetch.user_agent` and `fetch.timeout`. Future milestones will extend the schema.

- [ ] **Step 1: Write the failing tests**

Replace `src/config.rs`:

```rust
//! Configuration loading.
//!
//! M1 covers a tiny subset of the full schema documented in PRD §12.
//! Subsequent milestones extend this struct.

use std::path::Path;
use std::time::Duration;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Read { path: String, source: std::io::Error },

    #[error("failed to parse config at {path}: {source}")]
    Parse { path: String, source: toml::de::Error },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub fetch: FetchConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self { fetch: FetchConfig::default() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchConfig {
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds. Stored as u64 for TOML friendliness.
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
    format!("Rover/{} (+https://github.com/aaronbassett/rover)", env!("CARGO_PKG_VERSION"))
}

fn default_timeout_secs() -> u64 {
    15
}

/// Load config. If `path` is provided, the file must exist and parse cleanly.
/// If `path` is None, return defaults.
pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
    let Some(path) = path else { return Ok(Config::default()); };

    let bytes = std::fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.display().to_string(), source })?;
    let cfg: Config = toml::from_str(&bytes)
        .map_err(|source| ConfigError::Parse { path: path.display().to_string(), source })?;
    Ok(cfg)
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
    }

    #[test]
    fn load_with_no_path_returns_default() {
        let cfg = load(None).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, 15);
    }

    #[test]
    fn load_from_file_overrides_defaults() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, r#"
[fetch]
user_agent = "test-ua"
timeout_secs = 5
"#).unwrap();

        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.fetch.user_agent, "test-ua");
        assert_eq!(cfg.fetch.timeout_secs, 5);
    }

    #[test]
    fn load_missing_file_errors() {
        let result = load(Some(Path::new("/no/such/path/__rover_test__.toml")));
        assert!(matches!(result, Err(ConfigError::Read { .. })));
    }

    #[test]
    fn load_malformed_toml_errors() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "not = valid = toml").unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn load_unknown_field_errors() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, r#"
[fetch]
unknown_field = "x"
"#).unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }
}
```

Add `tempfile` to dev-dependencies in `Cargo.toml`:

```toml
[dev-dependencies]
# ... existing entries above
tempfile = "3"
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib config
```

Expected: tests don't compile yet because `ConfigError` was just introduced and `crate::error::Error` doesn't yet wrap it. Fix by un-commenting the Config variant in `src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 3: Run the tests to verify they pass**

```bash
cargo test --lib config
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/error.rs
git commit -m "feat(config): TOML loader for M1 fetch subset"
```

---

## Task 5: SSRF strict-level enforcement

**Files:**
- Create: `src/fetcher/ssrf.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod ssrf;`, define `FetcherError`)
- Modify: `src/error.rs` (un-comment Fetcher variant)

For M1 we only implement the `Strict` SSRF level: rejects non-public IPs, non-`http(s)` schemes. Levels `loopback`, `project`, `lan`, `none` come in later milestones. Per design supplement §2.4, DNS rebinding hardening is deferred — we validate the IPs returned by initial resolution and accept the TOCTOU window.

- [ ] **Step 1: Write the test**

Create `src/fetcher/ssrf.rs`:

```rust
//! SSRF policy enforcement.
//!
//! M1 implements only the `Strict` level (PRD §5.5):
//! - Public IPs only (no loopback, no private, no link-local, no multicast,
//!   no broadcast, no unspecified)
//! - `http://` or `https://` schemes only
//!
//! Per design supplement §2.4, DNS-rebinding-resistant fetching is deferred
//! to v2: we validate the addresses returned from initial resolution but do
//! not pin them through the connection.

use std::net::IpAddr;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfLevel {
    /// Public IPs only, http/https only.
    Strict,
    // loopback, project, lan, none — added in later milestones.
}

#[derive(Debug, Error)]
pub enum SsrfError {
    #[error("scheme `{scheme}` is not allowed (Strict level requires http or https)")]
    Scheme { scheme: String },

    #[error("URL has no host")]
    NoHost,

    #[error("address {address} is not allowed under SSRF level {level:?} ({reason})")]
    Address { address: IpAddr, level: SsrfLevel, reason: &'static str },
}

/// Validate the URL itself (scheme, presence of host).
///
/// Call this *before* DNS resolution — it's cheap and rules out bad URLs early.
pub fn validate_url(url: &Url, level: SsrfLevel) -> Result<(), SsrfError> {
    match level {
        SsrfLevel::Strict => match url.scheme() {
            "http" | "https" => {}
            other => return Err(SsrfError::Scheme { scheme: other.to_string() }),
        },
    }
    if url.host_str().is_none() {
        return Err(SsrfError::NoHost);
    }
    Ok(())
}

/// Validate every resolved address against the policy.
///
/// Pass the `IpAddr`s returned from a DNS lookup. If *any* address violates
/// the policy, this returns an error and the request must not proceed.
pub fn validate_addresses(addrs: &[IpAddr], level: SsrfLevel) -> Result<(), SsrfError> {
    for &addr in addrs {
        if let Some(reason) = strict_reject_reason(addr) {
            if matches!(level, SsrfLevel::Strict) {
                return Err(SsrfError::Address { address: addr, level, reason });
            }
        }
    }
    Ok(())
}

fn strict_reject_reason(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback()         { return Some("loopback IPv4"); }
            if v4.is_private()          { return Some("private IPv4 (RFC1918)"); }
            if v4.is_link_local()       { return Some("link-local IPv4"); }
            if v4.is_multicast()        { return Some("multicast IPv4"); }
            if v4.is_broadcast()        { return Some("broadcast IPv4"); }
            if v4.is_unspecified()      { return Some("unspecified IPv4 (0.0.0.0)"); }
            // 100.64.0.0/10 — CGN. Not in std as a method, check by hand.
            let octets = v4.octets();
            if octets[0] == 100 && (octets[1] & 0xC0) == 0x40 {
                return Some("carrier-grade NAT (100.64.0.0/10)");
            }
            None
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback()         { return Some("loopback IPv6"); }
            if v6.is_multicast()        { return Some("multicast IPv6"); }
            if v6.is_unspecified()      { return Some("unspecified IPv6 (::)"); }
            // Unique local fc00::/7
            let segs = v6.segments();
            if (segs[0] & 0xfe00) == 0xfc00 { return Some("unique-local IPv6 (fc00::/7)"); }
            // Link-local fe80::/10
            if (segs[0] & 0xffc0) == 0xfe80 { return Some("link-local IPv6 (fe80::/10)"); }
            // IPv4-mapped/embedded — reject too; check by mapping back
            if let Some(v4) = v6.to_ipv4_mapped() {
                if let Some(reason) = strict_reject_reason(IpAddr::V4(v4)) {
                    return Some(reason);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn http_https_allowed_strict() {
        assert!(validate_url(&Url::parse("http://example.com/").unwrap(), SsrfLevel::Strict).is_ok());
        assert!(validate_url(&Url::parse("https://example.com/").unwrap(), SsrfLevel::Strict).is_ok());
    }

    #[test]
    fn file_scheme_rejected_strict() {
        let err = validate_url(&Url::parse("file:///etc/passwd").unwrap(), SsrfLevel::Strict).unwrap_err();
        assert!(matches!(err, SsrfError::Scheme { .. }));
    }

    #[test]
    fn ftp_scheme_rejected_strict() {
        let err = validate_url(&Url::parse("ftp://example.com/").unwrap(), SsrfLevel::Strict).unwrap_err();
        assert!(matches!(err, SsrfError::Scheme { .. }));
    }

    #[test]
    fn loopback_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn private_rejected_strict() {
        for addr in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
        ] {
            assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err(), "{addr}");
        }
    }

    #[test]
    fn link_local_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv6_loopback_rejected_strict() {
        assert!(validate_addresses(&[IpAddr::V6(Ipv6Addr::LOCALHOST)], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv6_ula_rejected_strict() {
        let addr: IpAddr = "fd00::1".parse().unwrap();
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv4_mapped_loopback_rejected_strict() {
        let addr: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn cgn_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn public_ipv4_allowed_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_ok());
    }

    #[test]
    fn any_violator_in_set_rejects() {
        let addrs = [
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ];
        assert!(validate_addresses(&addrs, SsrfLevel::Strict).is_err());
    }
}
```

- [ ] **Step 2: Wire `ssrf` into `fetcher/mod.rs` and define `FetcherError`**

Replace `src/fetcher/mod.rs`:

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod ssrf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),
    // More variants added in subsequent tasks.
}
```

- [ ] **Step 3: Un-comment Fetcher variant in `src/error.rs`**

Replace `src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] crate::fetcher::FetcherError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --lib fetcher::ssrf
```

Expected: 11 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/fetcher/ src/error.rs
git commit -m "feat(fetcher): SSRF Strict-level URL and address validation"
```

---

## Task 6: Charset detection pipeline

**Files:**
- Create: `src/fetcher/charset.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod charset;`)

Implements the PRD §5.1 pipeline: BOM → HTTP `Content-Type` → in-document `<meta>` scan → `chardetng` → UTF-8 fallback. Re-encodes the final output to UTF-8 with replacement characters.

- [ ] **Step 1: Write the failing tests**

Create `src/fetcher/charset.rs`:

```rust
//! Charset detection pipeline (PRD §5.1).
//!
//! Order:
//!   1. BOM (`encoding_rs::Encoding::for_bom`)
//!   2. HTTP `Content-Type` charset parameter (`Encoding::for_label`)
//!   3. ASCII-decode first 1024 bytes, regex-scan for `<meta charset=...>` /
//!      `<meta http-equiv="Content-Type" content="...; charset=...">`
//!   4. `chardetng::EncodingDetector::guess(None, true)`
//!   5. UTF-8 with replacement.
//!
//! `readabilityrs` accepts `&str`, so we always re-encode the final output to
//! UTF-8 here.

use chardetng::EncodingDetector;
use encoding_rs::{Encoding, UTF_8};
use regex::Regex;
use std::sync::LazyLock;

/// What sniffing approach picked the encoding. Useful for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
    Bom,
    HttpHeader,
    MetaTag,
    Chardetng,
    Fallback,
}

#[derive(Debug, Clone, Copy)]
pub struct Detected {
    pub encoding: &'static Encoding,
    pub source: DetectionSource,
}

/// Detect the source encoding for an HTTP response body.
pub fn detect_encoding(content_type: Option<&str>, bytes: &[u8]) -> Detected {
    // 1. BOM
    if let Some((enc, _bom_len)) = Encoding::for_bom(bytes) {
        return Detected { encoding: enc, source: DetectionSource::Bom };
    }

    // 2. HTTP Content-Type charset
    if let Some(ct) = content_type {
        if let Some(label) = parse_charset_param(ct) {
            if let Some(enc) = Encoding::for_label(label.as_bytes()) {
                return Detected { encoding: enc, source: DetectionSource::HttpHeader };
            }
        }
    }

    // 3. <meta> sniff in first 1024 bytes
    if let Some(enc) = sniff_meta_charset(bytes) {
        return Detected { encoding: enc, source: DetectionSource::MetaTag };
    }

    // 4. chardetng
    let mut det = EncodingDetector::new();
    det.feed(bytes, true);
    let enc = det.guess(None, true);
    if enc != UTF_8 || looks_like_utf8(bytes) {
        return Detected { encoding: enc, source: DetectionSource::Chardetng };
    }

    // 5. Fallback
    Detected { encoding: UTF_8, source: DetectionSource::Fallback }
}

/// Decode `bytes` to UTF-8 using the result of [`detect_encoding`].
///
/// Returns the decoded string and the detection result so callers can log
/// HTTP-vs-detected mismatches.
pub fn decode_to_utf8(content_type: Option<&str>, bytes: &[u8]) -> (String, Detected) {
    let detected = detect_encoding(content_type, bytes);
    let (cow, _enc_used, _had_errors) = detected.encoding.decode(bytes);
    (cow.into_owned(), detected)
}

/// Extract the charset parameter from a `Content-Type` header value.
fn parse_charset_param(header: &str) -> Option<String> {
    for part in header.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix_ignore_case("charset=") {
            return Some(strip_quotes(rest).to_string());
        }
    }
    None
}

trait StripPrefixIgnoreCase {
    fn strip_prefix_ignore_case<'a>(&'a self, prefix: &str) -> Option<&'a str>;
}

impl StripPrefixIgnoreCase for str {
    fn strip_prefix_ignore_case<'a>(&'a self, prefix: &str) -> Option<&'a str> {
        if self.len() < prefix.len() { return None; }
        let head = &self[..prefix.len()];
        if head.eq_ignore_ascii_case(prefix) {
            Some(&self[prefix.len()..])
        } else {
            None
        }
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && (s.starts_with('"') && s.ends_with('"')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Look for `<meta charset>` or `<meta http-equiv="Content-Type" ...>` in the
/// first 1024 bytes, ASCII-decoded.
fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    static META_CHARSET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?xi)
            <meta \s [^>]*?
            (?:
                charset \s* = \s* ["']? ([A-Za-z0-9_:.\-]+)
              | http-equiv \s* = \s* ["']? content-type ["']? \s [^>]*?
                content \s* = \s* ["']? [^"'>]*? charset \s* = \s* ([A-Za-z0-9_:.\-]+)
            )
        "#).unwrap()
    });

    let head_len = bytes.len().min(1024);
    let head: String = bytes[..head_len].iter()
        .map(|&b| if b.is_ascii() { b as char } else { ' ' })
        .collect();
    let caps = META_CHARSET.captures(&head)?;
    let label = caps.get(1).or_else(|| caps.get(2))?.as_str();
    Encoding::for_label(label.as_bytes())
}

/// Quick UTF-8 plausibility check used to disambiguate the chardetng default.
fn looks_like_utf8(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf8_bom() {
        let bytes = b"\xEF\xBB\xBFhello";
        let det = detect_encoding(None, bytes);
        assert_eq!(det.source, DetectionSource::Bom);
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn detects_utf16le_bom() {
        let bytes = b"\xFF\xFEh\x00i\x00";
        let det = detect_encoding(None, bytes);
        assert_eq!(det.source, DetectionSource::Bom);
    }

    #[test]
    fn detects_from_http_header() {
        let bytes = b"<html><body>caf\xE9</body></html>";
        let det = detect_encoding(Some("text/html; charset=ISO-8859-1"), bytes);
        assert_eq!(det.source, DetectionSource::HttpHeader);
        assert_eq!(det.encoding.name(), "windows-1252"); // encoding_rs maps Latin-1 -> windows-1252
    }

    #[test]
    fn detects_from_http_header_with_quotes() {
        let bytes = b"hello";
        let det = detect_encoding(Some(r#"text/html; charset="utf-8""#), bytes);
        assert_eq!(det.source, DetectionSource::HttpHeader);
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn detects_from_meta_charset() {
        let html = br#"<!doctype html><html><head><meta charset="Shift_JIS"></head>"#;
        let det = detect_encoding(None, html);
        assert_eq!(det.source, DetectionSource::MetaTag);
        assert_eq!(det.encoding.name(), "Shift_JIS");
    }

    #[test]
    fn detects_from_meta_http_equiv() {
        let html = br#"<html><head><meta http-equiv="Content-Type" content="text/html; charset=EUC-KR"></head>"#;
        let det = detect_encoding(None, html);
        assert_eq!(det.source, DetectionSource::MetaTag);
        assert_eq!(det.encoding.name(), "EUC-KR");
    }

    #[test]
    fn falls_back_to_chardetng_for_plain_utf8() {
        let bytes = "héllo wörld".as_bytes();
        let det = detect_encoding(None, bytes);
        assert!(matches!(det.source, DetectionSource::Chardetng | DetectionSource::Fallback));
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn header_overrides_meta() {
        // Header says UTF-8, meta says Shift_JIS — header wins.
        let html = br#"<html><head><meta charset="Shift_JIS"></head>"#;
        let det = detect_encoding(Some("text/html; charset=utf-8"), html);
        assert_eq!(det.source, DetectionSource::HttpHeader);
    }

    #[test]
    fn invalid_label_in_header_falls_through() {
        let html = br#"<html><head><meta charset="utf-8"></head>"#;
        let det = detect_encoding(Some("text/html; charset=not-a-real-charset"), html);
        assert_eq!(det.source, DetectionSource::MetaTag);
    }

    #[test]
    fn decode_round_trips_utf8() {
        let (out, det) = decode_to_utf8(Some("text/html; charset=utf-8"), "héllo".as_bytes());
        assert_eq!(out, "héllo");
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn decode_handles_latin1() {
        // 0xE9 is é in ISO-8859-1.
        let (out, _det) = decode_to_utf8(Some("text/html; charset=ISO-8859-1"), &[b'h', 0xE9, b'l', b'l', b'o']);
        assert_eq!(out, "héllo");
    }
}
```

- [ ] **Step 2: Add the module to `fetcher/mod.rs`**

Modify `src/fetcher/mod.rs` to include `pub mod charset;` alongside the existing `pub mod ssrf;`:

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod charset;
pub mod ssrf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo test --lib fetcher::charset
```

Expected: 11 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): charset detection pipeline (BOM/header/meta/chardetng)"
```

---

## Task 7: HTTP client builder

**Files:**
- Create: `src/fetcher/client.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod client;`, extend `FetcherError`)

Builds a `reqwest::Client` with our user-agent, timeout, redirect policy capped at 10. The actual fetch loop comes in Task 9.

- [ ] **Step 1: Write the test**

Create `src/fetcher/client.rs`:

```rust
//! HTTP client construction.

use std::time::Duration;
use reqwest::redirect::Policy;

/// Build a `reqwest::Client` configured for Rover's fetch defaults.
///
/// Per PRD §5.2: max 10 redirects.
pub fn build_http_client(user_agent: &str, timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .redirect(Policy::limited(10))
        .build()
        .expect("reqwest::Client::builder() should not fail with these defaults")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_with_defaults() {
        let _client = build_http_client("test/0.1", Duration::from_secs(15));
        // Mere fact of building without panicking is the assertion. reqwest's
        // builder can fail (e.g. when TLS backends are misconfigured), so we
        // exercise the path here.
    }
}
```

- [ ] **Step 2: Add to `fetcher/mod.rs`**

Modify `src/fetcher/mod.rs`:

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod charset;
pub mod client;
pub mod ssrf;

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

- [ ] **Step 3: Run the tests**

```bash
cargo test --lib fetcher::client
```

Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): reqwest client builder with redirect cap and timeout"
```

---

## Task 8: Canonical URL extraction

**Files:**
- Create: `src/fetcher/canonical.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod canonical;`)

Extracts the canonical URL from the response. Resolution order: HTML `<link rel="canonical">` (preferred), HTTP `Link` header `rel="canonical"` (fallback), final URL after redirects (default).

- [ ] **Step 1: Write the failing tests**

Create `src/fetcher/canonical.rs`:

```rust
//! Canonical URL extraction (PRD §5.2).
//!
//! Resolution order:
//!   1. HTML `<link rel="canonical" href="...">` in `<head>`
//!   2. HTTP `Link: <...>; rel="canonical"` header
//!   3. Final URL after redirects (the request's final response URL)
//!
//! The PRD warns that stale `rel=canonical` exists in the wild; we still
//! return whatever the source claims and let upstream code decide whether to
//! validate. M1 returns the claimed URL without further checks.

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

/// Extract a canonical URL from the response.
///
/// `final_url` is the URL after all redirects. `html` is the decoded body.
/// `link_header` is the raw `Link:` header value, if any.
pub fn extract_canonical_url(
    html: &str,
    final_url: &Url,
    link_header: Option<&str>,
) -> Url {
    if let Some(url) = canonical_from_html(html, final_url) {
        return url;
    }
    if let Some(header) = link_header {
        if let Some(url) = canonical_from_link_header(header, final_url) {
            return url;
        }
    }
    final_url.clone()
}

fn canonical_from_html(html: &str, base: &Url) -> Option<Url> {
    static SEL: LazyLock<Selector> = LazyLock::new(|| {
        Selector::parse(r#"link[rel~="canonical"][href]"#).unwrap()
    });
    let doc = Html::parse_document(html);
    let el = doc.select(&SEL).next()?;
    let href = el.value().attr("href")?;
    base.join(href).ok()
}

/// Parse RFC 8288 `Link` header values, looking for `rel="canonical"`.
///
/// We accept multiple comma-separated link-values and ignore unrelated rels.
fn canonical_from_link_header(header: &str, base: &Url) -> Option<Url> {
    for value in split_link_values(header) {
        let value = value.trim();
        let (target, params) = match value.split_once(';') {
            Some((t, p)) => (t.trim(), p),
            None => (value, ""),
        };
        let target = target.trim_start_matches('<').trim_end_matches('>');
        // Look for rel="canonical" in params, case-insensitive on `rel`.
        for raw_param in params.split(';') {
            let p = raw_param.trim();
            if let Some(rest) = strip_prefix_ci(p, "rel=") {
                let rest = rest.trim_matches('"');
                if rest.split_whitespace().any(|tok| tok.eq_ignore_ascii_case("canonical")) {
                    return base.join(target).ok();
                }
            }
        }
    }
    None
}

/// Split a Link header on top-level commas (commas inside `<...>` or `"..."`
/// don't count).
fn split_link_values(header: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = header.as_bytes();
    let mut start = 0usize;
    let mut depth_angle = 0i32;
    let mut in_quote = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'<' if !in_quote => depth_angle += 1,
            b'>' if !in_quote => depth_angle -= 1,
            b'"' => in_quote = !in_quote,
            b',' if !in_quote && depth_angle == 0 => {
                out.push(&header[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < header.len() { out.push(&header[start..]); }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() { return None; }
    if s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url { Url::parse(s).unwrap() }

    #[test]
    fn returns_final_url_when_no_signal() {
        let final_url = url("https://example.com/page?utm=x");
        let got = extract_canonical_url("<html></html>", &final_url, None);
        assert_eq!(got, final_url);
    }

    #[test]
    fn extracts_from_html_link_canonical() {
        let html = r#"<html><head><link rel="canonical" href="https://example.com/page"></head></html>"#;
        let got = extract_canonical_url(html, &url("https://example.com/page?utm=x"), None);
        assert_eq!(got, url("https://example.com/page"));
    }

    #[test]
    fn extracts_from_html_relative_canonical() {
        let html = r#"<html><head><link rel="canonical" href="/page"></head></html>"#;
        let got = extract_canonical_url(html, &url("https://example.com/page?utm=x"), None);
        assert_eq!(got, url("https://example.com/page"));
    }

    #[test]
    fn html_canonical_preferred_over_link_header() {
        let html = r#"<html><head><link rel="canonical" href="https://example.com/from-html"></head></html>"#;
        let got = extract_canonical_url(
            html,
            &url("https://example.com/x"),
            Some(r#"<https://example.com/from-header>; rel="canonical""#),
        );
        assert_eq!(got, url("https://example.com/from-html"));
    }

    #[test]
    fn extracts_from_link_header_when_no_html() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/canon>; rel="canonical""#),
        );
        assert_eq!(got, url("https://example.com/canon"));
    }

    #[test]
    fn link_header_with_multiple_rels() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/p>; rel="prev", <https://example.com/c>; rel="canonical""#),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn link_header_rel_case_insensitive() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/c>; REL="Canonical""#),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn link_header_with_compound_rel() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/c>; rel="alternate canonical""#),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn falls_back_when_link_header_has_no_canonical() {
        let final_url = url("https://example.com/x");
        let got = extract_canonical_url(
            "<html></html>",
            &final_url,
            Some(r#"<https://example.com/p>; rel="prev""#),
        );
        assert_eq!(got, final_url);
    }
}
```

- [ ] **Step 2: Add module to `fetcher/mod.rs`**

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

pub mod canonical;
pub mod charset;
pub mod client;
pub mod ssrf;

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

- [ ] **Step 3: Run the tests**

```bash
cargo test --lib fetcher::canonical
```

Expected: 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/fetcher/
git commit -m "feat(fetcher): canonical URL extraction from HTML and Link header"
```

---

## Task 9: Fetcher integration

**Files:**
- Create: `src/fetcher/fetch.rs`
- Modify: `src/fetcher/mod.rs` (add `pub mod fetch;` and re-export)
- Create: `tests/fetcher_integration.rs`

Wires SSRF + client + charset + canonical into one `fetch_url(...)` function. Uses `tokio::net::lookup_host` for DNS, validates each address against the SSRF policy, then issues the GET via `reqwest`. (DNS-rebinding hardening deferred per design supplement §2.4.)

- [ ] **Step 1: Write the integration test first**

Create `tests/fetcher_integration.rs`:

```rust
//! Integration tests for the fetch pipeline.

use std::time::Duration;
use rover::fetcher::{client::build_http_client, fetch::fetch_url, ssrf::SsrfLevel};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetches_simple_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><head><title>hi</title></head><body>hi there</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse(&format!("{}/article", server.uri())).unwrap();

    // Wiremock binds to 127.0.0.1, which Strict SSRF rejects. Use a level
    // that allows loopback for the integration test. (For M1 we only ship
    // Strict; tests that need loopback get a temporary "loopback ok" path
    // via an internal test API — see step 2.)
    let result = fetch_url(&client, &url, SsrfLevel::Strict).await;
    assert!(matches!(result, Err(rover::fetcher::FetcherError::Ssrf(_))));
}

#[tokio::test]
async fn rejects_private_ip_at_ssrf_check() {
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    // 10.0.0.1 is RFC1918 private. Even if DNS pointed there, Strict rejects.
    // Here we bypass DNS by using a literal IP URL.
    let url = Url::parse("http://10.0.0.1/").unwrap();
    let result = fetch_url(&client, &url, SsrfLevel::Strict).await;
    assert!(matches!(result, Err(rover::fetcher::FetcherError::Ssrf(_))));
}
```

The first test demonstrates that `Strict` blocks loopback (which `wiremock` binds to). To exercise the happy path against `wiremock` we need a way to permit loopback in tests. We add an internal `SsrfLevel::TestAllowLoopback` variant that's `#[cfg(test)]` only — keeps the production surface to `Strict` while still letting integration tests run.

- [ ] **Step 2: Add a test-only `Loopback` variant to `SsrfLevel`**

Modify `src/fetcher/ssrf.rs`. Replace the `SsrfLevel` enum and the two validation functions:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfLevel {
    /// Public IPs only, http/https only.
    Strict,

    /// **Test-only.** Strict + loopback. Used by integration tests against
    /// wiremock. Not exposed in the production CLI/config surface.
    #[cfg(any(test, feature = "test-loopback"))]
    TestLoopback,
}

pub fn validate_url(url: &Url, level: SsrfLevel) -> Result<(), SsrfError> {
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(SsrfError::Scheme { scheme: other.to_string() }),
    }
    if url.host_str().is_none() {
        return Err(SsrfError::NoHost);
    }
    let _ = level; // currently no scheme variation across levels
    Ok(())
}

pub fn validate_addresses(addrs: &[IpAddr], level: SsrfLevel) -> Result<(), SsrfError> {
    for &addr in addrs {
        let strict_reject = strict_reject_reason(addr);
        match level {
            SsrfLevel::Strict => {
                if let Some(reason) = strict_reject {
                    return Err(SsrfError::Address { address: addr, level, reason });
                }
            }
            #[cfg(any(test, feature = "test-loopback"))]
            SsrfLevel::TestLoopback => {
                if let Some(reason) = strict_reject {
                    if !addr.is_loopback() {
                        return Err(SsrfError::Address { address: addr, level, reason });
                    }
                }
            }
        }
    }
    Ok(())
}
```

Add the feature flag to `Cargo.toml`:

```toml
[features]
default = []
test-loopback = []
```

Update the integration test to use `TestLoopback`:

```rust
//! Integration tests for the fetch pipeline.

use std::time::Duration;
use rover::fetcher::{client::build_http_client, fetch::fetch_url, ssrf::SsrfLevel};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetches_simple_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><head><title>hi</title></head><body>hi there</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse(&format!("{}/article", server.uri())).unwrap();

    let page = fetch_url(&client, &url, SsrfLevel::TestLoopback).await.expect("fetch ok");
    assert_eq!(page.final_url.as_str(), url.as_str());
    assert!(page.body.contains("hi there"));
}

#[tokio::test]
async fn rejects_private_ip_at_ssrf_check() {
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse("http://10.0.0.1/").unwrap();
    let result = fetch_url(&client, &url, SsrfLevel::Strict).await;
    assert!(matches!(result, Err(rover::fetcher::FetcherError::Ssrf(_))));
}

#[tokio::test]
async fn follows_redirects_and_records_final_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", "/final"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>destination</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let start = Url::parse(&format!("{}/redirect", server.uri())).unwrap();
    let page = fetch_url(&client, &start, SsrfLevel::TestLoopback).await.expect("fetch ok");
    assert!(page.final_url.path().ends_with("/final"));
    assert_eq!(page.canonical_url.path(), "/final");
}

#[tokio::test]
async fn extracts_canonical_from_html() {
    let server = MockServer::start().await;
    let canonical = format!("{}/canon", server.uri());
    let html = format!(
        r#"<html><head><link rel="canonical" href="{}"></head><body>x</body></html>"#,
        canonical
    );
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(html)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse(&format!("{}/page", server.uri())).unwrap();
    let page = fetch_url(&client, &url, SsrfLevel::TestLoopback).await.expect("fetch ok");
    assert_eq!(page.canonical_url.as_str(), canonical);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test --test fetcher_integration
```

Expected: compile errors (`fetch::fetch_url` doesn't exist).

- [ ] **Step 4: Implement `src/fetcher/fetch.rs`**

```rust
//! End-to-end fetch: SSRF check → DNS validate → GET → charset decode.

use std::net::IpAddr;
use tokio::net::lookup_host;
use tracing::debug;
use url::Url;

use super::{
    canonical::extract_canonical_url,
    charset::{decode_to_utf8, Detected},
    ssrf::{self, SsrfLevel},
    FetcherError,
};

/// A successfully fetched page.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// URL after redirects.
    pub final_url: Url,

    /// Canonical URL — `<link rel="canonical">`, then `Link` header, else `final_url`.
    pub canonical_url: Url,

    /// HTTP status of the final response.
    pub status: u16,

    /// `Content-Type` header value, if any.
    pub content_type: Option<String>,

    /// Decoded UTF-8 body.
    pub body: String,

    /// Charset detection result, for diagnostics.
    pub charset: Detected,

    /// Raw `Link` header value, if present.
    pub link_header: Option<String>,

    /// Raw `ETag` header, if present.
    pub etag: Option<String>,

    /// Raw `Last-Modified` header, if present.
    pub last_modified: Option<String>,
}

/// Fetch `url` honoring the given SSRF level.
pub async fn fetch_url(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
) -> Result<FetchedPage, FetcherError> {
    ssrf::validate_url(url, level)?;
    let host = url.host_str().ok_or(FetcherError::Ssrf(ssrf::SsrfError::NoHost))?;
    let port = url.port_or_known_default().unwrap_or(0);

    // Resolve and validate. Note: this is best-effort — see design §2.4 about
    // the deferred TOCTOU/DNS-rebinding hardening.
    let addrs = resolve_host(host, port).await?;
    ssrf::validate_addresses(&addrs, level)?;

    let response = client.get(url.clone()).send().await?;
    let status = response.status().as_u16();
    let final_url = Url::parse(response.url().as_str())?;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let link_header = response
        .headers()
        .get(reqwest::header::LINK)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let bytes = response.bytes().await?;
    let (body, charset) = decode_to_utf8(content_type.as_deref(), &bytes);

    if let Some(ref ct) = content_type {
        if ct.to_ascii_lowercase().contains("charset=") {
            debug!(
                target: "rover::fetcher::charset",
                http_charset = ct.as_str(),
                detected = %charset.encoding.name(),
                "charset detection complete"
            );
        }
    }

    let canonical_url = extract_canonical_url(&body, &final_url, link_header.as_deref());

    Ok(FetchedPage {
        final_url,
        canonical_url,
        status,
        content_type,
        body,
        charset,
        link_header,
        etag,
        last_modified,
    })
}

async fn resolve_host(host: &str, port: u16) -> Result<Vec<IpAddr>, FetcherError> {
    let target = format!("{host}:{port}");
    let iter = lookup_host(target.as_str())
        .await
        .map_err(|e| FetcherError::Dns { host: host.to_string(), source: e })?;
    Ok(iter.map(|sa| sa.ip()).collect())
}
```

- [ ] **Step 5: Re-export in `fetcher/mod.rs`**

```rust
//! HTTP fetching, charset detection, SSRF enforcement.

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

- [ ] **Step 6: Enable the test-loopback feature for integration tests**

Update `Cargo.toml`:

```toml
[features]
default = []
test-loopback = []
```

Run integration tests with that feature on:

```bash
cargo test --features test-loopback --test fetcher_integration
```

Expected: 4 tests pass.

- [ ] **Step 7: Run all tests to confirm nothing else broke**

```bash
cargo test --features test-loopback
```

Expected: all unit tests + integration tests pass.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/fetcher/ tests/fetcher_integration.rs
git commit -m "feat(fetcher): end-to-end fetch wiring SSRF, DNS, charset, canonical"
```

---

## Task 10: Extractor (readabilityrs wrapper)

**Files:**
- Create: `src/extractor/pipeline.rs`
- Modify: `src/extractor/mod.rs` (add module, define `ExtractorError`)
- Modify: `src/error.rs` (un-comment Extractor variant)

Wraps `readabilityrs` with our preferred `MarkdownOptions`. Returns a `ExtractedDoc` with title, body Markdown, and any inline metadata `readabilityrs` already extracted (we pull a few stable fields; full metadata extraction is M4).

- [ ] **Step 1: Write the failing tests**

Create `src/extractor/pipeline.rs`:

```rust
//! Content extraction pipeline (PRD §6.1).
//!
//! `bytes → charset_detect → utf8 → readabilityrs → markdown_postprocess`.
//!
//! Charset detection is the fetcher's job (see `fetcher::charset`); this
//! module receives a UTF-8 string and returns markdown.

use readabilityrs::{
    MarkdownOptions, Readability, ReadabilityOptions,
    markdown::options::{HeadingStyle, LinkStyle},
};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ExtractorError {
    #[error("readabilityrs: {0}")]
    Readability(String),

    #[error("readabilityrs returned no article")]
    NoArticle,
}

/// Successfully extracted article.
#[derive(Debug, Clone)]
pub struct ExtractedDoc {
    pub title: Option<String>,
    pub body_md: String,
    pub language: Option<String>,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub site_name: Option<String>,
    pub published_time: Option<String>,
    pub image: Option<String>,
}

/// Build the markdown options Rover prefers (PRD §6.1: ATX headings, backtick
/// fences, dash bullets, inline links).
fn rover_markdown_options() -> MarkdownOptions {
    MarkdownOptions {
        heading_style: HeadingStyle::Atx,
        bullet_char: '-',
        code_fence: '`',
        emphasis_delimiter: '*',
        strong_delimiter: "**".to_string(),
        link_style: LinkStyle::Inline,
        preserve_complex_tables: true,
    }
}

/// Extract the article from `html`, resolving relative links against `base_url`.
pub fn extract(html: &str, base_url: Option<&Url>) -> Result<ExtractedDoc, ExtractorError> {
    let opts = ReadabilityOptions::builder()
        .output_markdown(true)
        .markdown_options(rover_markdown_options())
        .build();

    let url_str = base_url.map(|u| u.as_str().to_string());
    let readability = Readability::new(html, url_str.as_deref(), Some(opts))
        .map_err(|e| ExtractorError::Readability(e.to_string()))?;

    let article = readability.parse().ok_or(ExtractorError::NoArticle)?;

    let body_md = article.markdown_content.unwrap_or_default();

    Ok(ExtractedDoc {
        title: article.title,
        body_md,
        language: article.lang,
        byline: article.byline,
        excerpt: article.excerpt,
        site_name: article.site_name,
        published_time: article.published_time,
        image: article.image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_HTML: &str = r#"
<!doctype html>
<html lang="en">
<head><title>Sample Article</title></head>
<body>
  <article>
    <h1>How to do the thing</h1>
    <p>This is a long paragraph of body content. It needs to be substantial enough that
       readabilityrs identifies it as the article. Otherwise the extractor will fall back
       to no-article, which is what we want to avoid in this test. The content has to
       cross the default character threshold of 500 characters, so we need a few sentences
       of filler. Here is more filler. Lorem ipsum dolor sit amet, consectetur adipiscing
       elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>
    <p>Second paragraph with a <a href="/relative">relative link</a> and a <a href="https://example.com/abs">absolute link</a>.</p>
  </article>
</body>
</html>
"#;

    #[test]
    fn extracts_title_and_body() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        assert!(doc.title.unwrap().contains("Sample Article")
            || doc.body_md.contains("How to do the thing"));
        assert!(doc.body_md.contains("How to do the thing"));
        assert!(doc.body_md.contains("filler"));
    }

    #[test]
    fn produces_atx_headings() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        // ATX heading is `# Heading`, not the Setext underline form.
        assert!(doc.body_md.contains("# How to do the thing"));
    }

    #[test]
    fn captures_language() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        assert_eq!(doc.language.as_deref(), Some("en"));
    }
}
```

- [ ] **Step 2: Modify `src/extractor/mod.rs`**

```rust
//! Content extraction pipeline.

pub mod frontmatter;
pub mod pipeline;

pub use pipeline::{ExtractedDoc, ExtractorError, extract};
```

The `frontmatter` module is added in the next task; declaring it here keeps imports tidy. Create an empty stub `src/extractor/frontmatter.rs` to satisfy the compile:

```rust
// src/extractor/frontmatter.rs
//! YAML frontmatter envelope writer.
```

- [ ] **Step 3: Un-comment Extractor variant in `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] crate::fetcher::FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] crate::extractor::ExtractorError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --lib extractor
```

Expected: 3 tests pass.

> **Note:** if `readabilityrs` does not export `markdown::options::{HeadingStyle, LinkStyle}` at the path used above, switch to using the `MarkdownOptions::default()` and only override the fields that differ. The fields are (per the upstream source): `heading_style`, `bullet_char`, `code_fence`, `emphasis_delimiter`, `strong_delimiter`, `link_style`, `preserve_complex_tables`. The default already matches what we want (Atx, '-', '`', '*', "**", Inline, true), so the simplest robust call is:
>
> ```rust
> let opts = ReadabilityOptions::builder()
>     .output_markdown(true)
>     .build();
> ```
>
> Do this if the explicit `MarkdownOptions` construction fails to compile.

- [ ] **Step 5: Commit**

```bash
git add src/extractor/ src/error.rs
git commit -m "feat(extractor): readabilityrs wrapper producing ATX-style markdown"
```

---

## Task 11: Frontmatter writer (M1 subset)

**Files:**
- Modify: `src/extractor/frontmatter.rs` (replace stub)

A small writer that emits the M1 subset of the frontmatter envelope (PRD §6.2): `url`, `canonical_url` when it differs from `url`, `title`, `fetched_at`, `content_hash`, `estimated_tokens`. Hand-rolled so we don't drag in a YAML crate just to emit a fixed schema. M4 will add the larger metadata block.

For M1, `estimated_tokens` uses a chars/4 heuristic; M3 swaps in real tokenizers behind the same call site.

- [ ] **Step 1: Write the failing tests**

Replace `src/extractor/frontmatter.rs`:

```rust
//! YAML frontmatter envelope writer.
//!
//! Emits the M1 subset of PRD §6.2:
//!   - url
//!   - canonical_url (only when different from url)
//!   - title (when present)
//!   - fetched_at (RFC 3339, UTC)
//!   - content_hash (sha256:...)
//!   - estimated_tokens
//!
//! M4 expands this with metadata, language, schema_types, tables/images
//! transformations, etc. M3 swaps the token estimator for real tokenizers.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

/// Inputs for the M1 frontmatter envelope.
pub struct PageMeta<'a> {
    pub url: &'a Url,
    pub canonical_url: &'a Url,
    pub title: Option<&'a str>,
    pub fetched_at: Timestamp,
    pub body: &'a str,
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

    let est = estimate_tokens(meta.body);
    buf.push_str(&format!("estimated_tokens: {est}\n"));

    buf.push_str("---\n\n");
    buf.push_str(meta.body);
    if !meta.body.ends_with('\n') {
        buf.push('\n');
    }
    buf
}

/// Emit one scalar field. Strings are double-quoted with backslash-escaping
/// applied to `"` and `\` so any title content survives intact.
fn write_field(buf: &mut String, key: &str, value: &str) {
    buf.push_str(key);
    buf.push_str(": ");
    buf.push('"');
    for c in value.chars() {
        match c {
            '\\' => buf.push_str(r"\\"),
            '"'  => buf.push_str(r#"\""#),
            '\n' => buf.push_str(r"\n"),
            '\r' => buf.push_str(r"\r"),
            '\t' => buf.push_str(r"\t"),
            _    => buf.push(c),
        }
    }
    buf.push('"');
    buf.push('\n');
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out { s.push_str(&format!("{b:02x}")); }
    s
}

/// **M1 placeholder.** chars/4. M3 replaces this with real tokenizers via a
/// trait so the call site here doesn't change.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;

    fn ts() -> Timestamp { "2026-05-07T12:34:56Z".parse().unwrap() }
    fn u(s: &str) -> Url { Url::parse(s).unwrap() }

    #[test]
    fn emits_required_fields() {
        let url = u("https://example.com/page");
        let body = "# Title\n\nBody.\n";
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: Some("Sample"),
            fetched_at: ts(),
            body,
        });

        assert!(out.starts_with("---\n"));
        assert!(out.contains(r#"url: "https://example.com/page""#));
        assert!(out.contains(r#"title: "Sample""#));
        assert!(out.contains(r#"fetched_at: "2026-05-07T12:34:56Z""#));
        assert!(out.contains("content_hash: \"sha256:"));
        assert!(out.contains("estimated_tokens: "));
        assert!(out.ends_with(body));
    }

    #[test]
    fn omits_canonical_when_same_as_url() {
        let url = u("https://example.com/page");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: None,
            fetched_at: ts(),
            body: "x",
        });
        assert!(!out.contains("canonical_url"));
    }

    #[test]
    fn includes_canonical_when_different() {
        let url = u("https://example.com/page?utm=1");
        let canon = u("https://example.com/page");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &canon,
            title: None,
            fetched_at: ts(),
            body: "x",
        });
        assert!(out.contains(r#"canonical_url: "https://example.com/page""#));
    }

    #[test]
    fn quotes_in_title_are_escaped() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: Some(r#"He said "hi""#),
            fetched_at: ts(),
            body: "x",
        });
        assert!(out.contains(r#"title: "He said \"hi\"""#));
    }

    #[test]
    fn content_hash_is_deterministic() {
        let url = u("https://example.com/p");
        let body = "stable body";
        let a = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body });
        let b = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body });
        assert_eq!(a, b);
    }

    #[test]
    fn estimate_tokens_chars_div_4_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("aaaa"), 1);
        assert_eq!(estimate_tokens("aaaaa"), 2);
    }

    #[test]
    fn body_terminates_with_newline() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body: "no trailing newline" });
        assert!(out.ends_with('\n'));
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test --lib extractor::frontmatter
```

Expected: 7 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/extractor/frontmatter.rs
git commit -m "feat(extractor): M1 frontmatter envelope writer"
```

---

## Task 12: CLI scaffold and `rover fetch` wiring

**Files:**
- Modify: `src/main.rs` (replace hello-world)
- Modify: `src/cli/mod.rs` (replace stub)
- Create: `src/cli/fetch.rs`
- Create: `tests/cli_fetch.rs`

`clap` derive defines all subcommands per PRD §3.1. M1 only implements `fetch`; the others print "not yet implemented" and exit 2. The `fetch` command wires fetcher → extractor → frontmatter → stdout.

- [ ] **Step 1: Write the failing CLI integration test**

Create `tests/cli_fetch.rs`:

```rust
//! End-to-end CLI test for `rover fetch`.

use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetch_prints_markdown_with_frontmatter() {
    let server = MockServer::start().await;
    let body = r#"
<!doctype html>
<html lang="en">
<head><title>Sample</title></head>
<body>
  <article>
    <h1>How to do the thing</h1>
    <p>Body paragraph one with enough text to clear readabilityrs's character threshold of 500 characters by default. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.</p>
  </article>
</body>
</html>
"#;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let url = format!("{}/article", server.uri());

    Command::cargo_bin("rover")
        .unwrap()
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("---\n"))
        .stdout(predicate::str::contains("url:"))
        .stdout(predicate::str::contains("content_hash: \"sha256:"))
        .stdout(predicate::str::contains("How to do the thing"));
}

#[test]
fn fetch_help_lists_args() {
    Command::cargo_bin("rover")
        .unwrap()
        .args(["fetch", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<URL>"));
}

#[test]
fn unknown_subcommand_errors() {
    Command::cargo_bin("rover")
        .unwrap()
        .args(["nope"])
        .assert()
        .failure();
}
```

The `--ssrf-test-loopback` flag is what we'll add to opt into the `TestLoopback` SSRF level for local-network tests against `wiremock`. The flag is gated on the `test-loopback` cargo feature, so it doesn't appear in production builds.

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test --features test-loopback --test cli_fetch
```

Expected: compile errors (binary entry point doesn't define subcommands yet).

- [ ] **Step 3: Implement `src/main.rs`**

Replace `src/main.rs`:

```rust
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rover", version, about = "Web fetch & prep for LLM agents")]
struct Cli {
    /// Path to a TOML config file. If absent, defaults are used.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the MCP server (long-running). M3.
    Mcp,

    /// One-shot fetch, prints markdown to stdout.
    Fetch(FetchArgs),

    /// Long-running batch status (M6).
    Batch { id: String, #[arg(long)] monitor: bool },

    /// Generic task status (M6).
    Task { id: String, #[arg(long)] monitor: bool, #[arg(long)] cancel: bool },

    /// Cache operations (M2).
    #[command(subcommand)]
    Cache(CacheCmd),

    /// Verify the Rover environment (M8).
    Doctor,

    /// Inspect or modify config (M8).
    #[command(subcommand)]
    Config(ConfigCmd),
}

#[derive(Debug, clap::Args)]
struct FetchArgs {
    /// URL to fetch.
    url: String,

    /// **Test-only.** Allow loopback addresses to satisfy SSRF checks. Used by
    /// the integration test suite against wiremock; never used in production.
    #[cfg(any(test, feature = "test-loopback"))]
    #[arg(long, hide = true)]
    ssrf_test_loopback: bool,
}

#[derive(Debug, Subcommand)]
enum CacheCmd { List, Get { url: String }, Purge { pattern: String }, Stats }

#[derive(Debug, Subcommand)]
enum ConfigCmd { Show, Set { key: String, value: String } }

fn main() -> ExitCode {
    rover::telemetry::init("info,rover=debug");
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(dispatch(cli))
}

async fn dispatch(cli: Cli) -> ExitCode {
    let result = match cli.command {
        Command::Fetch(args) => rover::cli::fetch::run(args.into_runtime_args(), cli.config.as_deref()).await,
        Command::Mcp
        | Command::Batch { .. }
        | Command::Task { .. }
        | Command::Cache(_)
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

impl FetchArgs {
    fn into_runtime_args(self) -> rover::cli::fetch::Args {
        rover::cli::fetch::Args {
            url: self.url,
            #[cfg(any(test, feature = "test-loopback"))]
            ssrf_test_loopback: self.ssrf_test_loopback,
        }
    }
}
```

- [ ] **Step 4: Implement `src/cli/fetch.rs`**

Replace `src/cli/mod.rs`:

```rust
//! CLI command implementations.

pub mod fetch;
```

Create `src/cli/fetch.rs`:

```rust
//! `rover fetch <url>` command.

use std::path::Path;
use anyhow::Context;
use jiff::Timestamp;
use url::Url;

use crate::config;
use crate::extractor::frontmatter::{PageMeta, render};
use crate::extractor::pipeline::extract;
use crate::fetcher::client::build_http_client;
use crate::fetcher::fetch::fetch_url;
use crate::fetcher::ssrf::SsrfLevel;

pub struct Args {
    pub url: String,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = config::load(config_path).context("loading config")?;
    let url = Url::parse(&args.url).context("parsing URL argument")?;

    let level = ssrf_level_for_args(&args);

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let page = fetch_url(&client, &url, level).await.context("fetching URL")?;

    if !(200..300).contains(&page.status) {
        anyhow::bail!("HTTP {} from {}", page.status, page.final_url);
    }

    let extracted = extract(&page.body, Some(&page.final_url)).context("extracting article")?;

    let meta = PageMeta {
        url: &url,
        canonical_url: &page.canonical_url,
        title: extracted.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &extracted.body_md,
    };

    let envelope = render(&meta);
    print!("{envelope}");
    Ok(())
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

- [ ] **Step 5: Run the integration tests with the test-loopback feature**

```bash
cargo test --features test-loopback --test cli_fetch
```

Expected: 3 tests pass.

- [ ] **Step 6: Run the entire suite**

```bash
cargo test --features test-loopback
```

Expected: all unit + integration tests pass.

- [ ] **Step 7: Smoke-test the CLI manually (optional)**

```bash
cargo run -- fetch https://example.com/
```

Expected: stdout starts with `---\n`, contains `url: "https://example.com/"`, includes a `# Example Domain` heading, exit 0.

(If you're offline this will obviously fail with a DNS error. The wiremock-based tests cover offline correctness.)

- [ ] **Step 8: Commit**

```bash
git add src/main.rs src/cli/ tests/cli_fetch.rs
git commit -m "feat(cli): rover fetch end-to-end (clap, fetcher, extractor, frontmatter)"
```

---

## Task 13: README quickstart

**Files:**
- Modify: `README.md` (replace existing one-line content)

A minimal README so a fresh visitor knows what Rover is, how to build it, and how to run a fetch. Keep it tight; the comprehensive docs come in M8/M9 per PRD §17.

- [ ] **Step 1: Replace `README.md`**

```markdown
# Rover

An MCP (Model Context Protocol) server that fetches web pages and turns them
into clean, token-efficient Markdown for LLM agents.

> **Status:** early development. Milestone M1 (single-URL fetch path) is
> currently being implemented. See `docs/superpowers/prd/2026-05-07-rover-prd.md`
> for the product spec and `docs/superpowers/specs/2026-05-07-rover-design.md`
> for architectural decisions.

## Build

```sh
cargo build --release
```

The release binary lands at `target/release/rover`.

## Try it

```sh
cargo run --release -- fetch https://en.wikipedia.org/wiki/Rust_(programming_language)
```

Output is YAML-frontmattered Markdown printed to stdout:

```yaml
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
fetched_at: "2026-05-07T12:34:56Z"
content_hash: "sha256:..."
estimated_tokens: 14823
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language ...
```

## Subcommands

`rover fetch <url>` is implemented in M1. The full subcommand surface
(`rover mcp`, `rover batch`, `rover task`, `rover cache`, `rover doctor`,
`rover config`) ships across milestones M2–M8 — see the PRD.

## License

MIT or Apache-2.0, at your option.
```

- [ ] **Step 2: Verify it renders**

```bash
cat README.md | head -50
```

(No automated check; manual review.)

- [ ] **Step 3: Final test pass**

```bash
cargo test --features test-loopback
cargo build --release
```

Expected: all tests pass; release build succeeds.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs(readme): M1 quickstart and status"
```

---

## Acceptance Check

The PRD acceptance for M1:

> Can fetch `https://example.com` and a few real article URLs, produces clean Markdown with frontmatter.

To verify against real URLs (requires network):

```bash
cargo run --release -- fetch https://example.com/
cargo run --release -- fetch https://en.wikipedia.org/wiki/Rust_(programming_language)
cargo run --release -- fetch https://blog.rust-lang.org/2024/02/29/Rust-1.85.0/
```

Each invocation should print frontmatter + Markdown to stdout and exit 0.

The wiremock-based integration tests are the deterministic acceptance gate; the live URLs above are sanity checks before declaring the milestone done.

---

## Decisions deferred to later milestones (intentional)

- **Caching** (M2): no SQLite yet; every `rover fetch` is a network round-trip.
- **MCP server** (M3): only the CLI exists in M1.
- **Token counting** (M3): chars/4 heuristic only; real tokenizers come behind the same `estimate_tokens` function.
- **Metadata extraction** (M4): only the title/byline/lang/excerpt/site_name/published_time/image that `readabilityrs` already gives us. JSON-LD, OG, Twitter Card, microdata in M4.
- **Tables/images transforms** (M4): readabilityrs's defaults pass through; no Sample/CsvFile/Download modes yet.
- **Rate limiting / robots** (M5): every URL is hit immediately at full speed.
- **Long-running tasks** (M6): no batch, no NDJSON streaming.
- **Summarization** (M7): no `summarize` tool.
- **SSRF levels beyond Strict** (M8): only `Strict` is exposed in the production CLI surface; `TestLoopback` is an internal test variant gated on the `test-loopback` cargo feature.
- **HAR / doctor / config show-set** (M8).
- **Headless / local inference / VLM** (M9).

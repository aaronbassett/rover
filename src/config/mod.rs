//! Configuration loading.
//!
//! M1 covers a tiny subset of the full schema documented in PRD §12.
//! Subsequent milestones extend this struct.

pub mod provenance;

use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub fetch: FetchConfig,

    #[serde(default)]
    pub ssrf: SsrfConfig,

    #[serde(default)]
    pub debug: DebugConfig,

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

    #[serde(default)]
    pub summarization: SummarizationConfig,

    #[serde(default)]
    pub backends: std::collections::HashMap<String, BackendConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

    /// Test-only convenience for swapping the SSRF level on an
    /// already-loaded config. Production callers go through TOML.
    #[cfg(any(test, feature = "test-loopback"))]
    pub fn with_ssrf_level(mut self, level: &str) -> Self {
        self.ssrf.level = level.to_string();
        self
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Tokenizer configuration. The `default` family is used for token counting
/// in the frontmatter and the MCP layer when callers don't specify one.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizerConfig {
    #[serde(default = "default_tokenizer")]
    pub default: crate::tokenizer::Tokenizer,
}

impl Default for TokenizerConfig {
    fn default() -> Self {
        Self {
            default: default_tokenizer(),
        }
    }
}

fn default_tokenizer() -> crate::tokenizer::Tokenizer {
    crate::tokenizer::Tokenizer::O200k
}

/// MCP server configuration. Durations are parsed by `humantime`
/// (e.g. "5s", "60s", "2m"). Both intervals must be non-zero.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Output configuration. When `dir` is `None`, `ROVER_OUTPUT_DIR` (if set)
/// takes precedence, otherwise the platform `data_local_dir()/rover/output`
/// default applies. See `OutputPaths::resolve`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    #[serde(default)]
    pub dir: Option<std::path::PathBuf>,
}

/// Per-domain pacing knobs. All HTTP-bound code paths run through a single
/// `Pacer` built from this struct at startup. See M5 design spec §3 and §4.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

    /// Threshold (seconds) above which a server-provided `Retry-After`
    /// converts a synchronous fetch into a deferred `retry` task instead of
    /// sleeping in-line. See M6 design §3.
    #[serde(default = "default_deferred_threshold_secs")]
    pub deferred_retry_threshold_secs: u64,
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
            deferred_retry_threshold_secs: default_deferred_threshold_secs(),
        }
    }
}

fn default_rpm_per_domain() -> u32 {
    60
}
fn default_per_domain_concurrency() -> u32 {
    2
}
fn default_global_concurrency() -> u32 {
    8
}
fn default_max_retries() -> u8 {
    3
}
fn default_initial_backoff() -> Duration {
    Duration::from_millis(500)
}
fn default_max_backoff() -> Duration {
    Duration::from_secs(30)
}
fn default_retry_after_ceiling() -> Duration {
    Duration::from_secs(300)
}
fn default_deferred_threshold_secs() -> u64 {
    30
}

/// Robots.txt fetch + respect knobs.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

fn default_respect() -> bool {
    true
}
fn default_robots_ttl() -> Duration {
    Duration::from_secs(24 * 3600)
}
fn default_robots_failure_ttl() -> Duration {
    Duration::from_secs(5 * 60)
}

/// Top-level `[summarization]` section.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

    /// Per-table summarization defaults consumed by the
    /// `TablesMode::Summarize` hook in `mcp::tools::fetch`. Lives under
    /// `[summarization.tables]` in the config file.
    #[serde(default)]
    pub tables: TablesSummarizationConfig,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            default_backend: default_summarization_backend(),
            default_mode: default_summarization_mode(),
            default_style: default_summarization_style(),
            fallback_to_extractive: default_summarization_fallback(),
            tables: TablesSummarizationConfig::default(),
        }
    }
}

fn default_summarization_backend() -> String {
    "default".to_string()
}
fn default_summarization_mode() -> String {
    "abstractive".to_string()
}
fn default_summarization_style() -> String {
    "prose".to_string()
}
fn default_summarization_fallback() -> bool {
    true
}

/// `[summarization.tables]` block. Controls the per-table summarize
/// defaults used by the `TablesMode::Summarize` hook.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TablesSummarizationConfig {
    #[serde(default = "default_tables_target_tokens")]
    pub target_tokens: usize,
    #[serde(default = "default_tables_focus")]
    pub focus: String,
}

impl Default for TablesSummarizationConfig {
    fn default() -> Self {
        Self {
            target_tokens: default_tables_target_tokens(),
            focus: default_tables_focus(),
        }
    }
}

fn default_tables_target_tokens() -> usize {
    150
}
fn default_tables_focus() -> String {
    "Describe what this table shows. Highlight any extreme values or notable rows.".to_string()
}

/// One `[backends.<name>]` block. Free-form `kind`/`provider` strings —
/// validation lives in `summarizer::registry::build` where the parsed
/// values are matched against the typed enum.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
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

/// Top-level `[ssrf]` section. M8 introduces this — earlier milestones
/// hardcoded `SsrfLevel::Strict`. The `level` field is a free-form string
/// here so the file accepts unknown levels with a typed error from the
/// fetcher rather than a serde error; `validate_url`/`validate_addresses`
/// reject malformed levels at first use.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Top-level `[debug]` section. M8 introduces this for HAR recording and
/// log-level overrides.
///
/// `har_body_cap` accepts either a raw integer (bytes) or a humansize
/// string like "64KiB" / "1MiB" via a custom deserializer. The internal
/// representation is `u64` bytes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugConfig {
    #[serde(default = "default_debug_har_path")]
    pub har_path: String,

    #[serde(
        default = "default_debug_har_body_cap",
        deserialize_with = "deserialize_humansize"
    )]
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
        other => {
            return Err(format!(
                "unknown size suffix `{other}` (expected KiB|MiB|GiB)"
            ));
        }
    };
    Ok(n * mult)
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
    let mut cfg: Config = toml::from_str(&bytes).map_err(|source| ConfigError::Parse {
        path: path.display().to_string(),
        source,
    })?;
    validate(&mut cfg).map_err(|message| ConfigError::Invalid {
        path: path.display().to_string(),
        message,
    })?;
    Ok(cfg)
}

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
    for d in &mut cfg.cache.override_no_store_domains {
        d.make_ascii_lowercase();
    }
    if cfg.mcp.heartbeat_interval.is_zero() {
        return Err("mcp.heartbeat_interval must be > 0".to_string());
    }
    if cfg.mcp.reap_threshold.is_zero() {
        return Err("mcp.reap_threshold must be > 0".to_string());
    }

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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert!(cfg.fetch.user_agent.starts_with("Rover/"));
        assert_eq!(cfg.fetch.timeout_secs, 15);

        // Cache defaults per PRD §12.
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(3600));
        assert_eq!(cfg.cache.min_ttl, Duration::from_secs(300));
        assert_eq!(cfg.cache.max_ttl, Duration::from_secs(7 * 86400));
        assert!(!cfg.cache.override_no_store);
        assert!(cfg.cache.override_no_store_domains.is_empty());
        assert!(!cfg.cache.store_raw_html);
    }

    #[test]
    fn load_with_no_path_returns_default() {
        let cfg = load(None).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, 15);
    }

    #[test]
    fn load_from_file_overrides_defaults() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[fetch]
user_agent = "test-ua"
timeout_secs = 5
"#
        )
        .unwrap();

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
        writeln!(
            file,
            r#"
[fetch]
unknown_field = "x"
"#
        )
        .unwrap();
        let result = load(Some(file.path()));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn load_unknown_field_in_cache_errors() {
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
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(30 * 60));
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
    fn override_no_store_domains_normalized_to_lowercase() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
override_no_store_domains = ["DOCS.example.COM", "CDN.foo.com"]
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(
            cfg.cache.override_no_store_domains,
            vec!["docs.example.com".to_string(), "cdn.foo.com".to_string()]
        );
    }

    #[test]
    fn load_accepts_equal_ttls() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[cache]
default_ttl = "1h"
min_ttl = "1h"
max_ttl = "1h"
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(3600));
    }

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
    fn load_output_dir_override() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[output]
dir = "/tmp/rover-out"
"#
        )
        .unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(
            cfg.output.dir.as_deref().unwrap().to_str(),
            Some("/tmp/rover-out")
        );
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
        assert!(matches!(
            load(Some(file.path())),
            Err(ConfigError::Invalid { .. })
        ));
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
        assert!(matches!(
            load(Some(file.path())),
            Err(ConfigError::Invalid { .. })
        ));
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
        assert!(matches!(
            load(Some(file.path())),
            Err(ConfigError::Invalid { .. })
        ));
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
        assert!(matches!(
            load(Some(file.path())),
            Err(ConfigError::Invalid { .. })
        ));
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
        assert!(matches!(
            load(Some(file.path())),
            Err(ConfigError::Invalid { .. })
        ));
    }

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
        assert_eq!(cfg.summarization.tables.target_tokens, 150);
        assert!(cfg.summarization.tables.focus.contains("Describe"));
    }

    #[test]
    fn summarization_tables_block_overrides_defaults() {
        let toml = r#"
[summarization.tables]
target_tokens = 250
focus = "Custom table focus prompt."
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.summarization.tables.target_tokens, 250);
        assert_eq!(cfg.summarization.tables.focus, "Custom table focus prompt.");
        // Sibling defaults remain in force.
        assert_eq!(cfg.summarization.default_backend, "default");
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

    #[test]
    fn debug_section_parses_with_defaults() {
        let cfg: Config = toml::from_str("[debug]\n").unwrap();
        assert_eq!(cfg.debug.har_path, "");
        assert_eq!(cfg.debug.har_body_cap, 64 * 1024);
        assert_eq!(cfg.debug.log_level, "info");
    }

    #[test]
    fn debug_section_har_body_cap_accepts_humansize() {
        let cfg: Config = toml::from_str(
            r#"[debug]
har_body_cap = "1MiB"
"#,
        )
        .unwrap();
        assert_eq!(cfg.debug.har_body_cap, 1024 * 1024);
    }

    #[test]
    fn debug_section_har_body_cap_accepts_integer_bytes() {
        let cfg: Config = toml::from_str(
            r#"[debug]
har_body_cap = 8192
"#,
        )
        .unwrap();
        assert_eq!(cfg.debug.har_body_cap, 8192);
    }

    #[test]
    fn debug_section_rejects_unknown_field() {
        let r: Result<Config, _> = toml::from_str(
            r#"[debug]
har_path = ""
bogus = 1
"#,
        );
        assert!(r.is_err());
    }
}

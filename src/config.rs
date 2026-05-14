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

/// Tokenizer configuration. The `default` family is used for token counting
/// in the frontmatter and the MCP layer when callers don't specify one.
#[derive(Debug, Clone, Deserialize)]
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

/// Output configuration. When `dir` is `None`, `ROVER_OUTPUT_DIR` (if set)
/// takes precedence, otherwise the platform `data_local_dir()/rover/output`
/// default applies. See `OutputPaths::resolve`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    #[serde(default)]
    pub dir: Option<std::path::PathBuf>,
}

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

fn default_respect() -> bool {
    true
}
fn default_robots_ttl() -> Duration {
    Duration::from_secs(24 * 3600)
}
fn default_robots_failure_ttl() -> Duration {
    Duration::from_secs(5 * 60)
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
}

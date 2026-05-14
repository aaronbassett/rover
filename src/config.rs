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
}

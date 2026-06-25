//! Configuration loading.
//!
//! M1 covers a tiny subset of the full schema documented in PRD §12.
//! Subsequent milestones extend this struct.

pub mod edit;
pub mod provenance;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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

    #[serde(default)]
    pub headless: HeadlessConfig,

    #[serde(default)]
    pub image_captions: ImageCaptionsConfig,

    #[serde(default)]
    pub captioners: std::collections::BTreeMap<String, CaptionerConfig>,

    #[serde(default)]
    pub prompt_injection: PromptInjectionConfig,
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

    /// Stale-while-revalidate grace window. When a cache entry expired no
    /// more than this long ago, `fetch_with_cache` may serve the stale row
    /// and queue a background `revalidate` task. Beyond this window the
    /// row is treated as a cache miss and re-fetched synchronously, so
    /// callers never receive arbitrarily old content from the cache.
    /// Default: 5 minutes.
    #[serde(default = "default_cache_swr_window", with = "humantime_serde")]
    pub stale_while_revalidate_window: Duration,

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
            stale_while_revalidate_window: default_cache_swr_window(),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        }
    }
}

fn default_cache_default_ttl() -> Duration {
    // 15 minutes. Tightened from 1h so that, absent an explicit `Cache-Control`
    // max-age, a cache poisoned with stale or attacker-influenced content has a
    // short blast radius before the next revalidation. Origins that want longer
    // caching can still say so via response headers.
    Duration::from_secs(15 * 60)
}

fn default_cache_min_ttl() -> Duration {
    Duration::from_secs(300)
}

fn default_cache_max_ttl() -> Duration {
    Duration::from_secs(7 * 86400)
}

fn default_cache_swr_window() -> Duration {
    Duration::from_secs(5 * 60)
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
    // Rover is an agent's browser, not a spider or scraper: it fetches the
    // page a user/agent explicitly asked for, one at a time. robots.txt governs
    // automated crawling, so the gate defaults off. Set `robots.respect = true`
    // (or pass nothing and rely on rate limits) to opt back into enforcement.
    false
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

/// `[headless]` configuration block. M9 adds browser/headless-fetch knobs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessConfig {
    #[serde(default = "default_headless_max_concurrent")]
    pub max_concurrent: usize,

    /// Path to a Chrome/Chromium executable. Empty string means auto-detect.
    #[serde(default)]
    pub chrome_executable: String,

    /// Fulfill image requests with empty 200 (saves bandwidth + render time).
    #[serde(default = "default_block_images")]
    pub block_images: bool,

    /// Fulfill font requests with empty 200.
    #[serde(default = "default_block_fonts")]
    pub block_fonts: bool,

    /// Fulfill audio/video/track requests with empty 200.
    #[serde(default = "default_block_media")]
    pub block_media: bool,

    /// Fulfill CSS requests with empty 200. Default `false` — many SPAs need
    /// layout to render correctly.
    #[serde(default)]
    pub block_css: bool,

    /// Fulfill third-party analytics/tracker requests with empty 200.
    #[serde(default = "default_block_third_party")]
    pub block_third_party: bool,

    /// Disable service workers at browser init via CDP bypass. Honored by
    /// `HeadlessRenderer` setup (not by the intercept handler).
    #[serde(default = "default_block_service_workers")]
    pub block_service_workers: bool,

    /// Default wait condition: `"domcontentloaded"` or `"networkidle0"`
    /// (wait for the network to fully settle — captures post-load XHR content).
    #[serde(default = "default_headless_wait")]
    pub default_wait: String,

    /// Per-render timeout in seconds (covers the wait phase).
    #[serde(default = "default_headless_timeout_secs")]
    pub timeout_secs: u64,

    /// Whether `HeadlessMode::Auto` should run the SPA detection heuristic.
    #[serde(default = "default_auto_detect_spa")]
    pub auto_detect_spa: bool,

    /// In `Auto` mode, the delay (in seconds) before escalating to a headless
    /// render once the plain HTTP fetch is in — i.e. between detecting that a
    /// render is needed (an unrendered SPA, or a bot-protection challenge) and
    /// launching/driving the browser. Gives the origin a breather between the
    /// lightweight fetch and the heavier browser hit. `0` disables the pause.
    /// Does not apply to `On` mode, which has no detection step.
    #[serde(default = "default_headless_launch_delay_secs")]
    pub launch_delay_secs: u64,
}

impl HeadlessConfig {
    /// Render timeout as a `Duration`.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_secs)
    }

    /// Auto-mode pre-render escalation delay as a `Duration`.
    pub fn launch_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.launch_delay_secs)
    }
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            max_concurrent: default_headless_max_concurrent(),
            chrome_executable: String::new(),
            block_images: default_block_images(),
            block_fonts: default_block_fonts(),
            block_media: default_block_media(),
            block_css: false,
            block_third_party: default_block_third_party(),
            block_service_workers: default_block_service_workers(),
            default_wait: default_headless_wait(),
            timeout_secs: default_headless_timeout_secs(),
            auto_detect_spa: default_auto_detect_spa(),
            launch_delay_secs: default_headless_launch_delay_secs(),
        }
    }
}

fn default_headless_max_concurrent() -> usize {
    4
}

fn default_headless_wait() -> String {
    "domcontentloaded".to_string()
}

fn default_headless_timeout_secs() -> u64 {
    15
}

fn default_headless_launch_delay_secs() -> u64 {
    2
}

fn default_auto_detect_spa() -> bool {
    true
}

fn default_block_images() -> bool {
    true
}

fn default_block_fonts() -> bool {
    true
}

fn default_block_media() -> bool {
    true
}

fn default_block_third_party() -> bool {
    true
}

fn default_block_service_workers() -> bool {
    true
}

/// `[image_captions]` defaults block.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageCaptionsConfig {
    pub default: Option<String>,
    pub max_tokens: usize,
    pub max_per_page: usize,
    pub min_width: u32,
    pub min_height: u32,
    #[serde(deserialize_with = "humanbytes_to_u64")]
    pub max_bytes: u64,
    pub max_concurrent: usize,
}

impl Default for ImageCaptionsConfig {
    fn default() -> Self {
        Self {
            default: None,
            max_tokens: 50,
            max_per_page: 10,
            min_width: 200,
            min_height: 200,
            max_bytes: 10 * 1024 * 1024,
            max_concurrent: 2,
        }
    }
}

/// `[captioners.<name>]` block. Mirrors `BackendConfig` (M7).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptionerConfig {
    pub kind: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
}

/// Parse a human-readable byte size string such as "10MiB", "1.5GiB", "1000"
/// into a raw `u64` byte count.
pub fn parse_human_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    let (num_str, unit) = s
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|i| (&s[..i], &s[i..]))
        .ok_or_else(|| format!("invalid size: {s}"))?;
    let num: f64 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid size number: {num_str}"))?;
    let mult: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "B" => 1,
        "K" | "KB" => 1_000,
        "KIB" => 1_024,
        "M" | "MB" => 1_000_000,
        "MIB" => 1_024 * 1_024,
        "G" | "GB" => 1_000_000_000,
        "GIB" => 1_024 * 1_024 * 1_024,
        other => return Err(format!("unknown size unit: {other}")),
    };
    Ok((num * mult as f64) as u64)
}

fn humanbytes_to_u64<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = toml::Value::deserialize(d)?;
    match v {
        toml::Value::Integer(n) if n >= 0 => Ok(n as u64),
        toml::Value::String(s) => parse_human_bytes(&s).map_err(D::Error::custom),
        other => Err(D::Error::custom(format!(
            "expected integer bytes or humansize string, got {other:?}",
        ))),
    }
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

/// Top-level `[prompt_injection]` section. `level` and `model` are free-form
/// strings here (mirroring `SsrfConfig.level`); `guard::GuardConfig::from_config`
/// parses them into typed enums at first use, surfacing a typed error rather
/// than a serde error.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptInjectionConfig {
    #[serde(default = "default_pi_level")]
    pub level: String,

    #[serde(default = "default_pi_model")]
    pub model: String,

    #[serde(default = "default_pi_model_threshold")]
    pub model_threshold: f64,

    #[serde(default)]
    pub allowlist: PromptInjectionAllowlist,

    #[serde(default)]
    pub agent_overrides: PromptInjectionOverrides,
}

impl Default for PromptInjectionConfig {
    fn default() -> Self {
        Self {
            level: default_pi_level(),
            model: default_pi_model(),
            model_threshold: default_pi_model_threshold(),
            allowlist: PromptInjectionAllowlist::default(),
            agent_overrides: PromptInjectionOverrides::default(),
        }
    }
}

/// Per-method URL-glob allowlists. A URL matching the glob list skips that
/// method on OUTPUT for that URL. A bare `"*"` disables the method entirely.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptInjectionAllowlist {
    #[serde(default)]
    pub wrap: Vec<String>,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub model: Vec<String>,
}

/// Per-method agent-override grants (default: all deny). The MCP `security`
/// arg is honored for a method only when its grant here is `true`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptInjectionOverrides {
    #[serde(default)]
    pub wrap: bool,
    #[serde(default)]
    pub patterns: bool,
    #[serde(default)]
    pub model: bool,
    #[serde(default)]
    pub level: bool,
}

fn default_pi_level() -> String {
    "moderate".to_string()
}
fn default_pi_model() -> String {
    "disabled".to_string()
}
fn default_pi_model_threshold() -> f64 {
    0.9
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

/// Ordered config-file candidates searched when `--config` is absent.
///
/// When `ROVER_CONFIG` is set it designates the sole candidate (an explicit
/// redirect should not silently fall through to other locations). Otherwise the
/// platform config dir (`<config_dir>/rover/rover.toml`) is tried first, then a
/// project-local `./rover.toml`.
fn config_candidates_from(
    rover_config_env: Option<&str>,
    config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(p) = rover_config_env {
        return vec![PathBuf::from(p)];
    }
    let mut candidates = Vec::with_capacity(2);
    if let Some(dir) = config_dir {
        candidates.push(dir.join("rover").join("rover.toml"));
    }
    candidates.push(PathBuf::from("rover.toml"));
    candidates
}

fn config_candidates() -> Vec<PathBuf> {
    config_candidates_from(
        std::env::var("ROVER_CONFIG").ok().as_deref(),
        dirs::config_dir().as_deref(),
    )
}

/// The canonical config path: where `rover config set` creates a new file, and
/// where `rover config show` reports when no file exists yet. This is the first
/// (highest-precedence) candidate, regardless of whether it exists on disk.
pub fn default_config_path() -> PathBuf {
    config_candidates()
        .into_iter()
        .next()
        .expect("config_candidates always yields at least one path")
}

/// The first existing config file among the ordered candidates, or `None` when
/// none exists (built-in defaults apply).
///
/// Shared by the runtime subcommands and by `config show` / `config set` so all
/// of them agree on which file is "the active config" — closing the footgun
/// where `config set` wrote a file the runtime never read.
pub fn resolve_existing_config_path() -> Option<PathBuf> {
    config_candidates().into_iter().find(|p| p.is_file())
}

/// Load the effective config, resolving the default path when `--config` is
/// absent.
///
/// - `Some(path)`: an explicitly requested file. It MUST exist and parse — a
///   typo in `--config` fails loudly rather than silently falling back to
///   defaults.
/// - `None`: search the default candidates (`ROVER_CONFIG`, then the platform
///   config dir, then `./rover.toml`) and load the first that exists; if none
///   exists, fall back to built-in defaults (the config file is optional).
///
/// Runtime subcommands call this instead of [`load`] so a saved config file is
/// honored without requiring `--config` on every invocation.
pub fn load_resolved(explicit: Option<&Path>) -> Result<Config, ConfigError> {
    if let Some(path) = explicit {
        tracing::debug!(path = %path.display(), "loading config from --config");
        return load(Some(path));
    }
    match resolve_existing_config_path() {
        Some(path) => {
            tracing::debug!(path = %path.display(), "loading config from resolved default path");
            load(Some(&path))
        }
        None => {
            tracing::debug!("no config file found at any default path; using built-in defaults");
            Ok(Config::default())
        }
    }
}

/// Pure core shared with the public [`load_resolved`], with the resolved
/// "active config" path injected so both branches are unit-testable without
/// touching process env or the real config dir.
#[cfg(test)]
fn load_resolved_from(
    explicit: Option<&Path>,
    resolved_existing: Option<&Path>,
) -> Result<Config, ConfigError> {
    match (explicit, resolved_existing) {
        (Some(path), _) => load(Some(path)),
        (None, Some(path)) => load(Some(path)),
        (None, None) => Ok(Config::default()),
    }
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
        let baseline_respect = cfg.robots.respect;
        cfg.apply_overrides(None, None, None, None, false);
        assert_eq!(cfg.rate_limit.requests_per_minute_per_domain, baseline_rpm);
        assert_eq!(cfg.rate_limit.max_retries, baseline_retries);
        assert_eq!(cfg.robots.respect, baseline_respect);
    }

    #[test]
    fn apply_overrides_disables_robots_when_requested() {
        let mut cfg = Config::default();
        // Start enabled so the assertion proves the override flips it, not just
        // that it matches the (now off-by-default) baseline.
        cfg.robots.respect = true;
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

        // Cache defaults per PRD §12 (default_ttl tightened to 15m).
        assert_eq!(cfg.cache.default_ttl, Duration::from_secs(15 * 60));
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
        // Rover is an agent browser, not a crawler: robots enforcement is off
        // by default (opt in with `robots.respect = true`).
        assert!(!cfg.robots.respect);
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

    #[test]
    fn image_captions_defaults_match_spec() {
        let c = ImageCaptionsConfig::default();
        assert_eq!(c.max_tokens, 50);
        assert_eq!(c.max_per_page, 10);
        assert_eq!(c.min_width, 200);
        assert_eq!(c.min_height, 200);
        assert_eq!(c.max_bytes, 10 * 1024 * 1024);
        assert_eq!(c.max_concurrent, 2);
    }

    #[test]
    fn human_bytes_parses_common_forms() {
        assert_eq!(parse_human_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_human_bytes("10MiB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_human_bytes("10MB").unwrap(), 10_000_000);
        assert_eq!(
            parse_human_bytes("1.5GiB").unwrap(),
            (1.5_f64 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert!(parse_human_bytes("bogus").is_err());
    }

    #[test]
    fn image_captions_deserializes_from_toml() {
        let toml_str = r#"
[image_captions]
default = "openai"
max_per_page = 5
min_width = 100
min_height = 100
max_bytes = "1MiB"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.image_captions.default.as_deref(), Some("openai"));
        assert_eq!(cfg.image_captions.max_per_page, 5);
        assert_eq!(cfg.image_captions.max_bytes, 1024 * 1024);
        assert_eq!(cfg.image_captions.max_tokens, 50);
    }

    #[test]
    fn captioners_block_round_trips() {
        let toml_str = r#"
[captioners.openai]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[captioners.local]
kind = "local"
model = "HuggingFaceTB/SmolVLM-256M-Instruct"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.captioners.len(), 2);
        assert_eq!(
            cfg.captioners.get("openai").unwrap().provider.as_deref(),
            Some("openai")
        );
        assert_eq!(cfg.captioners.get("local").unwrap().kind, "local");
    }

    #[test]
    fn headless_m9_keys_default_correctly() {
        let h = HeadlessConfig::default();
        assert_eq!(h.max_concurrent, 4);
        assert!(h.chrome_executable.is_empty());
        assert_eq!(h.launch_delay_secs, 2);
        assert_eq!(h.launch_delay(), std::time::Duration::from_secs(2));
    }

    #[test]
    fn headless_launch_delay_parses_and_disables() {
        let cfg: Config = toml::from_str("[headless]\nlaunch_delay_secs = 0\n").unwrap();
        assert_eq!(cfg.headless.launch_delay_secs, 0);
        assert!(cfg.headless.launch_delay().is_zero());
        let cfg: Config = toml::from_str("[headless]\nlaunch_delay_secs = 5\n").unwrap();
        assert_eq!(
            cfg.headless.launch_delay(),
            std::time::Duration::from_secs(5)
        );
    }

    #[test]
    fn prompt_injection_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.prompt_injection.level, "moderate");
        assert_eq!(cfg.prompt_injection.model, "disabled");
        assert!((cfg.prompt_injection.model_threshold - 0.9).abs() < f64::EPSILON);
        assert!(cfg.prompt_injection.allowlist.wrap.is_empty());
        assert!(cfg.prompt_injection.allowlist.patterns.is_empty());
        assert!(cfg.prompt_injection.allowlist.model.is_empty());
        assert!(!cfg.prompt_injection.agent_overrides.wrap);
        assert!(!cfg.prompt_injection.agent_overrides.patterns);
        assert!(!cfg.prompt_injection.agent_overrides.model);
        assert!(!cfg.prompt_injection.agent_overrides.level);
    }

    #[test]
    fn prompt_injection_parses_full_block() {
        let toml = r#"
[prompt_injection]
level = "strict"
model = "deberta-base"
model_threshold = 0.75

[prompt_injection.allowlist]
wrap = ["https://*.internal.example.com/*"]
patterns = ["*"]
model = []

[prompt_injection.agent_overrides]
wrap = true
patterns = false
model = true
level = true
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.prompt_injection.level, "strict");
        assert_eq!(cfg.prompt_injection.model, "deberta-base");
        assert!((cfg.prompt_injection.model_threshold - 0.75).abs() < f64::EPSILON);
        assert_eq!(
            cfg.prompt_injection.allowlist.wrap,
            vec!["https://*.internal.example.com/*".to_string()]
        );
        assert_eq!(
            cfg.prompt_injection.allowlist.patterns,
            vec!["*".to_string()]
        );
        assert!(cfg.prompt_injection.agent_overrides.wrap);
        assert!(!cfg.prompt_injection.agent_overrides.patterns);
        assert!(cfg.prompt_injection.agent_overrides.model);
        assert!(cfg.prompt_injection.agent_overrides.level);
    }

    #[test]
    fn prompt_injection_rejects_unknown_field() {
        let toml = "[prompt_injection]\nbogus = 1\n";
        let r: Result<Config, _> = toml::from_str(toml);
        assert!(r.is_err(), "expected deny_unknown_fields rejection");
    }

    #[test]
    fn config_candidates_prefers_rover_config_env_as_sole_candidate() {
        let c = config_candidates_from(Some("/custom/x.toml"), Some(Path::new("/cfg")));
        assert_eq!(c, vec![std::path::PathBuf::from("/custom/x.toml")]);
    }

    #[test]
    fn config_candidates_searches_platform_then_cwd() {
        let c = config_candidates_from(None, Some(Path::new("/cfg")));
        assert_eq!(
            c,
            vec![
                std::path::PathBuf::from("/cfg/rover/rover.toml"),
                std::path::PathBuf::from("rover.toml"),
            ]
        );
    }

    #[test]
    fn config_candidates_falls_back_to_cwd_rover_toml() {
        let c = config_candidates_from(None, None);
        assert_eq!(c, vec![std::path::PathBuf::from("rover.toml")]);
    }

    #[test]
    fn resolve_existing_prefers_platform_over_cwd_candidate() {
        // Lay down <tmp>/rover/rover.toml and confirm it is the chosen file.
        let tmp = tempfile::tempdir().unwrap();
        let rover_dir = tmp.path().join("rover");
        std::fs::create_dir_all(&rover_dir).unwrap();
        let platform_file = rover_dir.join("rover.toml");
        std::fs::write(&platform_file, "[fetch]\ntimeout_secs = 3\n").unwrap();

        let resolved = config_candidates_from(None, Some(tmp.path()))
            .into_iter()
            .find(|p| p.is_file());
        assert_eq!(resolved, Some(platform_file));
    }

    #[test]
    fn resolve_existing_is_none_when_no_candidate_exists() {
        let tmp = tempfile::tempdir().unwrap();
        // tmp has no rover/rover.toml, and the crate root has no ./rover.toml.
        let resolved = config_candidates_from(None, Some(tmp.path()))
            .into_iter()
            .find(|p| p.is_file());
        assert_eq!(resolved, None);
    }

    #[test]
    fn load_resolved_uses_explicit_path_when_present() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[fetch]\ntimeout_secs = 7\n").unwrap();
        // A resolved default must be ignored when --config is supplied.
        let cfg = load_resolved_from(Some(file.path()), None).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, 7);
    }

    #[test]
    fn load_resolved_errors_when_explicit_path_missing() {
        // An explicit --config typo must fail loudly, NOT fall back to the
        // resolved default or to built-in defaults.
        let mut default_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(default_file, "[fetch]\ntimeout_secs = 9\n").unwrap();
        let result = load_resolved_from(
            Some(Path::new("/no/such/__rover_explicit__.toml")),
            Some(default_file.path()),
        );
        assert!(matches!(result, Err(ConfigError::Read { .. })));
    }

    #[test]
    fn load_resolved_loads_resolved_default_when_no_explicit() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[fetch]\ntimeout_secs = 11\n").unwrap();
        let cfg = load_resolved_from(None, Some(file.path())).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, 11);
    }

    #[test]
    fn load_resolved_falls_back_to_defaults_when_nothing_resolves() {
        let cfg = load_resolved_from(None, None).unwrap();
        assert_eq!(cfg.fetch.timeout_secs, default_timeout_secs());
    }
}

//! `rover mcp` subcommand — start the MCP server over stdio.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use crate::config;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp;
use crate::storage::Db;

pub struct Args {
    pub ignore_robots: bool,
    pub rate_limit_rpm: Option<u32>,
    pub per_host_concurrency: Option<u32>,
    pub global_concurrency: Option<u32>,
    pub max_retries: Option<u8>,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let mut cfg = config::load(config_path).context("loading config")?;
    if args.ignore_robots {
        cfg.robots.respect = false;
    }
    // NOTE: overrides bypass config::validate; concurrency widths clamped to
    // >=1 to avoid Semaphore::new(0) silently hanging on acquire.
    if let Some(v) = args.rate_limit_rpm {
        cfg.rate_limit.requests_per_minute_per_domain = v;
    }
    if let Some(v) = args.per_host_concurrency {
        cfg.rate_limit.per_domain_concurrency = v.max(1);
    }
    if let Some(v) = args.global_concurrency {
        cfg.rate_limit.global_concurrency = v.max(1);
    }
    if let Some(v) = args.max_retries {
        cfg.rate_limit.max_retries = v;
    }
    let cfg = Arc::new(cfg);

    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let ssrf_level = ssrf_level_from_env();

    mcp::serve_stdio(db, cfg, ssrf_level).await
}

/// Production builds always use `SsrfLevel::Strict`.
///
/// When compiled with `--features test-loopback`, the integration test
/// harness can set `ROVER_MCP_SSRF=test_loopback` to allow wiremock to
/// bind to 127.0.0.1 and still satisfy SSRF. This env var has no effect
/// on a normal release build.
#[cfg(feature = "test-loopback")]
fn ssrf_level_from_env() -> SsrfLevel {
    match std::env::var("ROVER_MCP_SSRF").as_deref() {
        Ok("test_loopback") => SsrfLevel::TestLoopback,
        _ => SsrfLevel::Strict,
    }
}

#[cfg(not(feature = "test-loopback"))]
fn ssrf_level_from_env() -> SsrfLevel {
    SsrfLevel::Strict
}

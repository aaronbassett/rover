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
    cfg.apply_overrides(
        args.rate_limit_rpm,
        args.per_host_concurrency,
        args.global_concurrency,
        args.max_retries,
        args.ignore_robots,
    );
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
        Ok("test_loopback") => SsrfLevel::Loopback,
        _ => SsrfLevel::Strict,
    }
}

#[cfg(not(feature = "test-loopback"))]
fn ssrf_level_from_env() -> SsrfLevel {
    SsrfLevel::Strict
}

//! `rover mcp` subcommand — start the MCP server over stdio.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use crate::config;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp;
use crate::storage::Db;

pub async fn run(config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = Arc::new(config::load(config_path).context("loading config")?);

    let data_dir = data_dir()?;
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let ssrf_level = ssrf_level_from_env();

    mcp::serve_stdio(db, cfg, ssrf_level).await
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        return Ok(PathBuf::from(env_dir));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
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

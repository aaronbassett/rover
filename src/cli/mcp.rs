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

    let ssrf_level = SsrfLevel::parse(&cfg.ssrf.level)
        .with_context(|| format!("invalid [ssrf] level `{}` in config", cfg.ssrf.level))?;
    let ssrf_project_root = if ssrf_level == SsrfLevel::Project {
        let raw = &cfg.ssrf.project_root;
        let resolved = std::fs::canonicalize(raw)
            .with_context(|| format!("canonicalizing ssrf.project_root `{}`", raw.display()))?;
        tracing::info!(
            target: "rover::ssrf",
            project_root = %resolved.display(),
            "ssrf level=project; project_root resolved",
        );
        Some(resolved)
    } else {
        None
    };

    let cfg = Arc::new(cfg);

    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    mcp::serve_stdio(db, cfg, ssrf_level, ssrf_project_root).await
}

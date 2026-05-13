//! `rover fetch <url>` command.
//!
//! As of M2, `rover fetch` runs through the cache-aware orchestrator
//! (`fetcher::cached::fetch_with_cache`). The CLI opens (or creates) the
//! Rover cache database, dispatches the fetch, then renders the resulting
//! `Page` row to stdout as a frontmatter envelope.

use anyhow::Context;
use jiff::Timestamp;
use std::path::{Path, PathBuf};
use url::Url;

use crate::config;
use crate::extractor::frontmatter::{PageMeta, render};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{
    CacheStatus, ExtractResult, FetchOptions, fetch_with_cache, sha256_hex,
};
use crate::fetcher::client::build_http_client;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;

pub struct Args {
    pub url: String,
    pub force_refresh: bool,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = config::load(config_path).context("loading config")?;
    let url = Url::parse(&args.url).context("parsing URL argument")?;
    let level = ssrf_level_for_args(&args);

    let data_dir = data_dir()?;
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());

    let result = fetch_with_cache(
        &db,
        &client,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: args.force_refresh,
            ssrf_level: level,
        },
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(|_| crate::fetcher::FetcherError::Decode)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
            })
        },
    )
    .await
    .context("fetching URL")?;

    if matches!(result.cache_status, CacheStatus::Stale) {
        tracing::warn!(
            target: "rover::cli::fetch",
            url = url.as_str(),
            "serving stale cache entry (network unavailable)"
        );
    }

    let canonical =
        Url::parse(&result.page.canonical_url).context("parsing canonical URL from cache row")?;
    let meta = PageMeta {
        url: &url,
        canonical_url: &canonical,
        title: result.page.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &result.page.extracted_md,
    };

    let envelope = render(&meta);
    print!("{envelope}");
    Ok(())
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        return Ok(PathBuf::from(env_dir));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine local data dir"))?;
    Ok(base.join("rover"))
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

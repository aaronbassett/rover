//! `rover fetch <url>` command.
//!
//! As of M2, `rover fetch` runs through the cache-aware orchestrator
//! (`fetcher::cached::fetch_with_cache`). The CLI opens (or creates) the
//! Rover cache database, dispatches the fetch, then renders the resulting
//! `Page` row to stdout as a frontmatter envelope.

use anyhow::Context;
use jiff::Timestamp;
use std::path::Path;
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
    pub ignore_robots: bool,
    pub rate_limit_rpm: Option<u32>,
    pub per_host_concurrency: Option<u32>,
    pub global_concurrency: Option<u32>,
    pub max_retries: Option<u8>,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
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
    let url = Url::parse(&args.url).context("parsing URL argument")?;
    let level = ssrf_level_for_args(&args);

    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let pacer = crate::fetcher::concurrency::Pacer::new(&cfg.rate_limit);

    let result = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &cfg.rate_limit,
        &cfg.robots,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: args.force_refresh,
            ssrf_level: level,
            ignore_robots: args.ignore_robots,
            user_agent: cfg.fetch.user_agent.clone(),
        },
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
                metadata: extracted.metadata,
            })
        },
    )
    .await
    .context("fetching URL")?;

    if matches!(result.cache_status, CacheStatus::Stale { .. }) {
        tracing::warn!(
            target: "rover::cli::fetch",
            url = url.as_str(),
            "serving stale cache entry (network unavailable)"
        );
    }

    let canonical =
        Url::parse(&result.page.canonical_url).context("parsing canonical URL from cache row")?;

    // Choose the tokenizer for frontmatter `estimated_tokens` from config.
    let family = cfg.tokenizer.default;
    crate::tokenizer::ensure_loaded(family)
        .await
        .context("loading default tokenizer")?;
    let tokens = crate::tokenizer::count(&result.page.extracted_md, family)
        .context("counting tokens for frontmatter")?;

    // Recover the metadata persisted in the cache row (M2 `metadata_json`).
    // If deserialization fails (legacy rows, corrupt JSON), fall back to empty
    // — the frontmatter still renders, just without M4 fields.
    let metadata: crate::extractor::ExtractedMetadata = result
        .page
        .metadata_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    // NB: `raw_html_text_len` is not yet persisted on the cache row; we
    // approximate density from the markdown length itself. In practice that
    // saturates density to 1.0, so quality is dominated by the title and
    // metadata bonuses. M5+ can store the raw length to improve fidelity.
    let quality = crate::extractor::quality::score(
        &result.page.extracted_md,
        result.page.extracted_md.chars().count().max(1),
        !metadata.is_empty(),
        result.page.title.is_some(),
    );
    let meta = PageMeta {
        url: &url,
        canonical_url: &canonical,
        title: result.page.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &result.page.extracted_md,
        tokens,
        tokenizer_name: family.as_str(),
        description: metadata.description.as_deref(),
        author: metadata.author.as_deref(),
        published: metadata.published.as_deref(),
        modified: metadata.modified.as_deref(),
        image: metadata.image.as_deref(),
        og_type: metadata.og_type.as_deref(),
        language: metadata.language.as_deref(),
        schema_types: &metadata.schema_types,
        extraction_quality: quality,
        tables_transformed: &[],
        images_seen: 0,
        images_downloaded: 0,
        images_failed: 0,
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

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

    /// Auto-summarize when extracted markdown exceeds N tokens.
    ///
    /// **v1 note:** the canonical auto-summarize path is the MCP `fetch`
    /// tool. The CLI `fetch` subcommand accepts this flag for
    /// forward-compatibility but does not yet apply summarization in this
    /// milestone — the flag is parsed and validated only.
    pub max_tokens: Option<usize>,

    /// JSON `SummarizeOpts` blob. Same shape as the MCP `summarize` tool
    /// args minus the `url` field, e.g.
    /// `--summarize '{"mode":"abstractive","target_tokens":500}'`.
    ///
    /// **v1 note:** as with `--max-tokens`, the canonical summarization
    /// path is the MCP `fetch` / `summarize` tools. The CLI accepts and
    /// validates this JSON but does not invoke the summarizer in v1.
    pub summarize: Option<String>,
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
    let level = SsrfLevel::parse(&cfg.ssrf.level)
        .with_context(|| format!("invalid [ssrf] level `{}` in config", cfg.ssrf.level))?;
    let ssrf_project_root = if level == SsrfLevel::Project {
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

    // Validate the optional --summarize JSON blob up front so the user
    // gets a clean error before any network or storage I/O. The CLI does
    // not yet thread these through to the summarizer (the canonical path
    // is the MCP `fetch` / `summarize` tools); validating still catches
    // typos and surfaces the flag in `--help`.
    if let Some(s) = args.summarize.as_deref() {
        let _: crate::mcp::tools::fetch::InlineSummarizeArgs =
            serde_json::from_str(s).context("parsing --summarize JSON")?;
    }
    if matches!(args.max_tokens, Some(0)) {
        anyhow::bail!("--max-tokens must be greater than 0");
    }

    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let pacer = crate::fetcher::concurrency::Pacer::new(&cfg.rate_limit);

    // Optional HAR recorder for one-shot CLI runs. We flush once at the end
    // of this subcommand rather than running an interval task — a single
    // `fetch` invocation produces at most a handful of round-trips.
    let har_recorder: Option<std::sync::Arc<crate::fetcher::har::HarRecorder>> =
        if !cfg.debug.har_path.is_empty() {
            let path = std::path::PathBuf::from(&cfg.debug.har_path);
            let r = crate::fetcher::har::HarRecorder::new(path, cfg.debug.har_body_cap)
                .with_context(|| format!("opening har file at {}", cfg.debug.har_path))?;
            Some(std::sync::Arc::new(r))
        } else {
            None
        };

    // M9 fix C1: honor the server-config `auto_detect_spa` flag from the CLI
    // path too. The CLI doesn't yet expose a `--headless` flag, so the only
    // way to opt in is via `[headless] auto_detect_spa = true` in the
    // config. Construction is lazy — we only launch Chromium if Auto-mode
    // ends up needing it (the cached fetcher checks SPA heuristics first).
    let headless_mode = if cfg.headless.auto_detect_spa {
        crate::fetcher::HeadlessMode::Auto
    } else {
        crate::fetcher::HeadlessMode::Off
    };
    #[cfg(feature = "headless")]
    let headless: Option<std::sync::Arc<crate::fetcher::headless::HeadlessRenderer>> =
        if !matches!(headless_mode, crate::fetcher::HeadlessMode::Off) {
            let r = crate::fetcher::headless::HeadlessRenderer::new(&cfg.headless)
                .await
                .map(std::sync::Arc::new)
                .context("launching headless renderer")?;
            Some(r)
        } else {
            None
        };

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
            ssrf_project_root,
            har_recorder: har_recorder.clone(),
            ignore_robots: args.ignore_robots,
            user_agent: cfg.fetch.user_agent.clone(),
            #[cfg(feature = "headless")]
            headless: headless.clone(),
            headless_mode,
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
        images_processed: vec![],
    };

    let envelope = render(&meta);
    print!("{envelope}");

    if let Some(r) = &har_recorder {
        if let Err(e) = r.flush().await {
            tracing::warn!(target: "rover::fetcher", error = ?e, "har flush failed");
        }
    }

    // M9 fix C1: tear down the renderer cleanly so chromiumoxide's handler
    // task doesn't outlive this one-shot CLI invocation. `try_unwrap` is
    // expected to succeed — `fetch_with_cache` returned, so the only other
    // strong reference (the one we passed into `FetchOptions`) is gone.
    #[cfg(feature = "headless")]
    if let Some(renderer) = headless {
        match std::sync::Arc::try_unwrap(renderer) {
            Ok(r) => r.shutdown().await,
            Err(_still_shared) => {
                tracing::warn!(
                    target: "rover::cli::fetch",
                    "headless renderer still has outstanding Arc references at shutdown; skipping explicit shutdown",
                );
            }
        }
    }

    Ok(())
}

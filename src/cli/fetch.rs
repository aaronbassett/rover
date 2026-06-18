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

    /// Auto-summarize when the extracted markdown exceeds N tokens. Runs the
    /// configured `[summarization]` backend (the offline extractive backend
    /// by default) and replaces the body with a summary sized toward the
    /// budget (best-effort; may land a few tokens over).
    pub max_tokens: Option<usize>,

    /// JSON `SummarizeOpts` blob — same shape as the MCP `summarize` tool
    /// args minus the `url` field, e.g.
    /// `--summarize '{"mode":"abstractive","target_tokens":500}'`. Applied
    /// before `--max-tokens`; the body is replaced with the summary.
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

    // Parse the optional --summarize JSON blob up front so the user gets a
    // clean error before any network or storage I/O.
    let summarize_opts: Option<crate::mcp::tools::fetch::InlineSummarizeArgs> =
        match args.summarize.as_deref() {
            Some(s) => Some(serde_json::from_str(s).context("parsing --summarize JSON")?),
            None => None,
        };
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
            // The one-shot CLI has no background scheduler to process a
            // queued `revalidate` task — so on an expired entry we must
            // refresh inline. Otherwise the row would stay at its old
            // `fetched_at` indefinitely (the task we enqueued would
            // outlive the process).
            synchronous_revalidation: true,
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
    let original_tokens = crate::tokenizer::count(&result.page.extracted_md, family)
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
    // Extraction quality is scored on the extracted body, before any
    // summarization — it measures extraction fidelity, not the summary.
    let quality = crate::extractor::quality::score(
        &result.page.extracted_md,
        result.page.extracted_md.chars().count().max(1),
        !metadata.is_empty(),
        result.page.title.is_some(),
    );

    // Optional summarization. Mirrors the MCP `fetch` path: an explicit
    // `--summarize` blob runs first, then `--max-tokens` auto-summarizes when
    // the body is over budget. Built lazily so a plain fetch pays nothing.
    let (body_md, tokens, summarized) = if args.max_tokens.is_some() || summarize_opts.is_some() {
        let registry = std::sync::Arc::new(
            crate::summarizer::registry::build(&cfg, family)
                .context("building summarizer registry")?,
        );
        // Harden content fed to model backends (always-on, matching the MCP
        // path). Prompt-free extractive backends ignore it; cloud/local model
        // backends get cleaned, nonce-wrapped content.
        let guard = std::sync::Arc::new(
            crate::guard::Guard::from_config(&cfg.prompt_injection)
                .context("building prompt-injection guard")?,
        );
        let summarizer = crate::summarizer::SummarizerService::new(
            db.clone(),
            registry,
            cfg.summarization.fallback_to_extractive,
        )
        .with_guard(guard);
        let defaults = crate::summarizer::DefaultsHint::from_config(&cfg.summarization);
        maybe_summarize(
            &summarizer,
            &defaults,
            family,
            result.page.extracted_md.clone(),
            original_tokens,
            args.max_tokens,
            summarize_opts,
        )
        .await?
    } else {
        (result.page.extracted_md.clone(), original_tokens, false)
    };

    let meta = PageMeta {
        url: &url,
        canonical_url: &canonical,
        title: result.page.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &body_md,
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
        summarized,
        tables_transformed: &[],
        images_seen: 0,
        images_downloaded: 0,
        images_failed: 0,
        images_processed: vec![],
        prompt_injection: None,
    };

    let envelope = render(&meta);
    print!("{envelope}");

    if let Some(r) = &har_recorder
        && let Err(e) = r.flush().await
    {
        tracing::warn!(target: "rover::fetcher", error = ?e, "har flush failed");
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

/// Apply optional summarization to `body`, mirroring the MCP `fetch` path.
/// An explicit `--summarize` blob runs first; then `--max-tokens` triggers
/// auto-summarization when the body is still over budget. Returns the
/// (possibly summarized) body, its token count, and whether summarization ran.
async fn maybe_summarize(
    summarizer: &crate::summarizer::SummarizerService,
    defaults: &crate::summarizer::DefaultsHint,
    family: crate::tokenizer::Tokenizer,
    body: String,
    tokens: usize,
    max_tokens: Option<usize>,
    summarize: Option<crate::mcp::tools::fetch::InlineSummarizeArgs>,
) -> anyhow::Result<(String, usize, bool)> {
    let mut body = body;
    let mut tokens = tokens;
    let mut summarized = false;

    // Explicit `--summarize` first: the body becomes the summary.
    if let Some(inline) = summarize {
        let opts = summarizer.resolve_defaults(
            inline.mode.map(Into::into),
            inline.style.map(Into::into),
            inline.target_tokens,
            inline.focus,
            inline.preserve.into_iter().map(Into::into).collect(),
            inline.backend,
            defaults,
        );
        body = compact_body(summarizer, &body, &opts).await?;
        tokens = crate::tokenizer::count(&body, family).context("counting summary tokens")?;
        summarized = true;
    }

    // Auto-summarize on `--max-tokens` overflow. Best-effort: the offline
    // extractive backend budgets by summing per-sentence token counts, so the
    // joined summary can land a few tokens over the target. The CLI budget is
    // a target, not a hard ceiling, so we emit the result rather than failing.
    // (If an explicit `--summarize` already ran, we keep that result.)
    if let Some(max) = max_tokens
        && tokens > max
        && !summarized
    {
        let opts = summarizer.resolve_defaults(None, None, Some(max), None, vec![], None, defaults);
        body = compact_body(summarizer, &body, &opts).await?;
        tokens = crate::tokenizer::count(&body, family).context("counting summary tokens")?;
        summarized = true;
    }

    Ok((body, tokens, summarized))
}

/// Run the summarizer over `body` and return the summary markdown.
async fn compact_body(
    summarizer: &crate::summarizer::SummarizerService,
    body: &str,
    opts: &crate::summarizer::backend::CompactOpts,
) -> anyhow::Result<String> {
    let content_hash = format!("sha256:{}", sha256_hex(body.as_bytes()));
    let r = summarizer
        .compact(&content_hash, body, opts)
        .await
        .context("summarizing extracted markdown")?;
    Ok(r.summary_md)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::{DefaultsHint, SummarizerService};
    use std::sync::Arc;

    fn default_config() -> crate::config::Config {
        toml::from_str("").unwrap()
    }

    async fn service() -> (SummarizerService, DefaultsHint, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path().join("t.db")).await.unwrap();
        let cfg = default_config();
        let family = cfg.tokenizer.default;
        crate::tokenizer::ensure_loaded(family).await.unwrap();
        let registry = Arc::new(crate::summarizer::registry::build(&cfg, family).unwrap());
        let svc = SummarizerService::new(db, registry, cfg.summarization.fallback_to_extractive);
        let defaults = DefaultsHint::from_config(&cfg.summarization);
        (svc, defaults, tmp)
    }

    /// A multi-sentence body the extractive backend can rank and trim.
    fn long_body() -> String {
        let mut s = String::new();
        for i in 0..80 {
            s.push_str(&format!(
                "Sentence number {i} states a distinct and self-contained fact about how a rover \
                 fetches and prepares web content for an agent to reason over. "
            ));
        }
        s
    }

    #[tokio::test]
    async fn passthrough_when_under_budget_and_no_summarize() {
        let (svc, defaults, _tmp) = service().await;
        let family = default_config().tokenizer.default;
        let body = "A short extracted body.".to_string();
        let tokens = crate::tokenizer::count(&body, family).unwrap();
        let (out, out_tokens, summarized) = maybe_summarize(
            &svc,
            &defaults,
            family,
            body.clone(),
            tokens,
            Some(10_000),
            None,
        )
        .await
        .unwrap();
        assert!(!summarized, "should not summarize when under budget");
        assert_eq!(out, body);
        assert_eq!(out_tokens, tokens);
    }

    #[tokio::test]
    async fn explicit_summarize_shrinks_body() {
        let (svc, defaults, _tmp) = service().await;
        let family = default_config().tokenizer.default;
        let body = long_body();
        let tokens = crate::tokenizer::count(&body, family).unwrap();
        let inline = crate::mcp::tools::fetch::InlineSummarizeArgs {
            target_tokens: Some(80),
            ..Default::default()
        };
        let (out, out_tokens, summarized) =
            maybe_summarize(&svc, &defaults, family, body, tokens, None, Some(inline))
                .await
                .unwrap();
        assert!(summarized);
        assert!(!out.is_empty());
        assert!(
            out_tokens < tokens,
            "summary should be smaller than the original ({out_tokens} !< {tokens})"
        );
    }

    #[tokio::test]
    async fn max_tokens_auto_summarizes_over_budget() {
        let (svc, defaults, _tmp) = service().await;
        let family = default_config().tokenizer.default;
        let body = long_body();
        let tokens = crate::tokenizer::count(&body, family).unwrap();
        assert!(
            tokens > 400,
            "fixture should exceed the budget (got {tokens})"
        );
        let (out, out_tokens, summarized) =
            maybe_summarize(&svc, &defaults, family, body, tokens, Some(400), None)
                .await
                .unwrap();
        assert!(summarized);
        assert!(!out.is_empty());
        assert!(
            out_tokens < tokens,
            "auto-summary should be smaller than the original ({out_tokens} !< {tokens})"
        );
    }
}

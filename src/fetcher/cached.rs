//! Cache-aware fetch orchestrator.
//!
//! `fetch_with_cache` is the high-level entry point used by the CLI and the
//! (future) MCP `fetch` tool. It wraps the raw `fetcher::fetch::fetch_url`
//! with cache lookup, TTL-driven freshness, and write-back.
//!
//! Task 7 shipped the orchestrator skeleton (always a full GET on miss/stale).
//! Task 8 added conditional GETs (`If-None-Match` / `If-Modified-Since`),
//! 304 Not Modified handling via `pages::touch`, and real `Cache-Control` /
//! `Expires` header propagation into the TTL decision.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

use super::FetcherError;
use super::fetch::ConditionalGet;
use super::ssrf::SsrfLevel;
use super::ttl::{TtlDecision, compute_ttl};
use crate::config::CacheConfig;
use crate::extractor::metadata::ExtractedMetadata;
use crate::storage::Db;
use crate::storage::pages::{self, Page, url_hash};

/// Outcome of a cache-aware fetch.
///
/// `Stale` carries the id of the `revalidate` task that was enqueued when
/// the SWR fast-path (M6) returned the expired row. `None` means the row
/// was served stale but the task insert failed (logged; not fatal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Stale {
        revalidation_task_id: Option<String>,
    },
    Miss,
}

/// What `fetch_with_cache` returns: a Page (cache hit/miss/stale) plus the
/// cache_status that produced it. The Page mirrors the storage row so the
/// caller has both extracted_md and metadata available.
#[derive(Debug, Clone)]
pub struct CachedFetch {
    pub page: Page,
    pub cache_status: CacheStatus,
}

/// Per-call headless mode selection.
///
/// Defined here (not behind `#[cfg(feature = "headless")]`) so every call site
/// can use `HeadlessMode::Off` without conditional compilation. The headless
/// module's own `HeadlessMode` (in `src/fetcher/headless/mod.rs`) is the same
/// shape and is interconvertible via `as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadlessMode {
    /// Never use the headless renderer (default).
    #[default]
    Off,
    /// Always use the headless renderer.
    On,
    /// Use the headless renderer only when SPA heuristics trigger.
    Auto,
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
    /// Required (some) when `ssrf_level == Project`. Must be pre-canonicalized.
    pub ssrf_project_root: Option<std::path::PathBuf>,
    /// Optional HAR recorder. When `Some`, every round-trip is recorded.
    pub har_recorder: Option<std::sync::Arc<crate::fetcher::har::HarRecorder>>,
    /// When `true`, skip the robots gate. Used by `--ignore-robots`.
    pub ignore_robots: bool,
    /// User-Agent used for robots.txt UA-rule evaluation. Must match
    /// `[fetch] user_agent`.
    pub user_agent: String,
    /// M9: lazily-initialized headless renderer handle (`Some` when the binary
    /// was built with `--features headless` AND the caller wired one). The
    /// browser is launched on first use inside `fetch_with_cache` — only when a
    /// render actually happens (SPA detected, bot-challenge bypass, or
    /// `On` mode), never for a plain reqwest fetch.
    #[cfg(feature = "headless")]
    pub headless: Option<crate::fetcher::headless::HeadlessHandle>,
    /// M9: per-call mode selection.
    pub headless_mode: HeadlessMode,
    /// When `true`, the caller opts out of the stale-while-revalidate
    /// fast-path: on an expired cache entry, `fetch_with_cache` performs
    /// the network refresh inline rather than serving stale and queueing
    /// a background `revalidate` task.
    ///
    /// Set this from any caller that does NOT have a running task
    /// scheduler in the same process — chiefly the one-shot CLI. The
    /// MCP server's tools leave this `false` so the agent gets a fast
    /// response and the in-process scheduler refreshes the row.
    ///
    /// Independently of this flag, the row is also re-fetched
    /// synchronously when the row expired more than
    /// `[cache] stale_while_revalidate_window` ago, so callers never
    /// receive arbitrarily old content.
    pub synchronous_revalidation: bool,
}

/// What `fetch_with_cache` needs from the extractor. Defined here as a tiny
/// adapter so the extractor module isn't a hard dependency of the fetcher.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub title: Option<String>,
    pub body_md: String,
    pub content_hash: String,
    pub metadata: ExtractedMetadata,
}

/// Cache-aware fetch entry point.
///
/// The extraction step is delegated to `extract_fn`: this keeps the fetcher
/// independent of the extractor module. The CLI/MCP layer wires up
/// `extractor::pipeline::extract`.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_with_cache<F>(
    db: &Db,
    client: &reqwest::Client,
    pacer: &crate::fetcher::concurrency::Pacer,
    rate_cfg: &crate::config::RateLimitConfig,
    robots_cfg: &crate::config::RobotsConfig,
    url: &Url,
    cache_cfg: &CacheConfig,
    opts: FetchOptions,
    mut extract_fn: F,
) -> Result<CachedFetch, FetcherError>
where
    F: FnMut(&str, &Url) -> Result<ExtractResult, FetcherError>,
{
    let now = Timestamp::now().as_second();

    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(crate::fetcher::ssrf::SsrfError::NoHost))?;

    // Robots gate (M5). Skipped when explicitly disabled or for ignore_domains.
    let robots_skipped = !robots_cfg.respect
        || opts.ignore_robots
        || robots_cfg.ignore_domains.iter().any(|d| d == host);
    let crawl_delay: Option<std::time::Duration> = if robots_skipped {
        None
    } else {
        let entry = crate::fetcher::robots::ensure_entry(
            db,
            pacer,
            client,
            robots_cfg,
            host,
            opts.ssrf_level,
            &opts.user_agent,
            rate_cfg,
        )
        .await?;

        let verdict = crate::fetcher::robots::evaluate(&entry, &opts.user_agent, url.path());
        if matches!(verdict, crate::fetcher::robots::Verdict::Disallowed) {
            return Err(FetcherError::RobotsDisallowed {
                url: url.to_string(),
                ua: opts.user_agent.clone(),
            });
        }
        crate::fetcher::robots::crawl_delay(&entry, &opts.user_agent)
    };

    // Step 1: cache lookup.
    //
    // Three outcomes for an expired row:
    //  1. Fresh hit (`expires_at > now`) — return immediately.
    //  2. Expired within the SWR grace window AND caller hasn't opted out
    //     of SWR → serve stale now, queue a `revalidate` task in the
    //     background. The agent monitors the task id; the row gets
    //     refreshed out-of-band.
    //  3. Expired beyond the grace window, OR caller asked for
    //     synchronous behaviour (e.g. CLI) → fall through to the network
    //     refresh path, keeping the stale row threaded down for the
    //     conditional-GET validators in Step 2.
    //
    // The grace window stops the SWR path from ever returning arbitrarily
    // old content; without it, an entry that expired weeks ago would still
    // be served stale on every fetch (because nothing was refreshing it).
    let swr_window_secs = cache_cfg.stale_while_revalidate_window.as_secs() as i64;
    let stale: Option<Page> = if opts.force_refresh {
        None
    } else {
        match lookup_cached(db, url).await? {
            Some(p) if p.expires_at.is_some_and(|e| e > now) => {
                return Ok(CachedFetch {
                    page: p,
                    cache_status: CacheStatus::Hit,
                });
            }
            Some(p) => {
                let within_swr_window = p
                    .expires_at
                    .is_some_and(|e| now.saturating_sub(e) <= swr_window_secs);
                if within_swr_window && !opts.synchronous_revalidation {
                    // SWR fast-path: queue a revalidate task, return stale now.
                    let task_id = insert_revalidate_task(db, url, &p).await;
                    return Ok(CachedFetch {
                        page: p,
                        cache_status: CacheStatus::Stale {
                            revalidation_task_id: task_id,
                        },
                    });
                }
                // Treat as a miss. The stale row is kept around so Step 2
                // can build conditional validators from it.
                Some(p)
            }
            None => None,
        }
    };

    // Step 2: build conditional validators from any stale entry. `stale`
    // is Some when we're synchronously revalidating an expired row (either
    // because the caller opted out of SWR or the row expired beyond the
    // grace window) — in that case we forward `If-None-Match` /
    // `If-Modified-Since` so a 304 lets us extend the freshness on the
    // existing row instead of re-extracting.
    let cond = match &stale {
        Some(p) => ConditionalGet {
            if_none_match: p.etag.clone(),
            if_modified_since: p.last_modified.clone(),
        },
        None => ConditionalGet::default(),
    };

    // Step 3: fetch (conditional if validators present).
    //
    // M9 mode dispatch:
    //   - `Off` (default): today's reqwest path, unchanged.
    //   - `Auto`: reqwest path now, optional headless re-render after extract.
    //   - `On`: bypass reqwest entirely; render via the headless browser.
    //
    // The `On` branch synthesizes a `FetchedPage`-shaped value from the
    // renderer output so step 6 (TTL) and step 7 (store) work unchanged.
    // Track *why* the headless renderer was used, if at all, so the stored row
    // (and the fetch frontmatter) records how this content was obtained. Set at
    // each render site below; `None` means a plain HTTP fetch.
    #[cfg(feature = "headless")]
    let mut render_reason: Option<&'static str> = None;

    let fetched = match opts.headless_mode {
        HeadlessMode::Off | HeadlessMode::Auto => {
            let retry_result = crate::fetcher::retry::with_retries(
                db,
                pacer,
                client,
                url,
                opts.ssrf_level,
                opts.ssrf_project_root.as_deref(),
                opts.har_recorder.as_ref(),
                &cond,
                crawl_delay,
                rate_cfg,
            )
            .await;

            // Bot-challenge bypass (Auto mode only): a managed challenge
            // (Vercel/Cloudflare) returns no usable content to a plain HTTP
            // client, but the headless browser executes the JS challenge like a
            // real browser and reaches the page. Upgrade the challenge error
            // into a headless render here; on bypass failure, reconstruct the
            // original challenge error so the stale/propagate logic below is
            // unchanged.
            #[cfg(feature = "headless")]
            let retry_result = match retry_result {
                Err(FetcherError::BotChallenge {
                    url: ch_url,
                    provider,
                }) if opts.headless_mode == HeadlessMode::Auto && opts.headless.is_some() => {
                    let handle = opts.headless.as_ref().expect("guarded by is_some()");
                    auto_render_delay(handle, url, "bot_challenge_bypass").await;
                    match handle.get().await {
                        Ok(r) => match r
                            .render(url, opts.ssrf_level, opts.ssrf_project_root.as_deref())
                            .await
                        {
                            Ok(rendered) => {
                                tracing::info!(target: "rover::fetcher::cached",
                                    url = url.as_str(), provider = %provider,
                                    "bot-protection challenge on HTTP fetch; bypassed via headless render");
                                render_reason = Some("bot_challenge");
                                Ok(rendered_to_fetched(rendered))
                            }
                            Err(render_err) => {
                                tracing::warn!(target: "rover::fetcher::cached",
                                    error = %render_err, url = url.as_str(), provider = %provider,
                                    "headless bypass of bot-protection challenge failed; returning challenge error");
                                Err(FetcherError::BotChallenge {
                                    url: ch_url,
                                    provider,
                                })
                            }
                        },
                        Err(launch_err) => {
                            tracing::warn!(target: "rover::fetcher::cached",
                                error = %launch_err, url = url.as_str(), provider = %provider,
                                "could not launch headless renderer to bypass bot-protection challenge; returning challenge error");
                            Err(FetcherError::BotChallenge {
                                url: ch_url,
                                provider,
                            })
                        }
                    }
                }
                other => other,
            };

            match retry_result {
                Ok(f) => f,
                Err(e) => {
                    // Network failure with a stale entry available. Serve the
                    // stale row only if it's still within the SWR grace
                    // window — beyond that we'd rather propagate the error
                    // than misrepresent very old content as a successful
                    // fetch. Caller can retry with `--force-refresh` or wait
                    // for the upstream to recover.
                    if let Some(s) = stale {
                        let within_window = s
                            .expires_at
                            .is_some_and(|exp| now.saturating_sub(exp) <= swr_window_secs);
                        if within_window {
                            tracing::warn!(target: "rover::fetcher::cached",
                                error = %e, url = url.as_str(), "fetch failed; serving stale within SWR window");
                            let task_id = insert_revalidate_task(db, url, &s).await;
                            return Ok(CachedFetch {
                                page: s,
                                cache_status: CacheStatus::Stale {
                                    revalidation_task_id: task_id,
                                },
                            });
                        }
                        tracing::warn!(target: "rover::fetcher::cached",
                            error = %e, url = url.as_str(),
                            "fetch failed; stale entry is beyond SWR window — propagating error rather than serving very old content");
                    }
                    return Err(e);
                }
            }
        }
        HeadlessMode::On => {
            #[cfg(not(feature = "headless"))]
            {
                return Err(FetcherError::HeadlessFeatureNotCompiled);
            }
            #[cfg(feature = "headless")]
            {
                let r = opts
                    .headless
                    .as_ref()
                    .ok_or(FetcherError::HeadlessRendererUnavailable)?
                    .get()
                    .await?;
                let rendered = r
                    .render(url, opts.ssrf_level, opts.ssrf_project_root.as_deref())
                    .await?;
                render_reason = Some("on");
                rendered_to_fetched(rendered)
            }
        }
    };

    // Step 4: 304 Not Modified — extend freshness on the stale row and serve it.
    // With M6 SWR, conditional GETs are issued only when `force_refresh = true`
    // *and* a stale row was somehow threaded through; the no-force_refresh path
    // returns stale early. This block remains as a safety net.
    //
    // 304 is only possible via the reqwest path; the headless render
    // synthesizes status=200 so this branch is naturally skipped for `On`.
    if fetched.status == 304 {
        let stale = stale.expect("304 implies a stale entry was sent");
        let decision = compute_ttl(
            now,
            host,
            fetched.cache_control.as_deref().unwrap_or(""),
            fetched.expires.as_deref(),
            cache_cfg,
        );
        let expires_at = match decision {
            TtlDecision::Cache { expires_at } => Some(expires_at),
            TtlDecision::DoNotCache => None,
        };
        pages::touch(db, &stale.url_hash, now, expires_at)
            .await
            .map_err(map_storage_err)?;
        let mut page = stale;
        page.fetched_at = now;
        page.expires_at = expires_at;
        return Ok(CachedFetch {
            page,
            cache_status: CacheStatus::Hit,
        });
    }

    if !(200..300).contains(&fetched.status) {
        return Err(FetcherError::Status {
            status: fetched.status,
            url: fetched.final_url.to_string(),
        });
    }

    // Step 5: extract.
    let extracted = extract_fn(&fetched.body, &fetched.final_url)?;

    // M9 Auto-mode SPA heuristic: if the reqwest result looks like an
    // unrendered SPA (`detect_spa(...).total >= 2`), re-render via headless
    // and re-extract. When the feature is compiled out or the renderer isn't
    // wired, we silently keep the reqwest extraction.
    let (fetched, extracted) = if opts.headless_mode == HeadlessMode::Auto {
        #[cfg(feature = "headless")]
        {
            if let Some(h) = opts.headless.as_ref() {
                let hits =
                    crate::fetcher::headless::detect::detect_spa(&fetched.body, &extracted.body_md);
                if hits.total >= 2 {
                    auto_render_delay(h, url, "spa_rerender").await;
                    // `auto` only ever promised to render if it could. The
                    // plain-HTTP fetch above already succeeded and already
                    // extracted, so a renderer that won't launch (no seccomp
                    // profile in a container, no browser on the host) must
                    // degrade to that result rather than discard it — the same
                    // shape as the bot-challenge path above, which returns its
                    // pre-render outcome on both a launch and a render failure.
                    //
                    // No browser ran, so this is not a security compromise:
                    // refusing to *render* without a sandbox is the property,
                    // and it stays intact. The failure is not memoised — every
                    // subsequent Auto fetch retries the launch.
                    match h.get().await {
                        Ok(r) => {
                            match r
                                .render(url, opts.ssrf_level, opts.ssrf_project_root.as_deref())
                                .await
                            {
                                Ok(rendered) => {
                                    let f2 = rendered_to_fetched(rendered);
                                    let e2 = extract_fn(&f2.body, &f2.final_url)?;
                                    render_reason = Some("spa");
                                    (f2, e2)
                                }
                                Err(render_err) => {
                                    tracing::warn!(target: "rover::fetcher::cached",
                                        error = %render_err, url = url.as_str(),
                                        "headless re-render of a suspected SPA failed; returning the plain HTTP extraction");
                                    (fetched, extracted)
                                }
                            }
                        }
                        Err(launch_err) => {
                            tracing::warn!(target: "rover::fetcher::cached",
                                error = %launch_err, url = url.as_str(),
                                "could not launch headless renderer to re-render a suspected SPA; returning the plain HTTP extraction");
                            (fetched, extracted)
                        }
                    }
                } else {
                    (fetched, extracted)
                }
            } else {
                (fetched, extracted)
            }
        }
        #[cfg(not(feature = "headless"))]
        {
            (fetched, extracted)
        }
    } else {
        (fetched, extracted)
    };

    // Step 6: TTL from real Cache-Control / Expires headers.
    let decision = compute_ttl(
        now,
        host,
        fetched.cache_control.as_deref().unwrap_or(""),
        fetched.expires.as_deref(),
        cache_cfg,
    );

    let expires_at = match decision {
        TtlDecision::Cache { expires_at } => Some(expires_at),
        TtlDecision::DoNotCache => None,
    };

    let new_hash = url_hash(fetched.canonical_url.as_str());
    let metadata_json = serde_json::to_string(&extracted.metadata).ok();
    // Only retain the raw body when the operator opted in via `[cache]
    // store_raw_html`. We clone the decoded UTF-8 body's bytes: the cost is
    // proportional to the page size, but only paid on the fresh-fetch path.
    let raw_html = if cache_cfg.store_raw_html {
        Some(fetched.body.as_bytes().to_vec())
    } else {
        None
    };
    // Resolve the tracked headless reason into the persisted column. In a
    // non-headless build nothing can render, so it is always `None`.
    #[cfg(feature = "headless")]
    let render_reason = render_reason.map(str::to_owned);
    #[cfg(not(feature = "headless"))]
    let render_reason: Option<String> = None;
    let page = Page {
        url_hash: new_hash,
        url: url.as_str().to_owned(),
        canonical_url: fetched.canonical_url.as_str().to_owned(),
        title: extracted.title.clone(),
        fetched_at: now,
        expires_at,
        etag: fetched.etag.clone(),
        last_modified: fetched.last_modified.clone(),
        content_hash: extracted.content_hash.clone(),
        extracted_md: extracted.body_md.clone(),
        metadata_json,
        raw_html,
        render_reason,
    };

    // Step 7: store (only if cacheable).
    if expires_at.is_some() {
        pages::upsert(db, page.clone())
            .await
            .map_err(map_storage_err)?;
    }

    Ok(CachedFetch {
        page,
        cache_status: CacheStatus::Miss,
    })
}

/// Convert a `RenderedPage` (from the headless renderer) into a `FetchedPage`
/// so the rest of `fetch_with_cache` (TTL → store) can treat it uniformly.
///
/// Synthesizes empty cache headers, no ETag, no Last-Modified. The TTL
/// computation will therefore fall through to the default-TTL policy. The
/// canonical URL is resolved from the rendered DOM (`<link rel="canonical">`)
/// or falls back to the final URL.
/// Apply the configured Auto-mode pre-render delay before escalating to the
/// headless browser. Runs *after* the render trigger has been detected (SPA
/// heuristic fired, or a bot-challenge was returned) and *before* the browser
/// is launched/driven, giving the origin a breather between the lightweight
/// HTTP fetch and the heavier browser hit. A no-op when configured to zero.
#[cfg(feature = "headless")]
async fn auto_render_delay(
    handle: &crate::fetcher::headless::HeadlessHandle,
    url: &url::Url,
    reason: &'static str,
) {
    let delay = handle.launch_delay();
    if delay.is_zero() {
        return;
    }
    tracing::debug!(
        target: "rover::fetcher::cached",
        url = url.as_str(),
        delay_secs = delay.as_secs(),
        reason,
        "Auto-mode pre-render delay before headless launch",
    );
    tokio::time::sleep(delay).await;
}

#[cfg(feature = "headless")]
fn rendered_to_fetched(
    rendered: crate::fetcher::headless::RenderedPage,
) -> crate::fetcher::FetchedPage {
    use crate::fetcher::FetchedPage;
    use crate::fetcher::canonical::extract_canonical_url;
    use crate::fetcher::charset::Detected;

    let canonical_url = extract_canonical_url(&rendered.html, &rendered.final_url, None);
    FetchedPage {
        final_url: rendered.final_url,
        canonical_url,
        status: rendered.status,
        content_type: Some("text/html; charset=utf-8".to_string()),
        body: rendered.html,
        charset: Detected::default(),
        link_header: None,
        etag: None,
        last_modified: None,
        cache_control: None,
        expires: None,
        retry_after: None,
    }
}

/// Compute sha256 hex of bytes. Centralized here so callers don't have to
/// pull in `sha2` directly.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write as _;
        write!(s, "{b:02x}").expect("write to String never fails");
    }
    s
}

async fn lookup_cached(db: &Db, url: &Url) -> Result<Option<Page>, FetcherError> {
    let hash = url_hash(url.as_str());
    if let Some(p) = pages::get_by_url_hash(db, &hash)
        .await
        .map_err(map_storage_err)?
    {
        return Ok(Some(p));
    }
    pages::get_by_url(db, url.as_str())
        .await
        .map_err(map_storage_err)
}

fn map_storage_err(e: crate::storage::StorageError) -> FetcherError {
    tracing::error!(target: "rover::fetcher::cached", error = %e, "storage error");
    FetcherError::Storage(e)
}

/// Enqueue a `revalidate` task for an expired cache row. Returns the task id
/// on success. Failures are logged and swallowed: a stale-served response is
/// still a useful answer to the agent, and the worker will re-enqueue on the
/// next miss.
async fn insert_revalidate_task(db: &Db, url: &Url, stale: &Page) -> Option<String> {
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    let params = serde_json::to_string(&crate::tasks::types::RevalidateParams {
        url: url.to_string(),
        etag_at_serve: stale.etag.clone(),
        last_modified_at_serve: stale.last_modified.clone(),
    })
    .ok()?;
    let id = uuid::Uuid::now_v7().to_string();
    match insert(
        db,
        TaskInsert {
            id: id.clone(),
            kind: TaskKind::Revalidate,
            params_json: params,
            owner_pid: Some(std::process::id() as i64),
        },
    )
    .await
    {
        Ok(()) => Some(id),
        Err(e) => {
            tracing::warn!(
                target: "rover::fetcher::cached",
                error = %e,
                url = url.as_str(),
                "failed to enqueue revalidate task; serving stale without revalidation",
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_status_eq() {
        assert_ne!(
            CacheStatus::Hit,
            CacheStatus::Stale {
                revalidation_task_id: None
            }
        );
    }

    #[test]
    fn map_storage_err_routes_to_storage_variant() {
        // Regression: previously collapsed every StorageError into FetcherError::Decode,
        // producing the misleading "response decoding failed" message for DB failures.
        let storage_err = crate::storage::StorageError::from(rusqlite::Error::QueryReturnedNoRows);
        let mapped = map_storage_err(storage_err);
        assert!(matches!(mapped, FetcherError::Storage(_)));
        assert!(mapped.to_string().starts_with("storage error:"));
    }

    #[test]
    fn sha256_hex_matches_known() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[tokio::test]
    async fn cache_hit_within_ttl() {
        use crate::config::{RateLimitConfig, RobotsConfig};
        use crate::fetcher::concurrency::Pacer;
        use crate::storage::Db;
        use std::time::Duration;
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let url = Url::parse("https://example.com/").unwrap();
        let now = Timestamp::now().as_second();
        let page = Page {
            url_hash: url_hash(url.as_str()),
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("cached".into()),
            fetched_at: now - 60,
            expires_at: Some(now + 600),
            etag: None,
            last_modified: None,
            content_hash: "x".into(),
            extracted_md: "# cached".into(),
            metadata_json: None,
            raw_html: None,
            render_reason: None,
        };
        pages::upsert(&db, page.clone()).await.unwrap();

        let cache_cfg = CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(60),
            max_ttl: Duration::from_secs(86400),
            stale_while_revalidate_window: Duration::from_secs(300),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        };
        let rate_cfg = RateLimitConfig::default();
        // avoid robots fetch in this unit test
        let robots_cfg = RobotsConfig {
            respect: false,
            ..RobotsConfig::default()
        };
        let pacer = Pacer::new(&rate_cfg);
        let client = super::super::client::build_http_client("test/0.1", Duration::from_secs(5));
        let result = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate_cfg,
            &robots_cfg,
            &url,
            &cache_cfg,
            FetchOptions {
                force_refresh: false,
                ssrf_level: SsrfLevel::Strict,
                ssrf_project_root: None,
                har_recorder: None,
                ignore_robots: false,
                user_agent: "test/0.1".into(),
                #[cfg(feature = "headless")]
                headless: None,
                headless_mode: HeadlessMode::Off,
                synchronous_revalidation: false,
            },
            |_, _| {
                panic!("extract_fn must not be called on cache hit");
            },
        )
        .await
        .unwrap();
        assert_eq!(result.cache_status, CacheStatus::Hit);
        assert_eq!(result.page.title.as_deref(), Some("cached"));
    }

    // The remaining tests in this module exercise the three new corners of
    // the SWR / synchronous-revalidation decision matrix introduced
    // alongside the `stale_while_revalidate_window` config:
    //
    //   - expired within window, sync flag off  → SWR (return stale, queue task)
    //   - expired beyond window, sync flag off  → fall through to sync fetch
    //   - expired within window, sync flag on   → fall through to sync fetch
    //
    // The "cache hit" arm is covered by `cache_hit_within_ttl` above. The
    // "force_refresh" arm is exercised by tests/fetcher_full_loop.rs and
    // tests/fetcher_retry.rs and is unchanged by this branch.

    #[cfg(any(test, feature = "test-loopback"))]
    async fn build_swr_test_fixture(
        swr_window: std::time::Duration,
    ) -> (
        crate::storage::Db,
        Url,
        crate::config::CacheConfig,
        crate::config::RateLimitConfig,
        crate::config::RobotsConfig,
        crate::fetcher::concurrency::Pacer,
        reqwest::Client,
        tempfile::TempDir,
    ) {
        use crate::config::{RateLimitConfig, RobotsConfig};
        use crate::fetcher::concurrency::Pacer;
        use crate::storage::Db;
        use std::time::Duration;
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let cache_cfg = CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(0),
            max_ttl: Duration::from_secs(86400),
            stale_while_revalidate_window: swr_window,
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        };
        let rate_cfg = RateLimitConfig::default();
        let robots_cfg = RobotsConfig {
            respect: false,
            ..RobotsConfig::default()
        };
        let pacer = Pacer::new(&rate_cfg);
        let client = crate::fetcher::client::build_http_client("test/0.1", Duration::from_secs(5));
        // Concrete URL is supplied by callers (each wiremock server has a
        // different port).
        let url = Url::parse("https://placeholder.invalid/").unwrap();
        (tmp, db, url, cache_cfg, rate_cfg, robots_cfg, pacer, client).into_unzipped()
    }

    // Helper trait — tuple-swap to escape the borrow checker on the temp dir
    // (the tempdir handle must outlive the Db).
    #[cfg(any(test, feature = "test-loopback"))]
    trait IntoUnzipped {
        type Output;
        fn into_unzipped(self) -> Self::Output;
    }
    #[cfg(any(test, feature = "test-loopback"))]
    impl IntoUnzipped
        for (
            tempfile::TempDir,
            crate::storage::Db,
            Url,
            crate::config::CacheConfig,
            crate::config::RateLimitConfig,
            crate::config::RobotsConfig,
            crate::fetcher::concurrency::Pacer,
            reqwest::Client,
        )
    {
        type Output = (
            crate::storage::Db,
            Url,
            crate::config::CacheConfig,
            crate::config::RateLimitConfig,
            crate::config::RobotsConfig,
            crate::fetcher::concurrency::Pacer,
            reqwest::Client,
            tempfile::TempDir,
        );
        fn into_unzipped(self) -> Self::Output {
            (
                self.1, self.2, self.3, self.4, self.5, self.6, self.7, self.0,
            )
        }
    }

    #[cfg(any(test, feature = "test-loopback"))]
    async fn insert_expired_page(
        db: &crate::storage::Db,
        url: &Url,
        now: i64,
        expired_secs_ago: i64,
    ) {
        let page = Page {
            url_hash: url_hash(url.as_str()),
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("old".into()),
            fetched_at: now - expired_secs_ago - 60,
            expires_at: Some(now - expired_secs_ago),
            etag: None,
            last_modified: None,
            content_hash: "old-hash".into(),
            extracted_md: "# old".into(),
            metadata_json: None,
            raw_html: None,
            render_reason: None,
        };
        pages::upsert(db, page).await.unwrap();
    }

    fn fetch_opts_with_sync(sync: bool) -> FetchOptions {
        FetchOptions {
            force_refresh: false,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
            #[cfg(feature = "headless")]
            headless: None,
            headless_mode: HeadlessMode::Off,
            synchronous_revalidation: sync,
        }
    }

    #[cfg(any(test, feature = "test-loopback"))]
    #[tokio::test]
    async fn expired_within_window_serves_stale_swr() {
        use std::time::Duration;
        let (db, _placeholder, cache_cfg, rate_cfg, robots_cfg, pacer, client, _tmp) =
            build_swr_test_fixture(Duration::from_secs(300)).await;
        let url = Url::parse("https://example.com/within-window").unwrap();
        let now = Timestamp::now().as_second();
        insert_expired_page(&db, &url, now, 10).await; // expired 10s ago, window 300s

        let result = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate_cfg,
            &robots_cfg,
            &url,
            &cache_cfg,
            fetch_opts_with_sync(false),
            |_, _| panic!("extract_fn must not be called on SWR stale-serve"),
        )
        .await
        .expect("SWR path must succeed");
        let task_id = match &result.cache_status {
            CacheStatus::Stale {
                revalidation_task_id,
            } => revalidation_task_id
                .as_ref()
                .expect("SWR path must enqueue a revalidate task"),
            other => panic!("expected CacheStatus::Stale, got {other:?}"),
        };
        // Confirm the task row actually landed in storage.
        let row = crate::storage::tasks::get(&db, task_id)
            .await
            .unwrap()
            .expect("revalidate task row present after SWR fast-path");
        assert_eq!(row.kind, crate::storage::tasks::TaskKind::Revalidate);
    }

    #[cfg(any(test, feature = "test-loopback"))]
    #[tokio::test]
    async fn expired_beyond_window_falls_through_to_sync_fetch() {
        use std::time::Duration;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body>fresh content here</body></html>")
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .insert_header("cache-control", "max-age=60"),
            )
            .mount(&server)
            .await;

        let (db, _placeholder, cache_cfg, rate_cfg, robots_cfg, pacer, client, _tmp) =
            build_swr_test_fixture(Duration::from_secs(300)).await;
        let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
        let now = Timestamp::now().as_second();
        insert_expired_page(&db, &url, now, 3600).await; // expired 1h ago, window 5min

        let result = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate_cfg,
            &robots_cfg,
            &url,
            &cache_cfg,
            fetch_opts_with_sync(false), // caller did NOT request sync — grace window forces it
            |_body, _base| {
                Ok(ExtractResult {
                    title: Some("fresh".into()),
                    body_md: "fresh".into(),
                    content_hash: "fresh-hash".into(),
                    metadata: crate::extractor::metadata::ExtractedMetadata::default(),
                })
            },
        )
        .await
        .expect("beyond-window expired entry must trigger a sync fetch");
        assert_eq!(result.cache_status, CacheStatus::Miss);
        // Row should now carry the new content_hash and a fresh fetched_at.
        let row = pages::get_by_url(&db, url.as_str())
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(row.content_hash, "fresh-hash");
        assert!(row.fetched_at >= now);
        // Wiremock saw exactly one request (no double-fetch).
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[cfg(any(test, feature = "test-loopback"))]
    #[tokio::test]
    async fn synchronous_revalidation_bypasses_swr_within_window() {
        use std::time::Duration;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body>fresh</body></html>")
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .insert_header("cache-control", "max-age=60"),
            )
            .mount(&server)
            .await;

        let (db, _placeholder, cache_cfg, rate_cfg, robots_cfg, pacer, client, _tmp) =
            build_swr_test_fixture(Duration::from_secs(300)).await;
        let url = Url::parse(&format!("{}/y", server.uri())).unwrap();
        let now = Timestamp::now().as_second();
        insert_expired_page(&db, &url, now, 10).await; // within window…

        let result = fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate_cfg,
            &robots_cfg,
            &url,
            &cache_cfg,
            fetch_opts_with_sync(true), // …but caller (e.g. CLI) opted out of SWR
            |_body, _base| {
                Ok(ExtractResult {
                    title: Some("fresh".into()),
                    body_md: "fresh".into(),
                    content_hash: "fresh-hash".into(),
                    metadata: crate::extractor::metadata::ExtractedMetadata::default(),
                })
            },
        )
        .await
        .expect("synchronous opt-out must trigger a sync fetch");
        assert_eq!(result.cache_status, CacheStatus::Miss);
        let row = pages::get_by_url(&db, url.as_str())
            .await
            .unwrap()
            .expect("row present");
        assert!(row.fetched_at >= now);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

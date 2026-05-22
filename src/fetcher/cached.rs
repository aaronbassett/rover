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

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
    /// When `true`, skip the robots gate. Used by `--ignore-robots`.
    pub ignore_robots: bool,
    /// User-Agent used for robots.txt UA-rule evaluation. Must match
    /// `[fetch] user_agent`.
    pub user_agent: String,
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
    // M6 SWR: fresh hits short-circuit; expired entries return *immediately*
    // and a `revalidate` task is queued in the background. The caller surfaces
    // the task id in the `revalidation` envelope on the wire.
    //
    // The only remaining path that runs Step 2+ in practice is `force_refresh`
    // or a true cache miss. `stale` therefore stays `None` outside the early
    // return — the network-failure fallback below is reachable only on
    // `force_refresh = true` (kept for defense in depth).
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
                // SWR fast-path: queue a revalidate task, return stale now.
                let task_id = insert_revalidate_task(db, url, &p).await;
                return Ok(CachedFetch {
                    page: p,
                    cache_status: CacheStatus::Stale {
                        revalidation_task_id: task_id,
                    },
                });
            }
            None => None,
        }
    };

    // Step 2: build conditional validators from any stale entry.
    // With the M6 SWR fast-path above, `stale` is always `None` here on the
    // non-`force_refresh` branch, so this collapses to `ConditionalGet::default()`.
    // The match is kept verbatim for the `force_refresh = true` edge case
    // where a future change might surface validators differently.
    let cond = match &stale {
        Some(p) => ConditionalGet {
            if_none_match: p.etag.clone(),
            if_modified_since: p.last_modified.clone(),
        },
        None => ConditionalGet::default(),
    };

    // Step 3: fetch (conditional if validators present).
    let fetched = match crate::fetcher::retry::with_retries(
        db,
        pacer,
        client,
        url,
        opts.ssrf_level,
        &cond,
        crawl_delay,
        rate_cfg,
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            // Network failure with a stale entry available → return stale and
            // queue a revalidate task (defense-in-depth; with SWR this only
            // fires on `force_refresh = true`, since the no-force_refresh path
            // returned stale eagerly above).
            if let Some(s) = stale {
                tracing::warn!(target: "rover::fetcher::cached",
                    error = %e, url = url.as_str(), "fetch failed; serving stale");
                let task_id = insert_revalidate_task(db, url, &s).await;
                return Ok(CachedFetch {
                    page: s,
                    cache_status: CacheStatus::Stale {
                        revalidation_task_id: task_id,
                    },
                });
            }
            return Err(e);
        }
    };

    // Step 4: 304 Not Modified — extend freshness on the stale row and serve it.
    // With M6 SWR, conditional GETs are issued only when `force_refresh = true`
    // *and* a stale row was somehow threaded through; the no-force_refresh path
    // returns stale early. This block remains as a safety net.
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
        };
        pages::upsert(&db, page.clone()).await.unwrap();

        let cache_cfg = CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(60),
            max_ttl: Duration::from_secs(86400),
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
                ignore_robots: false,
                user_agent: "test/0.1".into(),
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
}

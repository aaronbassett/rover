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
use super::fetch::{ConditionalGet, fetch_url_conditional};
use super::ssrf::SsrfLevel;
use super::ttl::{TtlDecision, compute_ttl};
use crate::config::CacheConfig;
use crate::storage::Db;
use crate::storage::pages::{self, Page, url_hash};

/// Outcome of a cache-aware fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Stale,
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

#[derive(Debug, Clone, Copy)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
}

/// What `fetch_with_cache` needs from the extractor. Defined here as a tiny
/// adapter so the extractor module isn't a hard dependency of the fetcher.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub title: Option<String>,
    pub body_md: String,
    pub content_hash: String,
}

/// Cache-aware fetch entry point.
///
/// The extraction step is delegated to `extract_fn`: this keeps the fetcher
/// independent of the extractor module. The CLI/MCP layer wires up
/// `extractor::pipeline::extract`.
pub async fn fetch_with_cache<F>(
    db: &Db,
    client: &reqwest::Client,
    url: &Url,
    cfg: &CacheConfig,
    opts: FetchOptions,
    mut extract_fn: F,
) -> Result<CachedFetch, FetcherError>
where
    F: FnMut(&str, &Url) -> Result<ExtractResult, FetcherError>,
{
    let now = Timestamp::now().as_second();

    // Step 1: cache lookup. Fresh hits short-circuit; stale entries are kept
    // for revalidation (conditional GET) and as a fallback on network error.
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
            Some(p) => Some(p),
            None => None,
        }
    };

    // Step 2: build conditional validators from any stale entry.
    let cond = match &stale {
        Some(p) => ConditionalGet {
            if_none_match: p.etag.clone(),
            if_modified_since: p.last_modified.clone(),
        },
        None => ConditionalGet::default(),
    };

    // Step 3: fetch (conditional if validators present).
    let fetched = match fetch_url_conditional(client, url, opts.ssrf_level, &cond).await {
        Ok(f) => f,
        Err(e) => {
            // Network failure with a stale entry available → return stale.
            if let Some(s) = stale {
                tracing::warn!(target: "rover::fetcher::cached",
                    error = %e, url = url.as_str(), "fetch failed; serving stale");
                return Ok(CachedFetch {
                    page: s,
                    cache_status: CacheStatus::Stale,
                });
            }
            return Err(e);
        }
    };

    // Step 4: 304 Not Modified — extend freshness on the stale row and serve it.
    if fetched.status == 304 {
        let stale = stale.expect("304 implies a stale entry was sent");
        let host = url.host_str().unwrap_or("");
        let decision = compute_ttl(
            now,
            host,
            fetched.cache_control.as_deref().unwrap_or(""),
            fetched.expires.as_deref(),
            cfg,
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
    let host = url.host_str().unwrap_or("");
    let decision = compute_ttl(
        now,
        host,
        fetched.cache_control.as_deref().unwrap_or(""),
        fetched.expires.as_deref(),
        cfg,
    );

    let expires_at = match decision {
        TtlDecision::Cache { expires_at } => Some(expires_at),
        TtlDecision::DoNotCache => None,
    };

    let new_hash = url_hash(fetched.canonical_url.as_str());
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
        metadata_json: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_status_eq() {
        assert_ne!(CacheStatus::Hit, CacheStatus::Stale);
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
        };
        pages::upsert(&db, page.clone()).await.unwrap();

        let cfg = CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(60),
            max_ttl: Duration::from_secs(86400),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        };
        let client = super::super::client::build_http_client("test/0.1", Duration::from_secs(5));
        let result = fetch_with_cache(
            &db,
            &client,
            &url,
            &cfg,
            FetchOptions {
                force_refresh: false,
                ssrf_level: SsrfLevel::Strict,
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

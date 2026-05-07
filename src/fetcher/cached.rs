//! Cache-aware fetch orchestrator.
//!
//! `fetch_with_cache` is the high-level entry point used by the CLI and the
//! (future) MCP `fetch` tool. It wraps the raw `fetcher::fetch::fetch_url`
//! with cache lookup, TTL-driven freshness, and write-back.
//!
//! Task 7 ships a minimal version that always does a full GET on miss/stale.
//! Task 8 adds conditional GETs (`If-None-Match` / `If-Modified-Since`) and
//! 304 Not Modified handling, plus real `Cache-Control` / `Expires` header
//! extraction (currently stubbed — TTL falls back to `cache.default_ttl`).

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

use super::FetcherError;
use super::fetch::fetch_url;
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

    // Step 1: cache lookup.
    if !opts.force_refresh {
        if let Some(p) = lookup_cached(db, url).await? {
            if let Some(exp) = p.expires_at {
                if exp > now {
                    return Ok(CachedFetch {
                        page: p,
                        cache_status: CacheStatus::Hit,
                    });
                }
                // expired: fall through to fetch below
            }
        }
    }

    // Step 2: fetch. Task 8 will add conditional GET headers from a stale
    // entry's etag/last_modified; Task 7 always does a full GET.
    let fetched = match fetch_url(client, url, opts.ssrf_level).await {
        Ok(f) => f,
        Err(e) => {
            // Network failure with a stale entry available → return stale.
            if let Some(stale) = lookup_cached(db, url).await? {
                tracing::warn!(target: "rover::fetcher::cached",
                    error = %e, url = url.as_str(), "fetch failed; serving stale");
                return Ok(CachedFetch {
                    page: stale,
                    cache_status: CacheStatus::Stale,
                });
            }
            return Err(e);
        }
    };

    if !(200..300).contains(&fetched.status) {
        return Err(FetcherError::Status {
            status: fetched.status,
            url: fetched.final_url.to_string(),
        });
    }

    // Step 3: extract.
    let extracted = extract_fn(&fetched.body, &fetched.final_url)?;

    // Step 4: TTL. Task 8 wires real Cache-Control / Expires headers from
    // FetchedPage; Task 7 falls back to default_ttl.
    let cache_control_value = String::new();
    let expires_value: Option<&str> = None;
    let host = url.host_str().unwrap_or("");
    let decision = compute_ttl(now, host, &cache_control_value, expires_value, cfg);

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

    // Step 5: store (only if cacheable).
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
    // Surface as a generic decode failure for now; M3 may want a Storage variant.
    tracing::error!(target: "rover::fetcher::cached", error = %e, "storage error");
    FetcherError::Decode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_status_eq() {
        assert_ne!(CacheStatus::Hit, CacheStatus::Stale);
    }

    #[test]
    fn sha256_hex_matches_known() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }
}

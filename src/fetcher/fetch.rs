//! End-to-end fetch: SSRF check → DNS validate → GET → charset decode.

use std::net::IpAddr;
use tokio::net::lookup_host;
use tracing::debug;
use url::Url;

use super::{
    FetcherError,
    canonical::extract_canonical_url,
    charset::{Detected, decode_to_utf8},
    dns::SSRF_LEVEL,
    ssrf::{self, SsrfLevel},
};

/// A successfully fetched page.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// URL after redirects.
    pub final_url: Url,

    /// Canonical URL — `<link rel="canonical">`, then `Link` header, else `final_url`.
    pub canonical_url: Url,

    /// HTTP status of the final response.
    pub status: u16,

    /// `Content-Type` header value, if any.
    pub content_type: Option<String>,

    /// Decoded UTF-8 body.
    pub body: String,

    /// Charset detection result, for diagnostics.
    pub charset: Detected,

    /// Raw `Link` header value, if present.
    pub link_header: Option<String>,

    /// Raw `ETag` header, if present.
    pub etag: Option<String>,

    /// Raw `Last-Modified` header, if present.
    pub last_modified: Option<String>,

    /// `Cache-Control` response header (M2).
    pub cache_control: Option<String>,

    /// `Expires` response header (M2).
    pub expires: Option<String>,

    /// `Retry-After` response header (M5). RFC 9110 allows seconds-as-int or
    /// HTTP-date — parsing is in `fetcher::retry::parse_retry_after`.
    pub retry_after: Option<String>,
}

/// Conditional GET validators for revalidating a stale cache entry.
///
/// Both fields are forwarded as request headers when set:
/// `If-None-Match` for ETag-based validation and `If-Modified-Since` for
/// time-based validation. Either, both, or neither may be supplied.
#[derive(Debug, Clone, Default)]
pub struct ConditionalGet {
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

/// Fetch `url` honoring the given SSRF level.
pub async fn fetch_url(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    project_root: Option<&std::path::Path>,
    har_recorder: Option<&std::sync::Arc<super::har::HarRecorder>>,
) -> Result<FetchedPage, FetcherError> {
    fetch_url_conditional(
        client,
        url,
        level,
        project_root,
        har_recorder,
        &ConditionalGet::default(),
    )
    .await
}

/// Fetch `url` with optional conditional-GET validators.
///
/// Behaves identically to [`fetch_url`] but attaches `If-None-Match` and/or
/// `If-Modified-Since` headers when present in `cond`. Callers should be
/// prepared for a `304 Not Modified` response: in that case the body is empty
/// and only the freshness-related headers are meaningful.
///
/// When `har_recorder` is `Some`, the round-trip is captured and pushed onto
/// the recorder's in-memory entries buffer. The recorder is responsible for
/// flushing to disk on an interval or at shutdown.
pub async fn fetch_url_conditional(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    project_root: Option<&std::path::Path>,
    har_recorder: Option<&std::sync::Arc<super::har::HarRecorder>>,
    cond: &ConditionalGet,
) -> Result<FetchedPage, FetcherError> {
    let start = std::time::Instant::now();
    ssrf::validate_url_with_project_root(url, level, project_root)?;
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(ssrf::SsrfError::NoHost))?;
    let port = url.port_or_known_default().unwrap_or(0);

    // Pre-flight resolve+validate. Cheap rejection of obviously-bad addresses
    // before we set up TLS. The dial-time enforcement below (via the
    // task-local `SSRF_LEVEL` consumed by `dns::SsrfValidatingResolver`)
    // is what actually closes the DNS-rebinding TOCTOU window, including for
    // hosts reached via redirects.
    let addrs = resolve_host(host, port).await?;
    ssrf::validate_addresses(&addrs, level)?;

    let mut req = client.get(url.clone());
    // Capture only the headers we explicitly add; reqwest's implicit headers
    // (user-agent, accept-encoding) are intentionally omitted — RequestBuilder
    // doesn't expose them and HAR users care most about what we set.
    let mut request_headers_pairs: Vec<(String, String)> = Vec::new();
    if let Some(etag) = &cond.if_none_match {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
        request_headers_pairs.push(("if-none-match".into(), etag.clone()));
    }
    if let Some(lm) = &cond.if_modified_since {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
        request_headers_pairs.push(("if-modified-since".into(), lm.clone()));
    }
    // Carry the SSRF level into the resolver so every dial (initial + each
    // redirect hop) is re-validated against the policy. Without this, a
    // malicious authoritative DNS server could return a benign address to
    // our pre-flight `resolve_host` and a private/loopback address to
    // reqwest's internal dial-time resolver.
    let response = SSRF_LEVEL.scope(level, req.send()).await?;
    let status = response.status().as_u16();
    let final_url = Url::parse(response.url().as_str())?;

    // Snapshot response headers before `.bytes()` consumes the response.
    let response_headers_pairs: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let link_header = response
        .headers()
        .get(reqwest::header::LINK)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let expires = response
        .headers()
        .get(reqwest::header::EXPIRES)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let bytes = response.bytes().await?;

    if let Some(recorder) = har_recorder {
        let ex = super::har::RecordedExchange {
            url: final_url.to_string(),
            method: "GET".to_string(),
            request_headers: request_headers_pairs,
            response_status: status,
            response_headers: response_headers_pairs,
            response_body: bytes.to_vec(),
            duration: start.elapsed(),
        };
        if let Err(e) = recorder.record(ex).await {
            tracing::warn!(target: "rover::fetcher", error = ?e, "failed to record har entry");
        }
    }

    let (body, charset) = decode_to_utf8(content_type.as_deref(), &bytes);

    if let Some(ref ct) = content_type {
        if ct.to_ascii_lowercase().contains("charset=") {
            debug!(
                target: "rover::fetcher::charset",
                http_charset = ct.as_str(),
                detected = %charset.encoding.name(),
                "charset detection complete"
            );
        }
    }

    let canonical_url = extract_canonical_url(&body, &final_url, link_header.as_deref());

    Ok(FetchedPage {
        final_url,
        canonical_url,
        status,
        content_type,
        body,
        charset,
        link_header,
        etag,
        last_modified,
        cache_control,
        expires,
        retry_after,
    })
}

async fn resolve_host(host: &str, port: u16) -> Result<Vec<IpAddr>, FetcherError> {
    let target = format!("{host}:{port}");
    let iter = lookup_host(target.as_str())
        .await
        .map_err(|e| FetcherError::Dns {
            host: host.to_string(),
            source: e,
        })?;
    Ok(iter.map(|sa| sa.ip()).collect())
}

//! End-to-end fetch: SSRF check → DNS validate → GET → charset decode.

use std::net::IpAddr;
use tokio::net::lookup_host;
use tracing::debug;
use url::Url;

use super::{
    FetcherError,
    canonical::extract_canonical_url,
    charset::{Detected, decode_to_utf8},
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
}

/// Fetch `url` honoring the given SSRF level.
pub async fn fetch_url(
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
) -> Result<FetchedPage, FetcherError> {
    ssrf::validate_url(url, level)?;
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(ssrf::SsrfError::NoHost))?;
    let port = url.port_or_known_default().unwrap_or(0);

    // Resolve and validate. Note: this is best-effort — see design §2.4 about
    // the deferred TOCTOU/DNS-rebinding hardening.
    let addrs = resolve_host(host, port).await?;
    ssrf::validate_addresses(&addrs, level)?;

    let response = client.get(url.clone()).send().await?;
    let status = response.status().as_u16();
    let final_url = Url::parse(response.url().as_str())?;

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

    let bytes = response.bytes().await?;
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

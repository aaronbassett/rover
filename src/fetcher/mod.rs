//! HTTP fetching, charset detection, SSRF enforcement.

pub mod cache_control;
pub mod cached;
pub mod canonical;
pub mod charset;
pub mod client;
pub mod fetch;
pub mod ssrf;
pub mod ttl;

pub use cached::{CacheStatus, CachedFetch, ExtractResult, FetchOptions, fetch_with_cache};
pub use fetch::FetchedPage;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("dns lookup failed for {host}: {source}")]
    Dns {
        host: String,
        source: std::io::Error,
    },

    #[error("response decoding failed")]
    Decode,

    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
}

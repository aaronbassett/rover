//! HTTP fetching, charset detection, SSRF enforcement.

pub mod cache_control;
pub mod cached;
pub mod canonical;
pub mod charset;
pub mod client;
pub mod fetch;
pub mod har;
#[cfg(feature = "headless")]
pub mod headless;
pub mod ssrf;
pub mod ttl;

pub mod concurrency;
pub mod rate_limit;
pub mod retry;
pub mod robots;

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

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("extractor error: {0}")]
    Extract(#[from] crate::extractor::ExtractorError),

    #[error("retries exhausted after {attempts} attempts; last error: {last}")]
    RetryExhausted {
        attempts: u8,
        last: Box<FetcherError>,
    },

    #[error("rate limited: server requested wait of {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("robots.txt disallows {url} for user-agent {ua}")]
    RobotsDisallowed { url: String, ua: String },

    #[error("robots.txt fetch failed for {host}: {source}")]
    RobotsFetchFailed {
        host: String,
        #[source]
        source: Box<FetcherError>,
    },

    #[error("fetch deferred to retry task {task_id}")]
    Deferred { task_id: String },
}

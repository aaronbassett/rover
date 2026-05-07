//! HTTP fetching, charset detection, SSRF enforcement.

pub mod charset;
pub mod ssrf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetcherError {
    #[error("ssrf violation: {0}")]
    Ssrf(#[from] ssrf::SsrfError),
}

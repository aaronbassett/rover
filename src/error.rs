//! Crate-wide error type.
//!
//! Per design supplement §4.4: per-module error enums via `thiserror`,
//! `anyhow` only at the binary boundary. This `Error` enum is the
//! library-facing top-level type that wraps domain-specific errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] crate::fetcher::FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] crate::extractor::ExtractorError),

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

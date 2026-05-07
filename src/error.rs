//! Crate-wide error type.
//!
//! Per design supplement §4.4: per-module error enums via `thiserror`,
//! `anyhow` only at the binary boundary. This `Error` enum is the
//! library-facing top-level type that wraps domain-specific errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    // Variants are added as their respective modules introduce error types.
    // See tasks 4 (Config), 5+ (Fetcher), 9+ (Extractor).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

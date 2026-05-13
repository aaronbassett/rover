//! Tokenizer-module errors.

use thiserror::Error;

use crate::tokenizer::registry::Tokenizer;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("unknown tokenizer family: {0}")]
    UnknownFamily(String),

    // Constructed by the downloader landing in Task 3.
    #[allow(dead_code)]
    #[error("could not download tokenizer for {family}: {source}")]
    Download {
        family: Tokenizer,
        #[source]
        source: hf_hub::api::sync::ApiError,
    },

    #[error("tokenizer file for {family} is corrupt: {source}")]
    Parse {
        family: Tokenizer,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    // Constructed by the loader landing in Task 3.
    #[allow(dead_code)]
    #[error("io error reading tokenizer at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    // Constructed by the counter landing in Task 3/4.
    #[allow(dead_code)]
    #[error("tokenizer {0} is not loaded; call ensure_loaded() first")]
    NotLoaded(Tokenizer),
}

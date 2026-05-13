//! Internal MCP-layer errors. Translated to `RoverError` before crossing
//! the tool boundary.

use thiserror::Error;

use crate::extractor::ExtractorError;
use crate::fetcher::FetcherError;
use crate::mcp::envelope::RoverError;
use crate::storage::StorageError;
use crate::tokenizer::TokenizerError;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("tokenizer error: {0}")]
    Tokenizer(#[from] TokenizerError),

    #[error("fetcher error: {0}")]
    Fetcher(#[from] FetcherError),

    #[error("extractor error: {0}")]
    Extractor(#[from] ExtractorError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("max_tokens exceeded: {actual} > {max}")]
    MaxTokensExceeded { actual: usize, max: usize },
}

impl McpError {
    /// Translate to the stable wire envelope.
    pub fn into_rover_error(self) -> RoverError {
        match &self {
            Self::MaxTokensExceeded { actual, max } => {
                let msg = format!(
                    "extracted content is {actual} tokens; max_tokens={max}. \
                     summarize tool not yet available (M7)"
                );
                RoverError::new(RoverError::MAX_TOKENS_EXCEEDED, msg)
            }
            Self::InvalidArgs(m) => RoverError::new(RoverError::INVALID_ARGS, m.clone()),
            Self::InvalidUrl(m) => RoverError::new(RoverError::INVALID_URL, m.clone()),
            Self::Tokenizer(e) => match e {
                TokenizerError::UnknownFamily(name) => RoverError::new(
                    RoverError::INVALID_ARGS,
                    format!("unknown tokenizer family: {name}"),
                ),
                TokenizerError::Download { family, .. } => RoverError::new(
                    RoverError::TOKENIZER_UNAVAILABLE,
                    format!("could not fetch tokenizer for {family}: {e}"),
                ),
                TokenizerError::Parse { family, .. } => RoverError::new(
                    RoverError::TOKENIZER_UNAVAILABLE,
                    format!("tokenizer file for {family} is corrupt: {e}"),
                ),
                TokenizerError::Io { .. } | TokenizerError::NotLoaded(_) => {
                    RoverError::new(RoverError::TOKENIZER_UNAVAILABLE, e.to_string())
                }
            },
            Self::Fetcher(e) => {
                use crate::fetcher::FetcherError as F;
                match e {
                    F::Ssrf(_) => RoverError::new(RoverError::SSRF_DENIED, e.to_string()),
                    F::Url(_) => RoverError::new(RoverError::INVALID_URL, e.to_string()),
                    F::Storage(_) => RoverError::new(RoverError::STORAGE_ERROR, e.to_string()),
                    F::Http(_) | F::Dns { .. } | F::Decode | F::Status { .. } => {
                        RoverError::new(RoverError::FETCH_FAILED, e.to_string())
                    }
                }
            }
            Self::Extractor(e) => RoverError::new(RoverError::EXTRACT_FAILED, e.to_string()),
            Self::Storage(e) => RoverError::new(RoverError::STORAGE_ERROR, e.to_string()),
        }
    }
}

/// Convenience: log + translate. Use this at the tool boundary.
pub(crate) fn log_and_translate(err: McpError) -> RoverError {
    tracing::warn!(target: "rover::mcp", error = ?err, "tool error");
    err.into_rover_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_tokens_translation_uses_stable_code() {
        let e = McpError::MaxTokensExceeded {
            actual: 5000,
            max: 1000,
        };
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::MAX_TOKENS_EXCEEDED);
        assert!(r.message.contains("5000"));
        assert!(r.message.contains("1000"));
        assert!(r.message.contains("summarize"));
    }

    #[test]
    fn invalid_args_translation() {
        let e = McpError::InvalidArgs("bad".into());
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
        assert_eq!(r.message, "bad");
    }

    #[test]
    fn fetcher_url_routes_to_invalid_url() {
        use crate::fetcher::FetcherError;
        // url::ParseError doesn't have a no-arg constructor, so build by parsing a bad URL.
        let parse_err = url::Url::parse("not a url").unwrap_err();
        let e = McpError::Fetcher(FetcherError::Url(parse_err));
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_URL);
    }

    #[test]
    fn fetcher_storage_routes_to_storage_error() {
        use crate::fetcher::FetcherError;
        use crate::storage::StorageError;
        // Build a synthetic StorageError via rusqlite::Error (no DB connection needed).
        let rusqlite_err = rusqlite::Error::InvalidQuery;
        let storage_err: StorageError = rusqlite_err.into();
        let e = McpError::Fetcher(FetcherError::Storage(storage_err));
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::STORAGE_ERROR);
    }
}

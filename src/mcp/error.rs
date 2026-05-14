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
                    F::Extract(_) => RoverError::new(RoverError::EXTRACT_FAILED, e.to_string()),
                    F::RobotsDisallowed { .. } => {
                        RoverError::new(RoverError::ROBOTS_DISALLOWED, e.to_string())
                    }
                    F::RobotsFetchFailed { .. } => {
                        RoverError::new(RoverError::ROBOTS_FETCH_FAILED, e.to_string())
                    }
                    F::RetryExhausted { .. } => {
                        RoverError::new(RoverError::RETRY_EXHAUSTED, e.to_string())
                    }
                    F::RateLimited { .. } => {
                        RoverError::new(RoverError::RATE_LIMITED, e.to_string())
                    }
                    F::Deferred { task_id } => {
                        RoverError::new(RoverError::DEFERRED, format!("deferred to task {task_id}"))
                    }
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

    #[test]
    fn extractor_output_error_routes_to_extract_failed() {
        use crate::extractor::ExtractorError;
        let e = McpError::Extractor(ExtractorError::Output {
            path: "/no/such".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::EXTRACT_FAILED);
        assert!(r.message.contains("/no/such"));
    }

    #[test]
    fn fetcher_extract_routes_to_extract_failed() {
        use crate::extractor::ExtractorError;
        use crate::fetcher::FetcherError;
        let inner = ExtractorError::Output {
            path: "/tmp/x".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        let e = McpError::Fetcher(FetcherError::Extract(inner));
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::EXTRACT_FAILED);
        assert!(r.message.contains("/tmp/x"));
    }

    #[test]
    fn fetcher_robots_disallowed_routes_to_robots_disallowed() {
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RobotsDisallowed {
            url: "https://example.com/admin".into(),
            ua: "Rover/0.1".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::ROBOTS_DISALLOWED);
        assert!(r.message.contains("example.com/admin"));
        assert!(r.message.contains("Rover/0.1"));
    }

    #[test]
    fn fetcher_robots_fetch_failed_routes_to_robots_fetch_failed() {
        let inner = crate::fetcher::FetcherError::Decode;
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RobotsFetchFailed {
            host: "example.com".into(),
            source: Box::new(inner),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::ROBOTS_FETCH_FAILED);
        assert!(r.message.contains("example.com"));
    }

    #[test]
    fn robots_fetch_failed_translation_carries_source_message() {
        use crate::fetcher::FetcherError;
        let e = McpError::Fetcher(FetcherError::RobotsFetchFailed {
            host: "example.com".to_string(),
            source: Box::new(FetcherError::Decode),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::ROBOTS_FETCH_FAILED);
        assert!(
            r.message.contains("response decoding failed"),
            "expected inner cause in {}",
            r.message,
        );
    }

    #[test]
    fn fetcher_retry_exhausted_routes_to_retry_exhausted() {
        let last = Box::new(crate::fetcher::FetcherError::Status {
            status: 503,
            url: "https://example.com/".into(),
        });
        let e =
            McpError::Fetcher(crate::fetcher::FetcherError::RetryExhausted { attempts: 4, last });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::RETRY_EXHAUSTED);
        assert!(r.message.contains("4 attempts"));
    }

    #[test]
    fn deferred_translation_uses_stable_code() {
        let e = McpError::Fetcher(crate::fetcher::FetcherError::Deferred {
            task_id: "abc".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::DEFERRED);
        assert!(r.message.contains("abc"));
    }

    #[test]
    fn fetcher_rate_limited_routes_to_rate_limited() {
        let e = McpError::Fetcher(crate::fetcher::FetcherError::RateLimited {
            retry_after_secs: 60,
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::RATE_LIMITED);
        assert!(r.message.contains("60"));
    }
}

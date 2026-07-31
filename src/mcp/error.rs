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

    #[error("max_tokens exceeded: {actual} > {max} (was_auto: {was_auto})")]
    MaxTokensExceeded {
        actual: usize,
        max: usize,
        was_auto: bool,
    },

    #[error("too many URLs ({count}, max {max})")]
    TooManyUrls { count: usize, max: usize },

    #[error("empty URL list")]
    EmptyUrlList,

    #[error("summarizer error: {0}")]
    Summarizer(#[from] crate::summarizer::SummarizerError),

    #[error(
        "`{mode}` writes files to the server's filesystem and returns absolute paths, which a \
         remote HTTP caller cannot read. Use a different mode, or mount the same volume at the \
         same path in both containers and set `[http] allow_server_paths = true`."
    )]
    ServerPathModeUnavailable { mode: &'static str },
}

impl McpError {
    /// Translate to the stable wire envelope.
    pub fn into_rover_error(self) -> RoverError {
        match &self {
            Self::MaxTokensExceeded {
                actual,
                max,
                was_auto,
            } => {
                let msg = if *was_auto {
                    format!(
                        "content is {actual} tokens; max_tokens={max}. \
                         Auto-summarization was attempted and the result still exceeded \
                         the budget. Reduce max_tokens, or request a summarize call with \
                         stricter target_tokens."
                    )
                } else {
                    format!(
                        "content is {actual} tokens; max_tokens={max}. \
                         You provided an explicit `summarize` arg and the summary still \
                         exceeded the budget. Increase max_tokens or request stricter \
                         target_tokens in the summarize call."
                    )
                };
                RoverError::new(RoverError::MAX_TOKENS_EXCEEDED, msg)
            }
            Self::InvalidArgs(m) => RoverError::new(RoverError::INVALID_ARGS, m.clone()),
            Self::InvalidUrl(m) => RoverError::new(RoverError::INVALID_URL, m.clone()),
            Self::TooManyUrls { .. } => {
                RoverError::new(RoverError::TOO_MANY_URLS, self.to_string())
            }
            Self::EmptyUrlList => RoverError::new(RoverError::EMPTY_URL_LIST, self.to_string()),
            Self::ServerPathModeUnavailable { .. } => {
                RoverError::new(RoverError::INVALID_ARGS, self.to_string())
            }
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
                    F::BotChallenge { .. } => {
                        RoverError::new(RoverError::BOT_CHALLENGE, e.to_string())
                    }
                    F::HeadlessFeatureNotCompiled => {
                        RoverError::new(RoverError::HEADLESS_FEATURE_NOT_COMPILED, e.to_string())
                    }
                    F::HeadlessRendererUnavailable => {
                        RoverError::new(RoverError::HEADLESS_RENDERER_UNAVAILABLE, e.to_string())
                    }
                    #[cfg(feature = "headless")]
                    F::Headless(he) => match he {
                        crate::fetcher::headless::HeadlessError::LaunchFailed(_) => {
                            RoverError::new(RoverError::HEADLESS_LAUNCH_FAILED, e.to_string())
                        }
                        crate::fetcher::headless::HeadlessError::Timeout { .. } => {
                            RoverError::new(RoverError::HEADLESS_RENDER_TIMEOUT, e.to_string())
                        }
                        crate::fetcher::headless::HeadlessError::PageClosed(_) => {
                            RoverError::new(RoverError::HEADLESS_PAGE_CLOSED, e.to_string())
                        }
                        _ => RoverError::new(RoverError::HEADLESS_INTERNAL_ERROR, e.to_string()),
                    },
                }
            }
            Self::Extractor(e) => {
                use crate::extractor::ExtractorError as X;
                match e {
                    X::CaptionerCall { source, .. } => vlm_error_to_rover_error(source.as_ref()),
                    _ => RoverError::new(RoverError::EXTRACT_FAILED, e.to_string()),
                }
            }
            Self::Storage(e) => RoverError::new(RoverError::STORAGE_ERROR, e.to_string()),
            Self::Summarizer(e) => {
                use crate::summarizer::SummarizerError as S;
                match e {
                    S::NoSuchBackend { name } => RoverError::new(
                        RoverError::SUMMARIZER_NO_SUCH_BACKEND,
                        format!("no such summarizer backend: {name}"),
                    ),
                    S::NoExtractiveBackendForFallback => RoverError::new(
                        RoverError::SUMMARIZER_NO_EXTRACTIVE_FOR_FALLBACK,
                        "no extractive backend configured for fallback",
                    ),
                    S::BackendUnavailable { name, reason } => RoverError::new(
                        RoverError::SUMMARIZER_BACKEND_UNAVAILABLE,
                        format!("backend {name} unavailable: {reason}"),
                    ),
                    S::RateLimited { name } => RoverError::new(
                        RoverError::SUMMARIZER_RATE_LIMITED,
                        format!("backend {name} rate limited"),
                    ),
                    S::AuthFailed { name, reason } => RoverError::new(
                        RoverError::SUMMARIZER_AUTH_FAILED,
                        format!("backend {name} auth failed: {reason}"),
                    ),
                    S::ModelError { name, reason } => RoverError::new(
                        RoverError::SUMMARIZER_MODEL_ERROR,
                        format!("backend {name} model error: {reason}"),
                    ),
                    S::InvalidRequest { name, reason } => RoverError::new(
                        RoverError::SUMMARIZER_INVALID_REQUEST,
                        format!("invalid request to backend {name}: {reason}"),
                    ),
                    // Borrowed inner errors — route through the same code
                    // each outer variant produces.
                    S::Storage(inner) => {
                        RoverError::new(RoverError::STORAGE_ERROR, inner.to_string())
                    }
                    S::Tokenizer(inner) => match inner {
                        TokenizerError::UnknownFamily(name) => RoverError::new(
                            RoverError::INVALID_ARGS,
                            format!("unknown tokenizer family: {name}"),
                        ),
                        TokenizerError::Download { family, .. } => RoverError::new(
                            RoverError::TOKENIZER_UNAVAILABLE,
                            format!("could not fetch tokenizer for {family}: {inner}"),
                        ),
                        TokenizerError::Parse { family, .. } => RoverError::new(
                            RoverError::TOKENIZER_UNAVAILABLE,
                            format!("tokenizer file for {family} is corrupt: {inner}"),
                        ),
                        TokenizerError::Io { .. } | TokenizerError::NotLoaded(_) => {
                            RoverError::new(RoverError::TOKENIZER_UNAVAILABLE, inner.to_string())
                        }
                    },
                    S::LocalFeatureNotCompiled => RoverError::new(
                        RoverError::SUMMARIZER_LOCAL_FEATURE_NOT_COMPILED,
                        e.to_string(),
                    ),
                }
            }
        }
    }
}

/// Map a [`crate::vlm::VlmError`] to the appropriate stable MCP wire code.
fn vlm_error_to_rover_error(e: &crate::vlm::VlmError) -> RoverError {
    use crate::vlm::VlmError as V;
    match e {
        V::NoSuchCaptioner { name } => RoverError::new(
            RoverError::CAPTIONER_NO_SUCH,
            format!("no such captioner: {name}"),
        ),
        V::NoCaptionersConfigured => {
            RoverError::new(RoverError::CAPTIONER_NOT_CONFIGURED, e.to_string())
        }
        V::LocalFeatureNotCompiled => RoverError::new(
            RoverError::CAPTIONER_LOCAL_FEATURE_NOT_COMPILED,
            e.to_string(),
        ),
        V::RateLimited { name } => RoverError::new(
            RoverError::CAPTIONER_RATE_LIMITED,
            format!("captioner {name} rate limited"),
        ),
        V::AuthFailed { name } => RoverError::new(
            RoverError::CAPTIONER_AUTH_FAILED,
            format!("captioner {name} auth failed"),
        ),
        V::Unavailable { name, reason } => RoverError::new(
            RoverError::CAPTIONER_BACKEND_UNAVAILABLE,
            format!("captioner {name} unavailable: {reason}"),
        ),
        V::SemaphoreClosed => {
            RoverError::new(RoverError::CAPTIONER_BACKEND_UNAVAILABLE, e.to_string())
        }
        V::ModelError { name, reason } => RoverError::new(
            RoverError::CAPTIONER_MODEL_ERROR,
            format!("captioner {name} model error: {reason}"),
        ),
        V::ModelIntegrityFailure {
            name,
            file,
            expected,
            actual,
        } => RoverError::new(
            RoverError::CAPTIONER_MODEL_ERROR,
            format!(
                "captioner {name}: model file {file} has been modified \
                 (expected {expected}, got {actual})"
            ),
        ),
        V::Storage(inner) => RoverError::new(RoverError::STORAGE_ERROR, inner.to_string()),
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
            was_auto: true,
        };
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::MAX_TOKENS_EXCEEDED);
        assert!(r.message.contains("5000"));
        assert!(r.message.contains("1000"));
        assert!(r.message.contains("summarize"));
        assert!(r.message.contains("Auto-summarization"));
    }

    #[test]
    fn max_tokens_translation_explicit_summarize_message_differs() {
        let auto = McpError::MaxTokensExceeded {
            actual: 5000,
            max: 1000,
            was_auto: true,
        }
        .into_rover_error();
        let explicit = McpError::MaxTokensExceeded {
            actual: 5000,
            max: 1000,
            was_auto: false,
        }
        .into_rover_error();
        assert_eq!(explicit.code, RoverError::MAX_TOKENS_EXCEEDED);
        assert!(explicit.message.contains("5000"));
        assert!(explicit.message.contains("1000"));
        assert!(
            explicit.message.contains("explicit `summarize` arg"),
            "expected explicit-summarize message, got: {}",
            explicit.message,
        );
        assert_ne!(
            auto.message, explicit.message,
            "auto vs explicit messages should differ",
        );
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
    fn summarizer_no_such_backend_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::NoSuchBackend {
            name: "missing".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_NO_SUCH_BACKEND);
        assert!(r.message.contains("missing"));
    }

    #[test]
    fn summarizer_rate_limited_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::RateLimited {
            name: "fast".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_RATE_LIMITED);
        assert!(r.message.contains("fast"));
    }

    #[test]
    fn summarizer_auth_failed_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::AuthFailed {
            name: "fast".into(),
            reason: "401".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_AUTH_FAILED);
        assert!(r.message.contains("fast"));
        assert!(r.message.contains("401"));
    }

    #[test]
    fn summarizer_backend_unavailable_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::BackendUnavailable {
            name: "fast".into(),
            reason: "network timeout".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_BACKEND_UNAVAILABLE);
    }

    #[test]
    fn summarizer_model_error_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::ModelError {
            name: "fast".into(),
            reason: "model not found".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_MODEL_ERROR);
    }

    #[test]
    fn summarizer_invalid_request_translates() {
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::InvalidRequest {
            name: "default".into(),
            reason: "empty content".into(),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_INVALID_REQUEST);
    }

    #[test]
    fn summarizer_no_extractive_for_fallback_translates() {
        let e = McpError::Summarizer(
            crate::summarizer::SummarizerError::NoExtractiveBackendForFallback,
        );
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::SUMMARIZER_NO_EXTRACTIVE_FOR_FALLBACK);
    }

    #[test]
    fn summarizer_storage_inner_translates_to_storage_error_family() {
        // The inlined inner-Storage arm should produce the same code constant
        // as the outer McpError::Storage arm.
        let inner = crate::storage::StorageError::Backend(tokio_rusqlite::Error::ConnectionClosed);
        let e = McpError::Summarizer(crate::summarizer::SummarizerError::Storage(inner));
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::STORAGE_ERROR);
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

    // VLM / captioner error routing tests.

    #[test]
    fn captioner_no_such_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::NoSuchCaptioner {
                name: "openai".into(),
            }),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_NO_SUCH);
        assert!(r.message.contains("openai"));
    }

    #[test]
    fn captioner_not_configured_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "default".into(),
            source: Box::new(VlmError::NoCaptionersConfigured),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_NOT_CONFIGURED);
    }

    #[test]
    fn captioner_local_feature_not_compiled_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "local".into(),
            source: Box::new(VlmError::LocalFeatureNotCompiled),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_LOCAL_FEATURE_NOT_COMPILED);
    }

    #[test]
    fn captioner_rate_limited_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::RateLimited {
                name: "openai".into(),
            }),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_RATE_LIMITED);
        assert!(r.message.contains("openai"));
    }

    #[test]
    fn captioner_auth_failed_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::AuthFailed {
                name: "openai".into(),
            }),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_AUTH_FAILED);
        assert!(r.message.contains("openai"));
    }

    #[test]
    fn captioner_unavailable_routes_to_backend_unavailable() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::Unavailable {
                name: "openai".into(),
                reason: "connection refused".into(),
            }),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_BACKEND_UNAVAILABLE);
        assert!(r.message.contains("connection refused"));
    }

    #[test]
    fn captioner_semaphore_closed_routes_to_backend_unavailable() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "local".into(),
            source: Box::new(VlmError::SemaphoreClosed),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_BACKEND_UNAVAILABLE);
    }

    #[test]
    fn captioner_model_error_routes_to_typed_code() {
        use crate::extractor::ExtractorError;
        use crate::vlm::VlmError;
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::ModelError {
                name: "openai".into(),
                reason: "model not found".into(),
            }),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::CAPTIONER_MODEL_ERROR);
        assert!(r.message.contains("model not found"));
    }

    #[test]
    fn captioner_storage_inner_routes_to_storage_error() {
        use crate::extractor::ExtractorError;
        use crate::storage::StorageError;
        use crate::vlm::VlmError;
        let inner = StorageError::Backend(tokio_rusqlite::Error::ConnectionClosed);
        let e = McpError::Extractor(ExtractorError::CaptionerCall {
            name: "openai".into(),
            source: Box::new(VlmError::Storage(inner)),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::STORAGE_ERROR);
    }

    #[test]
    fn extractor_non_captioner_errors_still_route_to_extract_failed() {
        use crate::extractor::ExtractorError;
        let e = McpError::Extractor(ExtractorError::Output {
            path: "/no/such".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::EXTRACT_FAILED);
    }
}

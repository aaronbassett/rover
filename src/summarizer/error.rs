//! Errors raised by the summarizer subsystem.

use thiserror::Error;

/// Errors a backend can raise from `compact`.
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend unavailable: {0}")]
    Unavailable(String),

    #[error("rate limited")]
    RateLimited,

    #[error("auth failed: {0}")]
    AuthFailed(String),

    #[error("model error: {0}")]
    ModelError(String),

    /// Programmer-visible misuse (e.g. empty content) — distinct from
    /// network errors so the service doesn't retry through extractive.
    #[error("invalid request: {0}")]
    Invalid(String),
}

/// Errors a `SummarizerService` raises. Wraps `BackendError` with the
/// originating backend's name so MCP responses can identify the failing
/// backend in `summarizer_fallback.from`.
#[derive(Debug, Error)]
pub enum SummarizerError {
    #[error("no such backend: {name}")]
    NoSuchBackend { name: String },

    #[error("no extractive backend configured for fallback")]
    NoExtractiveBackendForFallback,

    #[error("backend {name} unavailable: {reason}")]
    BackendUnavailable { name: String, reason: String },

    #[error("backend {name} rate limited")]
    RateLimited { name: String },

    #[error("backend {name} auth failed: {reason}")]
    AuthFailed { name: String, reason: String },

    #[error("backend {name} model error: {reason}")]
    ModelError { name: String, reason: String },

    #[error("invalid request to backend {name}: {reason}")]
    InvalidRequest { name: String, reason: String },

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("token counting error: {0}")]
    Tokenizer(#[from] crate::tokenizer::TokenizerError),
}

impl SummarizerError {
    /// Convert a `BackendError` into a `SummarizerError` carrying the
    /// originating backend's name.
    pub(crate) fn from_backend(name: &str, e: BackendError) -> Self {
        match e {
            BackendError::Unavailable(r) => SummarizerError::BackendUnavailable {
                name: name.to_string(),
                reason: r,
            },
            BackendError::Invalid(r) => SummarizerError::InvalidRequest {
                name: name.to_string(),
                reason: r,
            },
            BackendError::RateLimited => SummarizerError::RateLimited {
                name: name.to_string(),
            },
            BackendError::AuthFailed(r) => SummarizerError::AuthFailed {
                name: name.to_string(),
                reason: r,
            },
            BackendError::ModelError(r) => SummarizerError::ModelError {
                name: name.to_string(),
                reason: r,
            },
        }
    }

    /// Short, stable reason string for `summarizer_fallback.reason` metadata.
    ///
    /// Only meaningful for variants produced by [`Self::from_backend`]
    /// (i.e. the five mapped from [`BackendError`]). Storage/Tokenizer/
    /// NoSuchBackend/NoExtractiveBackendForFallback errors don't flow
    /// through the fallback path and should never reach this method.
    pub fn fallback_reason(&self) -> &'static str {
        match self {
            SummarizerError::BackendUnavailable { .. } => "backend_unavailable",
            SummarizerError::InvalidRequest { .. } => "invalid_request",
            SummarizerError::RateLimited { .. } => "rate_limited",
            SummarizerError::AuthFailed { .. } => "auth_failed",
            SummarizerError::ModelError { .. } => "model_error",
            other => {
                debug_assert!(
                    false,
                    "fallback_reason called on non-fallback-path variant: {other}",
                );
                "other"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_backend_maps_each_variant() {
        let cases = [
            (
                BackendError::Unavailable("net".into()),
                "backend_unavailable",
            ),
            (BackendError::RateLimited, "rate_limited"),
            (BackendError::AuthFailed("401".into()), "auth_failed"),
            (BackendError::ModelError("bad".into()), "model_error"),
            (BackendError::Invalid("empty".into()), "invalid_request"),
        ];
        for (be, expected_reason) in cases {
            let e = SummarizerError::from_backend("fast", be);
            assert_eq!(e.fallback_reason(), expected_reason, "for {e}");
        }
    }

    #[test]
    fn storage_error_converts_via_from() {
        // The exact StorageError variant doesn't matter — just that the
        // From impl is wired up.
        let storage_err =
            crate::storage::StorageError::Backend(tokio_rusqlite::Error::ConnectionClosed);
        let e: SummarizerError = storage_err.into();
        assert!(matches!(e, SummarizerError::Storage(_)));
    }

    #[test]
    fn tokenizer_error_converts_via_from() {
        let tok_err = crate::tokenizer::TokenizerError::UnknownFamily("test".to_string());
        let e: SummarizerError = tok_err.into();
        assert!(matches!(e, SummarizerError::Tokenizer(_)));
    }
}

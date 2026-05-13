//! Wire-side envelope types returned to MCP clients.
//!
//! These are the JSON shapes Claude Code (or any other MCP client) sees.
//! The `code` strings on [`RoverError`] are stable from M3 onward and will
//! be documented in `docs/mcp-tools.md` (M8).

use serde::{Deserialize, Serialize};

/// Status of a fetch response relative to the cache. Mirrors the three
/// variants of [`crate::fetcher::cached::CacheStatus`]; M3 does not
/// distinguish 304-revalidated from a fresh hit (M2 treats a 304 as a
/// regular `Hit` after refreshing `expires_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStatus {
    Hit,
    Miss,
    Stale,
}

impl From<crate::fetcher::cached::CacheStatus> for CacheStatus {
    fn from(v: crate::fetcher::cached::CacheStatus) -> Self {
        use crate::fetcher::cached::CacheStatus as C;
        match v {
            C::Hit => CacheStatus::Hit,
            C::Miss => CacheStatus::Miss,
            C::Stale => CacheStatus::Stale,
        }
    }
}

/// Where the token count came from on a `count_tokens` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountSource {
    Text,
    Url,
}

/// Successful `fetch` response (full content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    pub markdown: String,
    pub frontmatter: String,
    pub cache_status: CacheStatus,
}

/// `count_tokens` or `fetch{count_only:true}` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountResponse {
    pub tokens: usize,
    pub tokenizer: String,
    pub source: CountSource,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_status: Option<CacheStatus>,
}

/// Stable error envelope returned over MCP. `code` is from the fixed set
/// documented in the M3 design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoverError {
    pub code: &'static str,
    pub message: String,
}

impl RoverError {
    pub const MAX_TOKENS_EXCEEDED: &'static str = "max_tokens_exceeded";
    pub const INVALID_ARGS: &'static str = "invalid_args";
    pub const INVALID_URL: &'static str = "invalid_url";
    pub const SSRF_DENIED: &'static str = "ssrf_denied";
    pub const FETCH_FAILED: &'static str = "fetch_failed";
    pub const EXTRACT_FAILED: &'static str = "extract_failed";
    pub const STORAGE_ERROR: &'static str = "storage_error";
    pub const TOKENIZER_UNAVAILABLE: &'static str = "tokenizer_unavailable";

    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_response_serialises_snake_case_cache_status() {
        let v = FetchResponse {
            markdown: "x".into(),
            frontmatter: "f".into(),
            cache_status: CacheStatus::Hit,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"cache_status\":\"hit\""), "got: {s}");
    }

    #[test]
    fn count_response_omits_optional_fields() {
        let v = CountResponse {
            tokens: 7,
            tokenizer: "o200k".into(),
            source: CountSource::Text,
            url: None,
            content_hash: None,
            fetched_at: None,
            cache_status: None,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("url"));
        assert!(!s.contains("content_hash"));
        assert!(!s.contains("cache_status"));
    }

    #[test]
    fn rover_error_codes_are_stable_constants() {
        let codes: &[&'static str] = &[
            RoverError::MAX_TOKENS_EXCEEDED,
            RoverError::INVALID_ARGS,
            RoverError::FETCH_FAILED,
            RoverError::SSRF_DENIED,
            RoverError::EXTRACT_FAILED,
            RoverError::STORAGE_ERROR,
            RoverError::TOKENIZER_UNAVAILABLE,
            RoverError::INVALID_URL,
        ];
        for (i, a) in codes.iter().enumerate() {
            for (j, b) in codes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "duplicate code: {a}");
                }
            }
        }
    }
}

//! Wire-side envelope types returned to MCP clients.
//!
//! These are the JSON shapes Claude Code (or any other MCP client) sees.
//! The `code` strings on [`RoverError`] are stable from M3 onward and will
//! be documented in `docs/mcp-tools.md` (M8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Status of a fetch response relative to the cache. Mirrors the three
/// variants of [`crate::fetcher::cached::CacheStatus`]; M3 does not
/// distinguish 304-revalidated from a fresh hit (M2 treats a 304 as a
/// regular `Hit` after refreshing `expires_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
            C::Stale { .. } => CacheStatus::Stale,
        }
    }
}

/// Where the token count came from on a `count_tokens` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountSource {
    Text,
    Url,
}

/// Successful `fetch` response (full content).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchResponse {
    /// The full agent-facing document: a trusted preamble followed by the
    /// nonce-wrapped frontmatter+body (see the prompt-injection guard). When
    /// the guard's `wrap` method is allowlisted for the URL this is the
    /// unwrapped frontmatter+body instead.
    pub content: String,
    pub cache_status: CacheStatus,

    /// Present when `cache_status == "stale"` and a background revalidate
    /// task was successfully queued. Agents can monitor or ignore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revalidation: Option<StaleRevalidation>,

    /// `true` when the agent supplied an explicit `summarize` arg and the
    /// returned `markdown` is the summary, not the extracted body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarized: Option<bool>,

    /// `true` when the extracted body exceeded `max_tokens` and Rover
    /// auto-summarized to bring it within budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_summarized: Option<bool>,

    /// Populated when whichever summarize path ran (`summarize` arg or the
    /// auto path on `max_tokens`) fell back to an extractive backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer_fallback: Option<SummarizerFallbackInfo>,
}

/// Single-count `count_tokens` or `fetch{count_only:true}` response.
///
/// This is the historical M2/M3 shape: one tokenization result over either
/// inline text or a fetched URL's extracted markdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CountSingleResponse {
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

/// Four token-count estimates returned in `mode = "estimates"`.
///
/// `raw_html` is `None` when `[cache] store_raw_html = false` (the default)
/// or when the cached row has no `raw_html_zstd` blob.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CountEstimates {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_html: Option<usize>,
    pub extracted_md: usize,
    pub summary_short: usize,
    pub summary_medium: usize,
}

/// `count_tokens { mode: "estimates" }` response shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CountEstimatesResponse {
    pub url: String,
    pub tokenizer: String,
    pub estimates: CountEstimates,
}

/// `count_tokens` / `fetch{count_only:true}` response. Untagged so the
/// historical single-count shape (still the default) remains
/// wire-compatible; agents that opt into `mode = "estimates"` see the
/// `CountEstimatesResponse` variant instead.
///
/// `JsonSchema` is implemented manually so the generated schema is rooted
/// at `type: "object"` with a `oneOf` of the two variants — matching the
/// pattern used by `FetchOutput` in `src/mcp/tools/fetch.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CountResponse {
    Single(CountSingleResponse),
    Estimates(CountEstimatesResponse),
}

impl JsonSchema for CountResponse {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CountResponse".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::CountResponse").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let single = generator.subschema_for::<CountSingleResponse>();
        let estimates = generator.subschema_for::<CountEstimatesResponse>();
        schemars::json_schema!({
            "type": "object",
            "oneOf": [single, estimates],
        })
    }
}

/// `get_metadata` response — structured metadata only, no markdown body.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub og_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub schema_types: Vec<String>,
    pub extraction_quality: f32,
    pub url: String,
    pub content_hash: String,
    pub fetched_at: String,
    pub cache_status: CacheStatus,

    /// Guard telemetry for this response.
    pub prompt_injection: crate::guard::GuardTelemetry,

    /// Trusted warning surfaced when injection text was detected in the
    /// metadata values (the structured equivalent of an in-band notice).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_notice: Option<String>,
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
    pub const ROBOTS_DISALLOWED: &'static str = "robots_disallowed";
    pub const ROBOTS_FETCH_FAILED: &'static str = "robots_fetch_failed";
    pub const RETRY_EXHAUSTED: &'static str = "retry_exhausted";
    pub const RATE_LIMITED: &'static str = "rate_limited";
    pub const DEFERRED: &'static str = "deferred";
    pub const TOO_MANY_URLS: &'static str = "too_many_urls";
    pub const EMPTY_URL_LIST: &'static str = "empty_url_list";
    pub const SUMMARIZER_NO_SUCH_BACKEND: &'static str = "summarizer_no_such_backend";
    pub const SUMMARIZER_NO_EXTRACTIVE_FOR_FALLBACK: &'static str =
        "summarizer_no_extractive_backend_for_fallback";
    pub const SUMMARIZER_BACKEND_UNAVAILABLE: &'static str = "summarizer_backend_unavailable";
    pub const SUMMARIZER_RATE_LIMITED: &'static str = "summarizer_rate_limited";
    pub const SUMMARIZER_AUTH_FAILED: &'static str = "summarizer_auth_failed";
    pub const SUMMARIZER_MODEL_ERROR: &'static str = "summarizer_model_error";
    pub const SUMMARIZER_INVALID_REQUEST: &'static str = "summarizer_invalid_request";
    pub const SUMMARIZER_LOCAL_FEATURE_NOT_COMPILED: &'static str =
        "summarizer_local_feature_not_compiled";
    pub const HEADLESS_FEATURE_NOT_COMPILED: &'static str = "headless_feature_not_compiled";
    pub const HEADLESS_RENDERER_UNAVAILABLE: &'static str = "headless_renderer_unavailable";
    pub const HEADLESS_LAUNCH_FAILED: &'static str = "headless_launch_failed";
    pub const HEADLESS_RENDER_TIMEOUT: &'static str = "headless_render_timeout";
    pub const HEADLESS_PAGE_CLOSED: &'static str = "headless_page_closed";
    pub const HEADLESS_INTERNAL_ERROR: &'static str = "headless_internal_error";
    pub const CAPTIONER_NO_SUCH: &'static str = "captioner_no_such";
    pub const CAPTIONER_NOT_CONFIGURED: &'static str = "captioner_not_configured";
    pub const CAPTIONER_LOCAL_FEATURE_NOT_COMPILED: &'static str =
        "captioner_local_feature_not_compiled";
    pub const CAPTIONER_RATE_LIMITED: &'static str = "captioner_rate_limited";
    pub const CAPTIONER_AUTH_FAILED: &'static str = "captioner_auth_failed";
    pub const CAPTIONER_BACKEND_UNAVAILABLE: &'static str = "captioner_backend_unavailable";
    pub const CAPTIONER_MODEL_ERROR: &'static str = "captioner_model_error";
    pub const CAPTIONER_IMAGE_DECODE_FAILED: &'static str = "captioner_image_decode_failed";

    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Returned by tools that schedule a background task.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreatedResponse {
    pub task_id: String,
    pub status: String,
    pub kind: String,
    pub monitor_command: String,
    pub poll_command: String,
    pub cancel_command: String,
    pub hint: String,
}

/// Stale-served envelope on a `fetch` response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StaleRevalidation {
    pub task_id: String,
    pub monitor_command: String,
    pub poll_command: String,
    pub hint: String,
}

/// `summarize` tool response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeResponse {
    /// The agent-facing summary as a nonce-wrapped document (see the guard).
    pub content: String,
    pub metadata: SummarizeMetadata,
}

/// Wire-side metadata for a `summarize` response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeMetadata {
    pub backend: String,
    pub mode: String,
    pub style: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tokens: Option<usize>,
    pub estimated_tokens: usize,
    pub cache_status: SummaryCacheStatusWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer_fallback: Option<SummarizerFallbackInfo>,
    pub source_url: String,
    pub source_fetched_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    pub preserve: Vec<String>,
}

/// Cache-status wire enum for the summary cache (distinct from the page
/// cache's `CacheStatus` because the summary cache has no `Stale` variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryCacheStatusWire {
    Hit,
    Miss,
}

/// Carried on the response when the requested backend failed and an
/// extractive backend was used in its place.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizerFallbackInfo {
    pub from: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_response_serialises_snake_case_cache_status() {
        let v = FetchResponse {
            content: "x".into(),
            cache_status: CacheStatus::Hit,
            revalidation: None,
            summarized: None,
            auto_summarized: None,
            summarizer_fallback: None,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"cache_status\":\"hit\""), "got: {s}");
        assert!(s.contains("\"content\":\"x\""), "got: {s}");
    }

    #[test]
    fn count_response_omits_optional_fields() {
        let v = CountResponse::Single(CountSingleResponse {
            tokens: 7,
            tokenizer: "o200k".into(),
            source: CountSource::Text,
            url: None,
            content_hash: None,
            fetched_at: None,
            cache_status: None,
        });
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("url"));
        assert!(!s.contains("content_hash"));
        assert!(!s.contains("cache_status"));
    }

    #[test]
    fn count_response_estimates_serialises_as_estimates_shape() {
        let v = CountResponse::Estimates(CountEstimatesResponse {
            url: "https://example.com/p".into(),
            tokenizer: "o200k".into(),
            estimates: CountEstimates {
                raw_html: None,
                extracted_md: 123,
                summary_short: 45,
                summary_medium: 78,
            },
        });
        let s = serde_json::to_string(&v).unwrap();
        // Untagged: top-level keys are the inner struct's fields.
        assert!(s.contains("\"estimates\""), "got: {s}");
        assert!(s.contains("\"extracted_md\":123"), "got: {s}");
        // raw_html=None is omitted by skip_serializing_if.
        assert!(!s.contains("raw_html"), "got: {s}");
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
            RoverError::ROBOTS_DISALLOWED,
            RoverError::ROBOTS_FETCH_FAILED,
            RoverError::RETRY_EXHAUSTED,
            RoverError::RATE_LIMITED,
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

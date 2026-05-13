//! MCP `fetch` tool — wraps the M1/M2 pipeline behind a typed arg struct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::frontmatter::{PageMeta, render as render_frontmatter};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::envelope::{CacheStatus, CountResponse, CountSource, FetchResponse};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

/// Wire-side `fetch` tool arguments.
///
/// Live in M3: `url`, `force_refresh`, `count_only`, `tokenizer`, `max_tokens`.
/// Accept-no-op (schema-stable, body-deferred): `headless`, `tables`, `images`,
/// `metadata`, `summarize`. Their values are accepted by the schema and
/// emit one `tracing::debug` line each.
///
/// `tokenizer` is exposed as a string on the wire (rather than the
/// [`Tokenizer`] enum) so the JSON schema doesn't have to mirror the
/// enum's manual `Serialize`/`Deserialize` impls. Parsing happens inside
/// [`RoverHandler::fetch_inner`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    pub url: String,

    #[serde(default)]
    pub force_refresh: bool,

    #[serde(default)]
    pub count_only: bool,

    #[serde(default)]
    pub tokenizer: Option<String>,

    #[serde(default)]
    pub max_tokens: Option<usize>,

    // ---- accept-no-op until later milestones ----
    #[serde(default)]
    pub headless: Option<serde_json::Value>,
    #[serde(default)]
    pub tables: Option<serde_json::Value>,
    #[serde(default)]
    pub images: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub summarize: Option<serde_json::Value>,
}

/// One of the two response shapes the `fetch` tool can produce, depending
/// on `count_only`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FetchOutput {
    Full(FetchResponse),
    Count(CountResponse),
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    /// Task 11 wires this into the router; here it's a plain async method.
    pub async fn fetch_inner(&self, args: FetchArgs) -> Result<FetchOutput, McpError> {
        log_deferred_args(&args);

        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &url,
            &self.config.cache,
            FetchOptions {
                force_refresh: args.force_refresh,
                ssrf_level: SsrfLevel::Strict,
            },
            |body, base| {
                let extracted =
                    extract(body, Some(base)).map_err(|_| crate::fetcher::FetcherError::Decode)?;
                let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                })
            },
        )
        .await?;

        tokenizer::ensure_loaded(family).await?;
        let tokens = tokenizer::count(&result.page.extracted_md, family)?;

        if let Some(max) = args.max_tokens
            && tokens > max
        {
            return Err(McpError::MaxTokensExceeded {
                actual: tokens,
                max,
            });
        }

        let cache_status: CacheStatus = result.cache_status.into();

        if args.count_only {
            // `result.page.content_hash` is already prefixed (`sha256:...`)
            // by the `extract_fn` above; pass it through verbatim.
            return Ok(FetchOutput::Count(CountResponse {
                tokens,
                tokenizer: family.as_str().to_string(),
                source: CountSource::Url,
                url: Some(url.as_str().to_string()),
                content_hash: Some(result.page.content_hash.clone()),
                fetched_at: Some(
                    jiff::Timestamp::from_second(result.page.fetched_at)
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                ),
                cache_status: Some(cache_status),
            }));
        }

        let canonical = Url::parse(&result.page.canonical_url)
            .map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let frontmatter = render_frontmatter(&PageMeta {
            url: &url,
            canonical_url: &canonical,
            title: result.page.title.as_deref(),
            fetched_at: jiff::Timestamp::now(),
            body: &result.page.extracted_md,
            tokens,
            tokenizer_name: family.as_str(),
        });

        Ok(FetchOutput::Full(FetchResponse {
            markdown: result.page.extracted_md,
            frontmatter,
            cache_status,
        }))
    }
}

fn log_deferred_args(args: &FetchArgs) {
    if let Some(v) = &args.headless {
        tracing::debug!(target: "rover::mcp", arg = "headless", value = ?v, "ignored until M9");
    }
    if let Some(v) = &args.tables {
        tracing::debug!(target: "rover::mcp", arg = "tables", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.images {
        tracing::debug!(target: "rover::mcp", arg = "images", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.metadata {
        tracing::debug!(target: "rover::mcp", arg = "metadata", value = ?v, "ignored until M4");
    }
    if let Some(v) = &args.summarize {
        tracing::debug!(target: "rover::mcp", arg = "summarize", value = ?v, "ignored until M7");
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::tokenizer::Tokenizer;

    #[test]
    fn fetch_args_deserialize_minimal() {
        let v: FetchArgs = serde_json::from_str(r#"{"url":"https://example.com"}"#).unwrap();
        assert_eq!(v.url, "https://example.com");
        assert!(!v.force_refresh);
        assert!(!v.count_only);
        assert!(v.tokenizer.is_none());
        assert!(v.max_tokens.is_none());
    }

    #[test]
    fn fetch_args_accept_deferred_keys_as_no_op() {
        let v: FetchArgs = serde_json::from_str(
            r#"{
                "url":"https://example.com",
                "headless":"auto",
                "tables":{"mode":"Sample"},
                "images":{"mode":"Drop"},
                "metadata":{"preset":"default"},
                "summarize":{"target_tokens":500}
            }"#,
        )
        .unwrap();
        assert!(v.headless.is_some());
        assert!(v.tables.is_some());
        assert!(v.summarize.is_some());
    }

    #[test]
    fn fetch_args_reject_unknown_fields() {
        let r: Result<FetchArgs, _> =
            serde_json::from_str(r#"{"url":"https://example.com","bogus":1}"#);
        assert!(r.is_err());
    }

    #[test]
    fn fetch_args_parse_tokenizer_string() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://example.com","tokenizer":"claude"}"#).unwrap();
        assert_eq!(v.tokenizer.as_deref(), Some("claude"));
        // And the string parses to the enum variant we expect.
        let t = Tokenizer::from_str(v.tokenizer.as_deref().unwrap()).unwrap();
        assert_eq!(t, Tokenizer::Claude);
    }

    #[test]
    fn fetch_args_schema_contains_all_documented_fields() {
        let schema = schemars::schema_for!(FetchArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for field in [
            "url",
            "force_refresh",
            "count_only",
            "tokenizer",
            "max_tokens",
            "headless",
            "tables",
            "images",
            "metadata",
            "summarize",
        ] {
            assert!(json.contains(field), "schema missing field: {field}");
        }
    }
}

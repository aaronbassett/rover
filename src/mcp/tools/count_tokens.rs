//! MCP `count_tokens` tool.
//!
//! Two input modes: `text` (in-process tokenization only) or `url` (shares
//! the cached fetch pipeline with the `fetch` tool). Exactly one of the
//! two must be provided. The body lives on [`RoverHandler`] as
//! [`RoverHandler::count_tokens_inner`]; Task 11 wires it into the
//! `#[tool_router]` surface.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{CacheStatus, CountResponse, CountSource};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

/// Wire-side `count_tokens` arguments.
///
/// `tokenizer` is exposed as a string (Option 2, matching `FetchArgs`) so
/// the JSON schema doesn't have to mirror the [`crate::tokenizer::Tokenizer`]
/// enum's manual serde impls. Parsing happens via
/// [`crate::mcp::handler::resolve_tokenizer`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountTokensArgs {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tokenizer: Option<String>,
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    pub async fn count_tokens_inner(
        &self,
        args: CountTokensArgs,
    ) -> Result<CountResponse, McpError> {
        match (args.text.as_deref(), args.url.as_deref()) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(McpError::InvalidArgs(
                    "count_tokens requires exactly one of text or url".into(),
                ));
            }
            _ => {}
        }

        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;
        tokenizer::ensure_loaded(family).await?;

        if let Some(text) = args.text {
            let tokens = tokenizer::count(&text, family)?;
            return Ok(CountResponse {
                tokens,
                tokenizer: family.as_str().to_string(),
                source: CountSource::Text,
                url: None,
                content_hash: None,
                fetched_at: None,
                cache_status: None,
            });
        }

        // URL mode: share the cached fetch + extract pipeline.
        let url_str = args.url.expect("validated above");
        let url = Url::parse(&url_str).map_err(|e| McpError::InvalidUrl(e.to_string()))?;

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &self.pacer,
            &self.config.rate_limit,
            &self.config.robots,
            &url,
            &self.config.cache,
            FetchOptions {
                force_refresh: false,
                ssrf_level: self.ssrf_level,
                ignore_robots: false,
                user_agent: self.config.fetch.user_agent.clone(),
            },
            |body, base| {
                let extracted =
                    extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
                let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                    metadata: extracted.metadata,
                })
            },
        )
        .await?;

        let tokens = tokenizer::count(&result.page.extracted_md, family)?;
        let cache_status: CacheStatus = result.cache_status.into();

        Ok(CountResponse {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::envelope::RoverError;

    /// Build a handler whose db/client suffice for the synchronous
    /// validation paths. URL-mode happy-path coverage lives in
    /// `tests/mcp_smoke.rs` (Task 13). The unit tests below all error
    /// before `ensure_loaded` runs, so the global tokenizer registry is
    /// never touched and no network I/O happens.
    async fn fake_handler() -> (RoverHandler, tempfile::TempDir) {
        let cfg = std::sync::Arc::new(crate::config::Config::default());
        let client = reqwest::Client::new();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = crate::storage::Db::open(&path).await.unwrap();
        let pacer = std::sync::Arc::new(crate::fetcher::concurrency::Pacer::new(&cfg.rate_limit));
        (
            RoverHandler::new(
                db,
                cfg,
                client,
                crate::fetcher::ssrf::SsrfLevel::Strict,
                pacer,
            ),
            tmp,
        )
    }

    #[tokio::test]
    async fn rejects_both_text_and_url() {
        let (h, _tmp) = fake_handler().await;
        let err = h
            .count_tokens_inner(CountTokensArgs {
                text: Some("hi".into()),
                url: Some("https://example.com".into()),
                tokenizer: None,
            })
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[tokio::test]
    async fn rejects_neither() {
        let (h, _tmp) = fake_handler().await;
        let err = h
            .count_tokens_inner(CountTokensArgs::default())
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[tokio::test]
    async fn rejects_unknown_tokenizer() {
        let (h, _tmp) = fake_handler().await;
        let err = h
            .count_tokens_inner(CountTokensArgs {
                text: Some("hi".into()),
                url: None,
                tokenizer: Some("gpt-5".into()),
            })
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[test]
    fn schema_contains_all_fields() {
        let schema = schemars::schema_for!(CountTokensArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in ["text", "url", "tokenizer"] {
            assert!(json.contains(f), "missing {f}");
        }
    }
}

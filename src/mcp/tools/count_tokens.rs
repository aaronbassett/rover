//! MCP `count_tokens` tool.
//!
//! Two input modes: `text` (in-process tokenization only) or `url` (shares
//! the cached fetch pipeline with the `fetch` tool). Exactly one of the
//! two must be provided. The body lives on [`RoverHandler`] as
//! [`RoverHandler::count_tokens_inner`]; Task 11 wires it into the
//! `#[tool_router]` surface.
//!
//! Task 12b adds an alternate `mode = "estimates"` shape (URL-only) that
//! returns four token counts in one round-trip: `raw_html` (when stored),
//! `extracted_md`, and two extractive summary estimates at ~250 and ~750
//! target tokens.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{
    CacheStatus, CountEstimates, CountEstimatesResponse, CountResponse, CountSingleResponse,
    CountSource,
};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::storage::pages;
use crate::summarizer::backend::{CompactMode, CompactOpts, Style};
use crate::summarizer::error::SummarizerError;
use crate::tokenizer;

/// `mode` arg for `count_tokens`. `Single` is the historical M2/M3 shape
/// (one token count); `Estimates` is the M7 four-count shape that requires
/// a URL and uses the extractive backend to estimate summary sizes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountTokensMode {
    #[default]
    Single,
    Estimates,
}

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
    #[serde(default)]
    pub mode: CountTokensMode,
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    pub async fn count_tokens_inner(
        &self,
        args: CountTokensArgs,
    ) -> Result<CountResponse, McpError> {
        match args.mode {
            CountTokensMode::Single => self.count_tokens_single(args).await,
            CountTokensMode::Estimates => self.count_tokens_estimates(args).await,
        }
    }

    async fn count_tokens_single(&self, args: CountTokensArgs) -> Result<CountResponse, McpError> {
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
            return Ok(CountResponse::Single(CountSingleResponse {
                tokens,
                tokenizer: family.as_str().to_string(),
                source: CountSource::Text,
                url: None,
                content_hash: None,
                fetched_at: None,
                cache_status: None,
            }));
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
                ssrf_project_root: self.ssrf_project_root.clone(),
                har_recorder: self.har_recorder.clone(),
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

        Ok(CountResponse::Single(CountSingleResponse {
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
        }))
    }

    /// `mode = "estimates"` arm. Requires `url`; tokenizes the extracted
    /// markdown, the (optional) cached raw HTML, and two extractive-summary
    /// estimates at ~250 and ~750 target tokens.
    async fn count_tokens_estimates(
        &self,
        args: CountTokensArgs,
    ) -> Result<CountResponse, McpError> {
        if args.text.is_some() {
            return Err(McpError::InvalidArgs(
                "count_tokens mode=\"estimates\" does not accept text; provide url".into(),
            ));
        }
        let url_str = args.url.ok_or_else(|| {
            McpError::InvalidArgs("count_tokens mode=\"estimates\" requires url".into())
        })?;
        let url = Url::parse(&url_str).map_err(|e| McpError::InvalidUrl(e.to_string()))?;

        // Resolve the extractive backend strictly: the spec says estimates
        // ALWAYS run on extractive (never cloud), so if no extractive fallback
        // is configured we surface the dedicated error code.
        let extractive_name = self
            .summarizer
            .registry()
            .extractive_fallback_name()
            .ok_or(SummarizerError::NoExtractiveBackendForFallback)?
            .to_string();

        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;
        tokenizer::ensure_loaded(family).await?;

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
                ssrf_project_root: self.ssrf_project_root.clone(),
                har_recorder: self.har_recorder.clone(),
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

        let extracted_md_tokens = tokenizer::count(&result.page.extracted_md, family)?;

        // Optional raw-html token count: only present when the row carries a
        // populated `raw_html_zstd` blob AND it decodes cleanly. Any decode
        // error degrades silently to `None`.
        let url_hash = pages::url_hash(url.as_str());
        let raw_html_tokens: Option<usize> = match pages::raw_html_bytes(&self.db, &url_hash).await
        {
            Ok(Some(blob)) => zstd::stream::decode_all(blob.as_slice())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|s| tokenizer::count(&s, family).ok()),
            Ok(None) => None,
            Err(_) => None,
        };

        // Two summary-token estimates via the extractive backend, both keyed
        // on the same content_hash. Run sequentially: the extractive backend
        // is CPU-only and bounded; parallelism gives no real win here.
        let content_hash = &result.page.content_hash;
        let extracted_md = &result.page.extracted_md;
        let short_opts = CompactOpts {
            mode: CompactMode::Extractive,
            style: Style::Bullet,
            target_tokens: Some(250),
            focus: None,
            preserve: vec![],
            backend_name: extractive_name.clone(),
        };
        let medium_opts = CompactOpts {
            mode: CompactMode::Extractive,
            style: Style::Bullet,
            target_tokens: Some(750),
            focus: None,
            preserve: vec![],
            backend_name: extractive_name,
        };
        let short = self
            .summarizer
            .compact(content_hash, extracted_md, &short_opts)
            .await?;
        let medium = self
            .summarizer
            .compact(content_hash, extracted_md, &medium_opts)
            .await?;
        let summary_short_tokens = tokenizer::count(&short.summary_md, family)?;
        let summary_medium_tokens = tokenizer::count(&medium.summary_md, family)?;

        Ok(CountResponse::Estimates(CountEstimatesResponse {
            url: url.as_str().to_string(),
            tokenizer: family.as_str().to_string(),
            estimates: CountEstimates {
                raw_html: raw_html_tokens,
                extracted_md: extracted_md_tokens,
                summary_short: summary_short_tokens,
                summary_medium: summary_medium_tokens,
            },
        }))
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
        let summarizer = {
            let mut map: std::collections::HashMap<
                String,
                std::sync::Arc<dyn crate::summarizer::backend::SummarizerBackend>,
            > = Default::default();
            map.insert(
                "default".into(),
                std::sync::Arc::new(crate::summarizer::extractive::ExtractiveBackend::new(
                    "default",
                    crate::tokenizer::Tokenizer::O200k,
                )),
            );
            let reg = std::sync::Arc::new(
                crate::summarizer::registry::SummarizerRegistry::__test_construct(
                    map,
                    "default".into(),
                    Some("default".into()),
                ),
            );
            std::sync::Arc::new(crate::summarizer::SummarizerService::new(
                db.clone(),
                reg,
                true,
            ))
        };
        (
            RoverHandler::new(
                db,
                cfg,
                client,
                crate::fetcher::ssrf::SsrfLevel::Strict,
                None,
                None,
                pacer,
                summarizer,
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
                mode: CountTokensMode::Single,
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
                mode: CountTokensMode::Single,
            })
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[tokio::test]
    async fn estimates_mode_rejects_text_arg() {
        let (h, _tmp) = fake_handler().await;
        let err = h
            .count_tokens_inner(CountTokensArgs {
                text: Some("hi".into()),
                url: None,
                tokenizer: None,
                mode: CountTokensMode::Estimates,
            })
            .await
            .unwrap_err();
        let r = err.into_rover_error();
        assert_eq!(r.code, RoverError::INVALID_ARGS);
    }

    #[tokio::test]
    async fn estimates_mode_requires_url() {
        let (h, _tmp) = fake_handler().await;
        let err = h
            .count_tokens_inner(CountTokensArgs {
                text: None,
                url: None,
                tokenizer: None,
                mode: CountTokensMode::Estimates,
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
        for f in ["text", "url", "tokenizer", "mode"] {
            assert!(json.contains(f), "missing {f}");
        }
    }
}

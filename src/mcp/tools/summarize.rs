//! MCP `summarize` tool.
//!
//! Cache-or-fetch the page (M2/M3 cache hot path), dispatch through
//! [`SummarizerService`] (Task 7), then render the response envelope.
//! Synchronous; no task spawning.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{
    SummarizeMetadata, SummarizeResponse, SummarizerFallbackInfo, SummaryCacheStatusWire,
};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::summarizer::backend::{CompactMode, PreserveSection, Style};
use crate::summarizer::{DefaultsHint, SummaryCacheStatus};
use crate::tokenizer;

/// Wire-side `summarize` args. All fields except `url` are optional;
/// defaults come from `[summarization]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummarizeArgs {
    pub url: String,

    #[serde(default)]
    pub target_tokens: Option<usize>,

    #[serde(default)]
    pub mode: Option<SummarizeMode>,

    #[serde(default)]
    pub focus: Option<String>,

    #[serde(default)]
    pub preserve: Vec<SummarizePreserve>,

    #[serde(default)]
    pub style: Option<SummarizeStyle>,

    #[serde(default)]
    pub backend: Option<String>,

    #[serde(default)]
    pub tokenizer: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeMode {
    Extractive,
    Abstractive,
    Headlines,
}

impl From<SummarizeMode> for CompactMode {
    fn from(v: SummarizeMode) -> Self {
        match v {
            SummarizeMode::Extractive => CompactMode::Extractive,
            SummarizeMode::Abstractive => CompactMode::Abstractive,
            SummarizeMode::Headlines => CompactMode::Headlines,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeStyle {
    Bullet,
    Prose,
    Executive,
}

impl From<SummarizeStyle> for Style {
    fn from(v: SummarizeStyle) -> Self {
        match v {
            SummarizeStyle::Bullet => Style::Bullet,
            SummarizeStyle::Prose => Style::Prose,
            SummarizeStyle::Executive => Style::Executive,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizePreserve {
    Code,
    Tables,
    Quotes,
    Lists,
}

impl From<SummarizePreserve> for PreserveSection {
    fn from(v: SummarizePreserve) -> Self {
        match v {
            SummarizePreserve::Code => PreserveSection::Code,
            SummarizePreserve::Tables => PreserveSection::Tables,
            SummarizePreserve::Quotes => PreserveSection::Quotes,
            SummarizePreserve::Lists => PreserveSection::Lists,
        }
    }
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    pub async fn summarize_inner(
        &self,
        args: SummarizeArgs,
    ) -> Result<SummarizeResponse, McpError> {
        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;
        tokenizer::ensure_loaded(family).await?;

        // Cache-or-fetch the page.
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

        let defaults = DefaultsHint::from_config(&self.config.summarization);
        let opts = self.summarizer.resolve_defaults(
            args.mode.map(Into::into),
            args.style.map(Into::into),
            args.target_tokens,
            args.focus,
            args.preserve.into_iter().map(Into::into).collect(),
            args.backend,
            &defaults,
        );

        let summary = self
            .summarizer
            .compact(&result.page.content_hash, &result.page.extracted_md, &opts)
            .await?;

        let estimated_tokens = tokenizer::count(&summary.summary_md, family)?;

        Ok(SummarizeResponse {
            summary_md: summary.summary_md,
            metadata: SummarizeMetadata {
                backend: summary.effective_backend,
                mode: opts.mode.as_str().to_string(),
                style: opts.style.as_str().to_string(),
                target_tokens: opts.target_tokens,
                estimated_tokens,
                cache_status: match summary.cache_status {
                    SummaryCacheStatus::Hit => SummaryCacheStatusWire::Hit,
                    SummaryCacheStatus::Miss => SummaryCacheStatusWire::Miss,
                },
                summarizer_fallback: summary.fallback.map(|f| SummarizerFallbackInfo {
                    from: f.from,
                    reason: f.reason.to_string(),
                }),
                source_url: url.as_str().to_string(),
                source_fetched_at: jiff::Timestamp::from_second(result.page.fetched_at)
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                focus: opts.focus,
                preserve: opts
                    .preserve
                    .iter()
                    .map(|p| p.as_str().to_string())
                    .collect(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trips_required_fields() {
        let schema = schemars::schema_for!(SummarizeArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in [
            "url",
            "target_tokens",
            "mode",
            "focus",
            "preserve",
            "style",
            "backend",
        ] {
            assert!(json.contains(f), "missing {f}");
        }
    }

    #[test]
    fn enum_mappings_round_trip() {
        assert_eq!(
            CompactMode::from(SummarizeMode::Headlines),
            CompactMode::Headlines,
        );
        assert_eq!(Style::from(SummarizeStyle::Bullet), Style::Bullet);
        assert_eq!(
            PreserveSection::from(SummarizePreserve::Tables),
            PreserveSection::Tables,
        );
    }

    #[test]
    fn rejects_unknown_field() {
        let r: Result<SummarizeArgs, _> = serde_json::from_str(r#"{"url":"https://x/","bogus":1}"#);
        assert!(r.is_err());
    }
}

//! MCP `get_metadata` tool — fetch metadata only (no markdown body).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::MetadataResponse;
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMetadataArgs {
    pub url: String,
    #[serde(default)]
    pub force_refresh: bool,
    #[serde(default)]
    pub tokenizer: Option<String>,
}

impl RoverHandler {
    pub async fn get_metadata_inner(
        &self,
        args: GetMetadataArgs,
    ) -> Result<MetadataResponse, McpError> {
        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
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
                force_refresh: args.force_refresh,
                ssrf_level: self.ssrf_level,
                ssrf_project_root: self.ssrf_project_root.clone(),
                ignore_robots: false,
                user_agent: self.config.fetch.user_agent.clone(),
            },
            |body, base| {
                let extracted = crate::extractor::pipeline::extract(body, Some(base))
                    .map_err(crate::fetcher::FetcherError::Extract)?;
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

        let metadata: crate::extractor::ExtractedMetadata = result
            .page
            .metadata_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let quality = crate::extractor::quality::score(
            &result.page.extracted_md,
            result.page.extracted_md.chars().count().max(1),
            !metadata.is_empty(),
            result.page.title.is_some(),
        );

        Ok(MetadataResponse {
            title: metadata.title.clone(),
            description: metadata.description.clone(),
            author: metadata.author.clone(),
            published: metadata.published.clone(),
            modified: metadata.modified.clone(),
            image: metadata.image.clone(),
            og_type: metadata.og_type.clone(),
            canonical: metadata.canonical.clone(),
            language: metadata.language.clone(),
            schema_types: metadata.schema_types.clone(),
            extraction_quality: quality,
            url: url.as_str().to_string(),
            content_hash: result.page.content_hash.clone(),
            fetched_at: jiff::Timestamp::from_second(result.page.fetched_at)
                .map(|t| t.to_string())
                .unwrap_or_default(),
            cache_status: result.cache_status.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_contains_required_fields() {
        let schema = schemars::schema_for!(GetMetadataArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in ["url", "force_refresh", "tokenizer"] {
            assert!(json.contains(f), "missing {f}");
        }
    }

    #[test]
    fn rejects_unknown_field() {
        let r: Result<GetMetadataArgs, _> =
            serde_json::from_str(r#"{"url":"https://x/","bogus":1}"#);
        assert!(r.is_err());
    }
}

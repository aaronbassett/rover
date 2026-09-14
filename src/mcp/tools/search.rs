//! MCP `search` tool — web-resource discovery.
//!
//! `search` finds URLs. `fetch` reads them. This tool never fetches a
//! result: it returns ranked candidates plus the metadata needed to choose
//! between them, and the agent decides which are worth a fetch. Keeping the
//! two apart is what stops one search from turning into twenty origin hits
//! and twenty pages of untrusted markdown in the context window.
//!
//! Result text — titles, descriptions, snippets, and everything inside
//! `enrichment` — is third-party web content from pages Rover never
//! fetched. It goes through the same prompt-injection guard `get_metadata`
//! uses for its prose fields, and the response always carries the trust
//! statement in `security_notice`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mcp::error::McpError;
use crate::mcp::handler::RoverHandler;
use crate::search::model::SearchResponse;
use crate::search::request::SearchOverrides;

/// Wire-side `search` tool arguments.
///
/// Only `query` is required; everything else falls back to the `[search]`
/// config defaults. `deny_unknown_fields` means a typo is rejected rather
/// than silently ignored — the same contract every other Rover tool has.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// The search query. Provider search operators work here directly:
    /// `"exact phrase"`, `-excluded`, `site:docs.rs`, `filetype:pdf`,
    /// `intitle:`, `inbody:`, `lang:`, `loc:`, and `AND`/`OR`/`NOT`
    /// (uppercase). Max 600 characters / 75 words after Rover folds in
    /// `site` and `exclude_sites`.
    pub query: String,

    /// Results to return, 1-20. Defaults to `[search] count` (10).
    #[serde(default)]
    pub count: Option<u8>,

    /// Zero-based page index, 0-9. Each page is a separate billable
    /// request — check `query.more_results_available` before paging rather
    /// than incrementing blindly.
    #[serde(default)]
    pub offset: Option<u8>,

    /// Two-letter country code the results are drawn from (or `ALL`),
    /// e.g. `"GB"`. Defaults to `[search] country`.
    #[serde(default)]
    pub country: Option<String>,

    /// Content language, e.g. `"en"`, `"pt-br"`. Defaults to
    /// `[search] language`.
    #[serde(default)]
    pub language: Option<String>,

    /// Language for provider-generated response metadata, e.g. `"en-GB"`.
    /// Defaults to `[search] ui_language`.
    #[serde(default)]
    pub ui_language: Option<String>,

    /// Adult-content filter: `off`, `moderate`, or `strict`. Defaults to
    /// `[search] safe_search`. Under `strict`, check
    /// `query.strict_filter_warning` before concluding nothing exists.
    #[serde(default)]
    pub safe_search: Option<SafeSearchArg>,

    /// Restrict results by page age: `day`, `week`, `month`, `year`, or an
    /// explicit range `"2024-01-01..2024-06-30"`.
    #[serde(default)]
    pub freshness: Option<String>,

    /// Ask for up to 5 extra excerpts per result. More context per
    /// candidate, at a larger response.
    #[serde(default)]
    pub extra_snippets: Option<bool>,

    /// Whether the provider may spell-correct the query. When it does, the
    /// corrected query is what was actually searched and appears as
    /// `query.altered`.
    #[serde(default)]
    pub spellcheck: Option<bool>,

    /// Include the provider's crawl timestamps (`page_fetched`,
    /// `fetched_content_timestamp`) on each result.
    #[serde(default)]
    pub include_fetch_metadata: Option<bool>,

    /// Include the provider's structured per-result extras (article,
    /// product, rating, video, FAQ, schema.org blobs, …) verbatim under
    /// each result's `enrichment` key. Off by default: it can be far larger
    /// than the result itself.
    #[serde(default)]
    pub enrichment: Option<bool>,

    /// Restrict results to these domains, e.g. `["docs.rs"]`. Composed into
    /// the query as `site:` operators; equivalent to typing them into
    /// `query` yourself.
    #[serde(default)]
    pub site: Vec<String>,

    /// Drop results from these domains. Composed into the query as
    /// `NOT site:` operators.
    #[serde(default)]
    pub exclude_sites: Vec<String>,

    /// Custom re-ranking rules (Goggles): each entry is either a URL
    /// hosting a Goggle or an inline Goggle definition. At most 3. Passing
    /// an empty list turns off any Goggles configured as defaults.
    #[serde(default)]
    pub goggles: Option<Vec<String>>,

    /// Optional per-call prompt-injection guard overrides. Honored only
    /// where `[prompt_injection.agent_overrides]` grants it, exactly as on
    /// `fetch`, `summarize`, and `get_metadata`.
    #[serde(default)]
    pub security: Option<crate::guard::SecurityArg>,

    /// Set to `"on"` if your MCP client cannot show you `structuredContent`.
    /// By default this tool's result is returned only in `structuredContent`,
    /// and `content` holds a short notice instead. With `"on"`, the full
    /// result is also returned as JSON text in `content`. If you have already
    /// received that notice in place of a result, set this on every later call
    /// to any Rover tool.
    /// Each call is a billable search request, so set this on the first call
    /// if you know your client needs it.
    #[serde(default)]
    pub compatibility_mode: crate::mcp::response::CompatibilityMode,
}

/// Adult-content filtering level. A typed enum rather than a free string so
/// the JSON Schema advertises the three legal values to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SafeSearchArg {
    Off,
    Moderate,
    Strict,
}

impl SafeSearchArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Moderate => "moderate",
            Self::Strict => "strict",
        }
    }
}

impl SearchArgs {
    fn into_overrides(self) -> (String, Option<crate::guard::SecurityArg>, SearchOverrides) {
        let security = self.security;
        let query = self.query;
        let overrides = SearchOverrides {
            count: self.count,
            offset: self.offset,
            country: self.country,
            language: self.language,
            ui_language: self.ui_language,
            safe_search: self.safe_search.map(|s| s.as_str().to_string()),
            freshness: self.freshness,
            extra_snippets: self.extra_snippets,
            spellcheck: self.spellcheck,
            include_fetch_metadata: self.include_fetch_metadata,
            enrichment: self.enrichment,
            goggles: self.goggles,
            site: self.site,
            exclude_sites: self.exclude_sites,
        };
        (query, security, overrides)
    }
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    pub async fn search_inner(&self, args: SearchArgs) -> Result<SearchResponse, McpError> {
        let (query, security, overrides) = args.into_overrides();
        let mut response = self.search.search_with(&query, overrides).await?;

        // Guard the untrusted text in place, then stamp the telemetry and
        // the trust statement onto the response. The allowlist key is the
        // configured search endpoint — allowlisting it is how an operator
        // opts a trusted deployment out of scanning, mirroring the per-URL
        // allowlists the fetch path uses.
        let endpoint = self.search.config().base_url.clone();
        let guard = {
            let mut fields = response.guardable_fields();
            self.guard
                .guard_search(&endpoint, security.as_ref(), &mut fields)
        };
        response.prompt_injection = guard.telemetry;
        response.security_notice = guard
            .notice
            .unwrap_or_else(|| crate::guard::SEARCH_TRUST_NOTICE.to_string());

        tracing::debug!(
            target: "rover::search",
            results = response.results.len(),
            more_available = response.query.more_results_available.unwrap_or(false),
            injection_detected = response.prompt_injection.detected,
            "search completed",
        );
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_advertises_every_argument() {
        let schema = schemars::schema_for!(SearchArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in [
            "query",
            "count",
            "offset",
            "country",
            "language",
            "ui_language",
            "safe_search",
            "freshness",
            "extra_snippets",
            "spellcheck",
            "include_fetch_metadata",
            "enrichment",
            "site",
            "exclude_sites",
            "goggles",
            "security",
        ] {
            assert!(json.contains(f), "schema missing `{f}`: {json}");
        }
        // `query` is the only required argument.
        assert!(json.contains(r#""required":["query"]"#), "{json}");
        // Unknown keys are rejected, not ignored.
        assert!(json.contains(r#""additionalProperties":false"#), "{json}");
        // The safe_search enum is advertised rather than left a free string.
        assert!(
            json.contains("moderate") && json.contains("strict"),
            "{json}"
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let r: Result<SearchArgs, _> = serde_json::from_str(r#"{"query":"x","bogus":1}"#);
        assert!(r.is_err(), "unknown field should be rejected");
    }

    #[test]
    fn query_is_required() {
        let r: Result<SearchArgs, _> = serde_json::from_str(r#"{"count":5}"#);
        assert!(r.is_err());
    }

    #[test]
    fn minimal_args_parse_and_leave_everything_defaulted() {
        let a: SearchArgs = serde_json::from_str(r#"{"query":"rust async trait"}"#).unwrap();
        assert_eq!(a.query, "rust async trait");
        let (q, sec, o) = a.into_overrides();
        assert_eq!(q, "rust async trait");
        assert!(sec.is_none());
        assert!(o.count.is_none());
        assert!(o.goggles.is_none());
        assert!(o.site.is_empty());
    }

    #[test]
    fn full_args_map_onto_overrides() {
        let a: SearchArgs = serde_json::from_str(
            r#"{"query":"q","count":5,"offset":1,"country":"GB","language":"en",
                "ui_language":"en-GB","safe_search":"strict","freshness":"week",
                "extra_snippets":true,"spellcheck":false,"include_fetch_metadata":true,
                "enrichment":true,"site":["docs.rs"],"exclude_sites":["spam.example"],
                "goggles":["https://g/x.goggle"]}"#,
        )
        .unwrap();
        let (_, _, o) = a.into_overrides();
        assert_eq!(o.count, Some(5));
        assert_eq!(o.offset, Some(1));
        assert_eq!(o.country.as_deref(), Some("GB"));
        assert_eq!(o.safe_search.as_deref(), Some("strict"));
        assert_eq!(o.freshness.as_deref(), Some("week"));
        assert_eq!(o.extra_snippets, Some(true));
        assert_eq!(o.spellcheck, Some(false));
        assert_eq!(o.include_fetch_metadata, Some(true));
        assert_eq!(o.enrichment, Some(true));
        assert_eq!(o.site, vec!["docs.rs"]);
        assert_eq!(o.exclude_sites, vec!["spam.example"]);
        assert_eq!(o.goggles.unwrap(), vec!["https://g/x.goggle"]);
    }

    #[test]
    fn an_explicitly_empty_goggles_list_is_distinguishable_from_absent() {
        let a: SearchArgs = serde_json::from_str(r#"{"query":"q","goggles":[]}"#).unwrap();
        let (_, _, o) = a.into_overrides();
        assert_eq!(o.goggles, Some(vec![]), "must clear configured defaults");

        let a: SearchArgs = serde_json::from_str(r#"{"query":"q"}"#).unwrap();
        let (_, _, o) = a.into_overrides();
        assert_eq!(o.goggles, None, "must inherit configured defaults");
    }

    #[test]
    fn safe_search_only_accepts_the_three_levels() {
        assert!(serde_json::from_str::<SearchArgs>(r#"{"query":"q","safe_search":"off"}"#).is_ok());
        assert!(
            serde_json::from_str::<SearchArgs>(r#"{"query":"q","safe_search":"maybe"}"#).is_err()
        );
    }
}

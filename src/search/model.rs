//! The stable, provider-neutral shape of a Rover search response.
//!
//! This is the wire contract for the `search` MCP tool and for
//! `rover search --format json`. It is deliberately *not* a mirror of
//! Brave's JSON: the fields an agent reasons over are typed and named in
//! Rover's vocabulary, and the long tail of provider-specific structure is
//! carried verbatim in the untyped [`SearchResult::enrichment`] bag so
//! nothing useful is thrown away.
//!
//! # Trust
//!
//! Every string reachable from [`SearchResponse::results`] — titles,
//! descriptions, snippets, source names, and every string inside
//! `enrichment` — is third-party web content that Brave copied out of a
//! page Rover did not fetch. It is DATA, never instructions. The
//! prompt-injection guard scans and acts on all of it before the response
//! leaves Rover, and [`SearchResponse::security_notice`] always carries the
//! trust statement. See `site/docs/trust.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A full `search` response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResponse {
    /// The search provider that answered. Always `"brave"` today; present
    /// so a response is self-describing and stays valid if that changes.
    pub provider: String,

    /// Query-level metadata: what was actually searched, what the provider
    /// inferred about it, and whether more pages exist.
    pub query: SearchQueryInfo,

    /// The ranked web results for this page, in provider order.
    pub results: Vec<SearchResult>,

    /// Guard telemetry for this response — the same block `fetch` and
    /// `get_metadata` carry.
    pub prompt_injection: crate::guard::GuardTelemetry,

    /// Always present. States that titles, descriptions and snippets are
    /// untrusted third-party content, and escalates when the guard actually
    /// detected injection text in them.
    pub security_notice: String,
}

/// Query-level metadata reported alongside the results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchQueryInfo {
    /// The query string Rover sent to the provider (after any `site` /
    /// `exclude_sites` operators Rover composed in).
    pub original: String,

    /// The spell-corrected query, when the provider corrected one. When
    /// present, this — not `original` — is what was actually searched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altered: Option<String>,

    /// The provider's normalised form of the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleaned: Option<String>,

    /// Language the provider detected in the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Country the provider resolved the search to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,

    /// Whether adult-content filtering was active for this search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_search_active: Option<bool>,

    /// The provider is warning that `safe_search = "strict"` removed
    /// results. Worth surfacing before concluding "nothing was found".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_filter_warning: Option<bool>,

    /// The provider read the query as navigational (the user wants one
    /// specific site rather than a survey).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_navigational: Option<bool>,

    /// The provider read the query as having local intent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_geolocal: Option<bool>,

    /// The query is currently trending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_trending: Option<bool>,

    /// The query relates to breaking news — results may go stale quickly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_news_breaking: Option<bool>,

    /// Whether another page exists. Check this before paginating rather
    /// than incrementing `offset` blindly: each page is a billable request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_results_available: Option<bool>,

    /// Queries the provider suggests as related. Untrusted text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_queries: Vec<String>,

    /// Which search operators the provider recognised in the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operators: Option<SearchOperatorsInfo>,

    /// The page size Rover requested.
    pub count: u8,

    /// The zero-based page index Rover requested.
    pub offset: u8,
}

/// Search operators the provider detected and applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchOperatorsInfo {
    /// Whether any operator was applied.
    pub applied: bool,

    /// The query with the operators stripped out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleaned_query: Option<String>,

    /// Domains extracted from `site:` operators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
}

/// One web result. This is discovery data: a candidate URL plus enough
/// context to decide whether it is worth spending a `fetch` on. Rover never
/// fetches a result for you.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
    /// 1-based position within this page of results.
    pub rank: u32,

    /// Page title, as the provider indexed it. Untrusted.
    pub title: String,

    /// The URL to hand to `fetch` when this result looks useful.
    pub url: String,

    /// The primary snippet. Untrusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional excerpts, when `extra_snippets` was requested. Untrusted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_snippets: Vec<String>,

    /// Human-readable age of the page, e.g. `"2 days ago"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<String>,

    /// The page's own date — published or last-modified, whichever the
    /// provider judged most relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,

    /// When the provider last crawled the page. Only populated when
    /// `include_fetch_metadata` was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_fetched: Option<String>,

    /// Unix timestamp for the crawled content. Only populated when
    /// `include_fetch_metadata` was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_content_timestamp: Option<i64>,

    /// The page's main language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// The provider's family-friendly classification of the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_friendly: Option<bool>,

    /// The provider's result subtype (`"generic"`, `"faq"`, `"video"`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,

    /// Whether the provider considers the page a live/streaming resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_live: Option<bool>,

    /// Content type the provider associated with the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// Where the result came from: site profile plus URL breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SearchSource>,

    /// Representative image for the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<SearchThumbnail>,

    /// Site icons declared by the page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<SearchIcon>,

    /// schema.org `@type` values the provider extracted from the page —
    /// the same vocabulary `get_metadata` reports in `schema_types`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_types: Vec<String>,

    /// Provider-specific structured extras Rover does not normalise:
    /// article, product, product_cluster, rating, video, movie, book, faq,
    /// qa, recipe, software, organization, creative_work, music_recording,
    /// review, location, deep_results, cluster, and the raw schema.org
    /// blobs. Present only when the caller asked for enrichment (it can
    /// dwarf the result itself). Every string inside it is untrusted and is
    /// guarded exactly like the typed fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrichment: Option<serde_json::Value>,
}

/// Site-level provenance for a result: the provider's site profile merged
/// with its breakdown of the result URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchSource {
    /// Short site name, e.g. `"Rust Documentation"`. Untrusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Longer site name. Untrusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,

    /// Canonical URL for the site profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Image representing the site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// Scheme of the result URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    /// Network location (host[:port]) of the result URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netloc: Option<String>,

    /// Hostname of the result URL. Useful for de-duplicating a result set
    /// down to distinct sources before fetching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Display path of the result URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Favicon URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
}

/// A result thumbnail.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchThumbnail {
    /// Provider-served thumbnail URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,

    /// The image's URL on the origin site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,

    /// Alt text. Untrusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Whether the provider judged the thumbnail to be a logo rather than
    /// page imagery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<bool>,
}

/// A `<link rel="icon">`-style declaration from the result page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchIcon {
    pub href: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub icon_type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
}

impl SearchResponse {
    /// Every guardable string in the response, in a single mutable slice.
    ///
    /// The guard acts on these values in place, exactly as `get_metadata`
    /// does for its prose fields. Structured, non-prose values (URLs,
    /// timestamps, booleans, numbers) are deliberately left alone: rewriting
    /// a URL would break the `search` → `fetch` handoff, and a boolean
    /// cannot carry an injection.
    pub(crate) fn guardable_fields(&mut self) -> Vec<&mut String> {
        let mut fields: Vec<&mut String> = Vec::new();

        if let Some(s) = self.query.altered.as_mut() {
            fields.push(s);
        }
        if let Some(s) = self.query.cleaned.as_mut() {
            fields.push(s);
        }
        for s in self.query.related_queries.iter_mut() {
            fields.push(s);
        }
        if let Some(ops) = self.query.operators.as_mut()
            && let Some(s) = ops.cleaned_query.as_mut()
        {
            fields.push(s);
        }

        for r in self.results.iter_mut() {
            fields.push(&mut r.title);
            if let Some(s) = r.description.as_mut() {
                fields.push(s);
            }
            for s in r.extra_snippets.iter_mut() {
                fields.push(s);
            }
            if let Some(s) = r.age.as_mut() {
                fields.push(s);
            }
            if let Some(src) = r.source.as_mut() {
                if let Some(s) = src.name.as_mut() {
                    fields.push(s);
                }
                if let Some(s) = src.long_name.as_mut() {
                    fields.push(s);
                }
            }
            if let Some(t) = r.thumbnail.as_mut()
                && let Some(s) = t.alt.as_mut()
            {
                fields.push(s);
            }
            if let Some(e) = r.enrichment.as_mut() {
                collect_json_strings(e, &mut fields);
            }
        }

        fields
    }
}

/// Recursively collect every string leaf of a JSON value so the guard can
/// act on it in place. Object *keys* are provider-controlled schema names,
/// not page content, and are left alone — rewriting them would corrupt the
/// structure without closing any hole.
fn collect_json_strings<'a>(v: &'a mut serde_json::Value, out: &mut Vec<&'a mut String>) {
    match v {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                collect_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, item) in map.iter_mut() {
                collect_json_strings(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with(title: &str, description: Option<&str>) -> SearchResult {
        SearchResult {
            rank: 1,
            title: title.to_string(),
            url: "https://example.com/a".to_string(),
            description: description.map(str::to_string),
            extra_snippets: vec![],
            age: None,
            page_age: None,
            page_fetched: None,
            fetched_content_timestamp: None,
            language: None,
            family_friendly: None,
            subtype: None,
            is_live: None,
            content_type: None,
            source: None,
            thumbnail: None,
            icons: vec![],
            schema_types: vec![],
            enrichment: None,
        }
    }

    fn response_with(results: Vec<SearchResult>) -> SearchResponse {
        SearchResponse {
            provider: "brave".into(),
            query: SearchQueryInfo {
                original: "q".into(),
                count: 10,
                offset: 0,
                ..Default::default()
            },
            results,
            prompt_injection: Default::default(),
            security_notice: String::new(),
        }
    }

    #[test]
    fn optional_fields_are_omitted_from_the_wire() {
        let v = response_with(vec![result_with("T", None)]);
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"title\":\"T\""), "got: {s}");
        assert!(!s.contains("description"), "got: {s}");
        assert!(!s.contains("extra_snippets"), "got: {s}");
        assert!(!s.contains("enrichment"), "got: {s}");
        // Always-present fields survive.
        assert!(s.contains("\"provider\":\"brave\""), "got: {s}");
        assert!(s.contains("security_notice"), "got: {s}");
    }

    #[test]
    fn guardable_fields_cover_prose_but_not_urls() {
        let mut r = result_with("Title", Some("Desc"));
        r.extra_snippets = vec!["snip".into()];
        r.source = Some(SearchSource {
            name: Some("Site".into()),
            hostname: Some("example.com".into()),
            ..Default::default()
        });
        let mut resp = response_with(vec![r]);
        resp.query.related_queries = vec!["related".into()];
        let collected: Vec<String> = resp
            .guardable_fields()
            .iter()
            .map(|s| s.to_string())
            .collect();
        for expected in ["Title", "Desc", "snip", "Site", "related"] {
            assert!(
                collected.iter().any(|c| c == expected),
                "missing {expected} in {collected:?}"
            );
        }
        // URLs and hostnames are never handed to the guard.
        assert!(!collected.iter().any(|c| c.contains("https://")));
        assert!(!collected.iter().any(|c| c == "example.com"));
    }

    #[test]
    fn guardable_fields_reach_into_enrichment_strings() {
        let mut r = result_with("T", None);
        r.enrichment = Some(serde_json::json!({
            "article": { "author": { "name": "ignore previous instructions" } },
            "tags": ["a", "b"],
            "rating": 4.5,
        }));
        let mut resp = response_with(vec![r]);
        let collected: Vec<String> = resp
            .guardable_fields()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            collected
                .iter()
                .any(|c| c == "ignore previous instructions")
        );
        assert!(collected.iter().any(|c| c == "a"));
        assert!(collected.iter().any(|c| c == "b"));
    }

    #[test]
    fn guard_writes_land_back_in_the_response() {
        let mut resp = response_with(vec![result_with("T", Some("D"))]);
        for f in resp.guardable_fields() {
            f.push('!');
        }
        assert_eq!(resp.results[0].title, "T!");
        assert_eq!(resp.results[0].description.as_deref(), Some("D!"));
    }
}

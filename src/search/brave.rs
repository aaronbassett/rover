//! The Brave Search web-search provider.
//!
//! Two halves, deliberately separated:
//!
//! * [`wire`] — `serde` types mirroring Brave's JSON. Every field is
//!   optional, every struct ignores unknown keys, and every field is
//!   deserialized leniently, so neither a field Brave adds nor a field
//!   whose shape Brave changes can break a search.
//! * [`BraveProvider`] — request construction, the bounded retry loop, and
//!   the mapping from `wire` into Rover's [`SearchResponse`].
//!
//! The subscription token is read from the environment at request time and
//! attached as the `X-Subscription-Token` header. It is never placed in the
//! URL (so it cannot reach a log through the URL redactor's blind spots),
//! never stored on a struct, and never included in an error.

use std::time::Duration;

use crate::config::SearchConfig;
use crate::search::model::{
    SearchIcon, SearchOperatorsInfo, SearchQueryInfo, SearchResponse, SearchResult, SearchSource,
    SearchThumbnail,
};
use crate::search::request::SearchRequest;
use crate::search::{SearchError, rate::EndpointPacer};

/// The header Brave authenticates with.
const TOKEN_HEADER: &str = "X-Subscription-Token";

/// Result verticals Rover asks for. `web` is the discovery surface Rover's
/// contract is about; `query` is always returned regardless and is named
/// here for clarity. Deliberately narrow — news/videos/discussions/FAQ
/// verticals are a separate design question, not a v1 default that silently
/// inflates every response.
const RESULT_FILTER: &str = "query,web";

pub mod wire {
    //! `serde` mirrors of Brave's web-search JSON.
    //!
    //! Forward-compatibility rules for everything in here:
    //! * no `deny_unknown_fields` — Brave adds fields regularly;
    //! * every field `Option` or `#[serde(default)]`;
    //! * every field `deserialize_with = "lenient"`, so a field that changes
    //!   *type* upstream degrades to absent instead of failing the response;
    //! * anything Rover does not model stays a `serde_json::Value`.

    use serde::Deserialize;

    /// Deserialize a field leniently: a value whose *type* is not what Rover
    /// expects becomes `T::default()` instead of failing the whole response.
    ///
    /// `#[serde(default)]` alone only covers a *missing* key, which is the
    /// easy half of provider drift. The hard half is a field whose shape
    /// changes in place — a bare string becoming an object, a count becoming
    /// a string. Without this, one such change turns *every* search into
    /// `search_malformed_response`; with it, the drifted field costs its own
    /// value and every other result still reaches the agent.
    fn lenient<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: serde::de::DeserializeOwned + Default,
    {
        let v = serde_json::Value::deserialize(d)?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    /// Like [`lenient`], but element-wise: one unparseable result is dropped
    /// rather than emptying the whole list.
    fn lenient_vec<'de, D, T>(d: D) -> std::result::Result<Vec<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: serde::de::DeserializeOwned,
    {
        let v = serde_json::Value::deserialize(d)?;
        let serde_json::Value::Array(items) = v else {
            return Ok(Vec::new());
        };
        Ok(items
            .into_iter()
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect())
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct WebSearchApiResponse {
        #[serde(default, deserialize_with = "lenient")]
        pub query: Option<Query>,
        #[serde(default, deserialize_with = "lenient")]
        pub web: Option<Web>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Query {
        #[serde(default, deserialize_with = "lenient")]
        pub original: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub altered: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub cleaned: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub safesearch: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub show_strict_warning: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub is_navigational: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub is_geolocal: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub is_trending: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub is_news_breaking: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub more_results_available: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub country: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub language: Option<Language>,
        #[serde(default, deserialize_with = "lenient")]
        pub related_queries: Option<Vec<String>>,
        #[serde(default, deserialize_with = "lenient")]
        pub search_operators: Option<SearchOperators>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Language {
        #[serde(default, deserialize_with = "lenient")]
        pub main: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct SearchOperators {
        #[serde(default, deserialize_with = "lenient")]
        pub applied: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub cleaned_query: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub sites: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Web {
        #[serde(default, deserialize_with = "lenient_vec")]
        pub results: Vec<Result>,
        #[serde(default, deserialize_with = "lenient")]
        pub family_friendly: Option<bool>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Result {
        #[serde(default, deserialize_with = "lenient")]
        pub title: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub url: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub description: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub extra_snippets: Option<Vec<String>>,
        #[serde(default, deserialize_with = "lenient")]
        pub age: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub page_age: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub page_fetched: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub fetched_content_timestamp: Option<i64>,
        #[serde(default, deserialize_with = "lenient")]
        pub language: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub family_friendly: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub subtype: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub is_live: Option<bool>,
        #[serde(default, deserialize_with = "lenient")]
        pub content_type: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub profile: Option<Profile>,
        #[serde(default, deserialize_with = "lenient")]
        pub meta_url: Option<MetaUrl>,
        #[serde(default, deserialize_with = "lenient")]
        pub thumbnail: Option<Thumbnail>,
        #[serde(default, deserialize_with = "lenient")]
        pub icons: Option<Vec<Icon>>,
        #[serde(default, deserialize_with = "lenient")]
        pub schemas: Option<serde_json::Value>,

        /// Everything else Brave sent for this result. Populated by
        /// `#[serde(flatten)]`, so any field added upstream lands here
        /// automatically rather than being dropped.
        #[serde(flatten)]
        pub extra: serde_json::Map<String, serde_json::Value>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Profile {
        #[serde(default, deserialize_with = "lenient")]
        pub name: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub long_name: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub url: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub img: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct MetaUrl {
        #[serde(default, deserialize_with = "lenient")]
        pub scheme: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub netloc: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub hostname: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub favicon: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub path: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Thumbnail {
        #[serde(default, deserialize_with = "lenient")]
        pub src: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub original: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub alt: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub width: Option<u32>,
        #[serde(default, deserialize_with = "lenient")]
        pub height: Option<u32>,
        #[serde(default, deserialize_with = "lenient")]
        pub logo: Option<bool>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct Icon {
        #[serde(default, deserialize_with = "lenient")]
        pub href: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub sizes: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub rel: Option<String>,
        #[serde(rename = "type", default, deserialize_with = "lenient")]
        pub icon_type: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub ext: Option<String>,
    }

    /// Brave's error envelope (404 / 422 / 429 and friends).
    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct ErrorResponse {
        #[serde(default, deserialize_with = "lenient")]
        pub error: Option<ErrorBody>,
    }

    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct ErrorBody {
        #[serde(default, deserialize_with = "lenient")]
        pub code: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub detail: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        pub status: Option<i64>,
    }
}

/// Keys of `wire::Result::extra` that Rover maps into typed fields, or that
/// are pure provider bookkeeping. Everything *not* in this set is carried
/// through to `SearchResult::enrichment`.
const ENRICHMENT_SKIP: &[&str] = &[
    "type",
    "is_source_local",
    "is_source_both",
    // Deprecated alias Brave still emits alongside `location`.
    "restaurant",
];

/// The Brave provider: an HTTP client, the endpoint pacer, and the config
/// it was built from.
pub struct BraveProvider {
    client: reqwest::Client,
    cfg: SearchConfig,
    pacer: EndpointPacer,
}

impl std::fmt::Debug for BraveProvider {
    /// Hand-written so no future derive can start printing the config's
    /// neighbours; the token is never on this struct in the first place.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BraveProvider")
            .field("base_url", &self.cfg.base_url)
            .field("api_key_env", &self.cfg.api_key_env)
            .finish_non_exhaustive()
    }
}

impl BraveProvider {
    pub fn new(cfg: SearchConfig, user_agent: &str) -> Self {
        let client = crate::fetcher::client::build_http_client(user_agent, cfg.timeout());
        let pacer = EndpointPacer::new(cfg.requests_per_minute);
        Self { client, cfg, pacer }
    }

    /// Run one search. Bounded retries; the credential is resolved here so
    /// a key exported after startup is picked up without a restart.
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse, SearchError> {
        let key = self
            .cfg
            .api_key()
            .ok_or_else(|| SearchError::NotConfigured {
                env_var: self.cfg.api_key_env.clone(),
            })?;

        let url = self.build_url(req)?;
        let body = self.send_with_retries(&url, &key).await?;
        let parsed: wire::WebSearchApiResponse = serde_json::from_str(&body).map_err(|e| {
            SearchError::MalformedResponse(format!("could not parse the search response: {e}"))
        })?;
        Ok(map_response(parsed, req))
    }

    /// Build the request URL. Every parameter is a query-string value; the
    /// credential is NOT one of them.
    fn build_url(&self, req: &SearchRequest) -> Result<url::Url, SearchError> {
        let mut url = url::Url::parse(&self.cfg.base_url).map_err(|e| {
            SearchError::InvalidRequest(format!(
                "search.base_url `{}` is not a URL: {e}",
                self.cfg.base_url
            ))
        })?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("q", &req.query);
            q.append_pair("count", &req.count.to_string());
            q.append_pair("offset", &req.offset.to_string());
            q.append_pair("country", &req.country);
            q.append_pair("search_lang", &req.language);
            q.append_pair("ui_lang", &req.ui_language);
            q.append_pair("safesearch", req.safe_search.as_str());
            q.append_pair("spellcheck", bool_str(req.spellcheck));
            q.append_pair("result_filter", RESULT_FILTER);
            // Brave defaults this to `true`, which sprinkles highlight
            // markers through every snippet. An agent reads snippets as
            // prose, so Rover always asks for undecorated text.
            q.append_pair("text_decorations", "false");
            if req.extra_snippets {
                q.append_pair("extra_snippets", "true");
            }
            if req.include_fetch_metadata {
                q.append_pair("include_fetch_metadata", "true");
            }
            if let Some(f) = &req.freshness {
                q.append_pair("freshness", &f.to_wire());
            }
            for g in &req.goggles {
                q.append_pair("goggles", g);
            }
        }
        Ok(url)
    }

    /// Send the request, retrying only what is genuinely retryable.
    ///
    /// This is deliberately NOT `fetcher::retry::with_retries`. That path is
    /// built for origin fetches: it consults robots, takes a per-host pacer
    /// guard, and can *defer* a long `Retry-After` into a background task
    /// that retries later. Applied to a metered, paid API, deferral would
    /// turn one agent call into an unbounded stream of billable requests.
    /// Here the loop is short, capped by `[search] max_retries` (itself
    /// capped at 5), honours `Retry-After` clamped to
    /// `[search] retry_after_ceiling`, and never retries a 4xx other than
    /// 429 — a bad argument or a bad key will not get better by being asked
    /// again.
    async fn send_with_retries(&self, url: &url::Url, key: &str) -> Result<String, SearchError> {
        let mut attempt: u8 = 0;
        loop {
            self.pacer.acquire().await;
            let outcome = self.send_once(url, key).await;
            let err = match outcome {
                Ok(body) => return Ok(body),
                Err(e) => e,
            };

            let retry_in = match &err {
                SearchError::RateLimited { retry_after_secs } => Some(
                    retry_after_secs
                        .map(Duration::from_secs)
                        .map(|d| d.min(self.cfg.retry_after_ceiling))
                        .unwrap_or_else(|| backoff(attempt)),
                ),
                SearchError::Upstream { .. } | SearchError::Network { .. } => {
                    Some(backoff(attempt))
                }
                // Auth, subscription, quota, invalid args, malformed body
                // and timeouts are all terminal: retrying spends money for
                // the same answer.
                _ => None,
            };

            let Some(wait) = retry_in else {
                return Err(err);
            };
            if attempt >= self.cfg.max_retries {
                tracing::warn!(
                    target: "rover::search",
                    attempts = attempt + 1,
                    error = %err,
                    "search retries exhausted",
                );
                return Err(err);
            }
            tracing::debug!(
                target: "rover::search",
                attempt = attempt + 1,
                wait_ms = wait.as_millis() as u64,
                error = %err,
                "retrying search request",
            );
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    async fn send_once(&self, url: &url::Url, key: &str) -> Result<String, SearchError> {
        let resp = self
            .client
            .get(url.clone())
            .header(TOKEN_HEADER, key)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SearchError::Timeout {
                        secs: self.cfg.timeout_secs,
                    }
                } else {
                    // `e` renders the request URL, which carries the query
                    // but never the credential (that is a header).
                    SearchError::Network {
                        detail: e.to_string(),
                    }
                }
            })?;

        let status = resp.status();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(crate::fetcher::retry::parse_retry_after)
            .map(|d| d.as_secs());
        let body = resp.text().await.map_err(|e| SearchError::Network {
            detail: format!("could not read the search response body: {e}"),
        })?;

        if status.is_success() {
            return Ok(body);
        }
        Err(classify_status(status.as_u16(), retry_after, &body))
    }
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Exponential backoff with a 1s base, capped at 8s. Small on purpose: the
/// caller is an agent waiting on a tool result, and every attempt is billed.
fn backoff(attempt: u8) -> Duration {
    Duration::from_secs(1u64 << attempt.min(3))
}

/// Map an HTTP status (plus Brave's error envelope, when it sent one) onto
/// a typed [`SearchError`].
///
/// Brave's `error.code` and `error.detail` are folded into the human-facing
/// message because they say useful things ("SUBSCRIPTION_TOKEN_INVALID"),
/// but they are never Rover's contract — callers branch on the stable MCP
/// code that each variant maps to.
fn classify_status(status: u16, retry_after: Option<u64>, body: &str) -> SearchError {
    let parsed: Option<wire::ErrorBody> = serde_json::from_str::<wire::ErrorResponse>(body)
        .ok()
        .and_then(|e| e.error);
    let code = parsed.as_ref().and_then(|e| e.code.clone());
    let detail_text = parsed.as_ref().and_then(|e| e.detail.clone());
    let detail = match (&code, &detail_text) {
        (Some(c), Some(d)) => format!("HTTP {status} {c}: {d}"),
        (Some(c), None) => format!("HTTP {status} {c}"),
        (None, Some(d)) => format!("HTTP {status}: {d}"),
        (None, None) => format!("HTTP {status}"),
    };
    let code_upper = code.unwrap_or_default().to_ascii_uppercase();

    // The provider's own code wins where it is unambiguous — a 422 can
    // carry either a validation failure or an expired subscription.
    if code_upper.contains("QUOTA") {
        return SearchError::QuotaExhausted { detail };
    }
    if code_upper.contains("RATE") {
        return SearchError::RateLimited {
            retry_after_secs: retry_after,
        };
    }
    if code_upper.contains("TOKEN") || code_upper.contains("AUTH") {
        return SearchError::AuthFailed { detail };
    }
    if code_upper.contains("SUBSCRIPTION") || code_upper.contains("PLAN") {
        return SearchError::SubscriptionDenied { detail };
    }

    match status {
        401 => SearchError::AuthFailed { detail },
        402 | 403 => SearchError::SubscriptionDenied { detail },
        429 => SearchError::RateLimited {
            retry_after_secs: retry_after,
        },
        400 | 404 | 422 => SearchError::InvalidRequest(format!(
            "the search provider rejected the request ({detail})"
        )),
        s if (500..600).contains(&s) => SearchError::Upstream { detail },
        _ => SearchError::Upstream { detail },
    }
}

/// Map Brave's response onto Rover's stable model.
fn map_response(raw: wire::WebSearchApiResponse, req: &SearchRequest) -> SearchResponse {
    let q = raw.query.unwrap_or_default();
    let query = SearchQueryInfo {
        // Fall back to what Rover sent: `query.original` is nullable, and a
        // response that cannot name its own query is useless for paging.
        original: q.original.unwrap_or_else(|| req.query.clone()),
        altered: q.altered,
        cleaned: q.cleaned,
        language: q.language.and_then(|l| l.main),
        country: q.country,
        safe_search_active: q.safesearch,
        strict_filter_warning: q.show_strict_warning,
        is_navigational: q.is_navigational,
        is_geolocal: q.is_geolocal,
        is_trending: q.is_trending,
        is_news_breaking: q.is_news_breaking,
        more_results_available: q.more_results_available,
        related_queries: q.related_queries.unwrap_or_default(),
        operators: q.search_operators.map(|o| SearchOperatorsInfo {
            applied: o.applied.unwrap_or(false),
            cleaned_query: o.cleaned_query,
            sites: o.sites.unwrap_or_default(),
        }),
        count: req.count,
        offset: req.offset,
    };

    let results = raw
        .web
        .unwrap_or_default()
        .results
        .into_iter()
        .filter_map(|r| map_result(r, req.enrichment))
        .enumerate()
        .map(|(i, mut r)| {
            r.rank = i as u32 + 1;
            r
        })
        .collect();

    SearchResponse {
        provider: "brave".to_string(),
        query,
        results,
        // Filled in by the guard at the tool boundary.
        prompt_injection: Default::default(),
        security_notice: String::new(),
    }
}

/// Map one Brave result. Returns `None` for a result with no usable URL —
/// a result an agent cannot fetch is not discovery data.
fn map_result(r: wire::Result, keep_enrichment: bool) -> Option<SearchResult> {
    let url = r.url.filter(|u| !u.trim().is_empty())?;

    let source = build_source(r.profile, r.meta_url);
    let schema_types = extract_schema_types(r.schemas.as_ref());
    let enrichment = if keep_enrichment {
        build_enrichment(r.extra, r.schemas)
    } else {
        None
    };

    Some(SearchResult {
        // Overwritten by the caller once ordering is known.
        rank: 0,
        title: r.title.unwrap_or_default(),
        url,
        description: r.description.filter(|d| !d.is_empty()),
        extra_snippets: r.extra_snippets.unwrap_or_default(),
        age: r.age,
        page_age: r.page_age,
        page_fetched: r.page_fetched,
        fetched_content_timestamp: r.fetched_content_timestamp,
        language: r.language,
        family_friendly: r.family_friendly,
        subtype: r.subtype,
        is_live: r.is_live,
        content_type: r.content_type,
        source,
        thumbnail: r.thumbnail.map(|t| SearchThumbnail {
            src: t.src,
            original: t.original,
            alt: t.alt,
            width: t.width,
            height: t.height,
            logo: t.logo,
        }),
        icons: r
            .icons
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| {
                i.href.map(|href| SearchIcon {
                    href,
                    sizes: i.sizes,
                    rel: i.rel,
                    icon_type: i.icon_type,
                    ext: i.ext,
                })
            })
            .collect(),
        schema_types,
        enrichment,
    })
}

/// Merge Brave's site `profile` and URL `meta_url` into one source block.
/// Returns `None` when neither carried anything.
fn build_source(
    profile: Option<wire::Profile>,
    meta_url: Option<wire::MetaUrl>,
) -> Option<SearchSource> {
    let p = profile.unwrap_or_default();
    let m = meta_url.unwrap_or_default();
    let s = SearchSource {
        name: p.name,
        long_name: p.long_name,
        url: p.url,
        image: p.img,
        scheme: m.scheme,
        netloc: m.netloc,
        hostname: m.hostname,
        path: m.path,
        favicon: m.favicon,
    };
    let empty = s.name.is_none()
        && s.long_name.is_none()
        && s.url.is_none()
        && s.image.is_none()
        && s.scheme.is_none()
        && s.netloc.is_none()
        && s.hostname.is_none()
        && s.path.is_none()
        && s.favicon.is_none();
    if empty { None } else { Some(s) }
}

/// Pull `@type` values out of Brave's schema.org blobs, mirroring the
/// `schema_types` vocabulary `get_metadata` already returns. Brave's
/// `schemas` is documented as free-form (`any[]`), so this walks
/// defensively rather than assuming a shape.
fn extract_schema_types(schemas: Option<&serde_json::Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(t)) = map.get("@type") {
                    out.push(t.clone());
                }
                for (_, child) in map {
                    walk(child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            _ => {}
        }
    }
    if let Some(v) = schemas {
        walk(v, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Everything Brave sent for a result that Rover does not normalise,
/// preserved verbatim. Returns `None` when there was nothing left over.
fn build_enrichment(
    extra: serde_json::Map<String, serde_json::Value>,
    schemas: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut map: serde_json::Map<String, serde_json::Value> = extra
        .into_iter()
        .filter(|(k, v)| !ENRICHMENT_SKIP.contains(&k.as_str()) && !v.is_null())
        .collect();
    if let Some(s) = schemas
        && !s.is_null()
    {
        map.insert("schemas".to_string(), s);
    }
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::request::{SafeSearch, SearchOverrides};

    fn req() -> SearchRequest {
        SearchRequest::build(
            "rust async trait",
            &SearchConfig::default(),
            SearchOverrides::default(),
        )
        .unwrap()
    }

    fn provider() -> BraveProvider {
        BraveProvider::new(SearchConfig::default(), "rover-test/0")
    }

    fn params(url: &url::Url) -> std::collections::HashMap<String, Vec<String>> {
        let mut out: std::collections::HashMap<String, Vec<String>> = Default::default();
        for (k, v) in url.query_pairs() {
            out.entry(k.into_owned()).or_default().push(v.into_owned());
        }
        out
    }

    #[test]
    fn url_carries_every_mapped_parameter_and_no_credential() {
        let mut r = req();
        r.extra_snippets = true;
        r.include_fetch_metadata = true;
        r.freshness = Some(crate::search::request::Freshness::Week);
        r.safe_search = SafeSearch::Strict;
        r.goggles = vec!["https://example.com/a.goggle".into()];
        let url = provider().build_url(&r).unwrap();
        let p = params(&url);

        assert_eq!(p["q"], vec!["rust async trait"]);
        assert_eq!(p["count"], vec!["10"]);
        assert_eq!(p["offset"], vec!["0"]);
        assert_eq!(p["country"], vec!["US"]);
        assert_eq!(p["search_lang"], vec!["en"]);
        assert_eq!(p["ui_lang"], vec!["en-US"]);
        assert_eq!(p["safesearch"], vec!["strict"]);
        assert_eq!(p["spellcheck"], vec!["true"]);
        assert_eq!(p["freshness"], vec!["pw"]);
        assert_eq!(p["extra_snippets"], vec!["true"]);
        assert_eq!(p["include_fetch_metadata"], vec!["true"]);
        assert_eq!(p["result_filter"], vec!["query,web"]);
        assert_eq!(p["text_decorations"], vec!["false"]);
        assert_eq!(p["goggles"], vec!["https://example.com/a.goggle"]);

        // The credential is a header, never a query parameter.
        let raw = url.as_str();
        assert!(!raw.contains("token"), "{raw}");
        assert!(!raw.contains("key"), "{raw}");
    }

    #[test]
    fn optional_parameters_are_omitted_when_off() {
        let url = provider().build_url(&req()).unwrap();
        let p = params(&url);
        assert!(!p.contains_key("extra_snippets"));
        assert!(!p.contains_key("include_fetch_metadata"));
        assert!(!p.contains_key("freshness"));
        assert!(!p.contains_key("goggles"));
    }

    #[test]
    fn multiple_goggles_become_repeated_parameters() {
        let mut r = req();
        r.goggles = vec!["a".into(), "b".into(), "c".into()];
        let url = provider().build_url(&r).unwrap();
        assert_eq!(params(&url)["goggles"], vec!["a", "b", "c"]);
    }

    #[test]
    fn provider_debug_never_prints_a_key() {
        // SAFETY: the value is read through `SearchConfig::api_key()` only;
        // this test asserts Debug output, which never consults the env.
        let p = provider();
        let s = format!("{p:?}");
        assert!(s.contains("BRAVE_SEARCH_API_KEY"), "{s}");
        assert!(s.contains("api.search.brave.com"), "{s}");
    }

    #[test]
    fn status_classification_covers_the_documented_failures() {
        assert!(matches!(
            classify_status(401, None, ""),
            SearchError::AuthFailed { .. }
        ));
        assert!(matches!(
            classify_status(403, None, ""),
            SearchError::SubscriptionDenied { .. }
        ));
        assert!(matches!(
            classify_status(429, Some(3), ""),
            SearchError::RateLimited {
                retry_after_secs: Some(3)
            }
        ));
        assert!(matches!(
            classify_status(422, None, ""),
            SearchError::InvalidRequest(_)
        ));
        assert!(matches!(
            classify_status(503, None, ""),
            SearchError::Upstream { .. }
        ));
    }

    #[test]
    fn provider_error_code_refines_the_status() {
        // A 422 that is really a quota problem, not a validation problem.
        let body = r#"{"error":{"code":"QUOTA_LIMITED","detail":"monthly quota reached"}}"#;
        let e = classify_status(422, None, body);
        assert!(matches!(e, SearchError::QuotaExhausted { .. }), "{e}");
        assert!(e.to_string().contains("monthly quota reached"), "{e}");

        let body = r#"{"error":{"code":"SUBSCRIPTION_TOKEN_INVALID","detail":"bad token"}}"#;
        let e = classify_status(422, None, body);
        assert!(matches!(e, SearchError::AuthFailed { .. }), "{e}");

        let body = r#"{"error":{"code":"RATE_LIMITED"}}"#;
        let e = classify_status(422, Some(5), body);
        assert!(
            matches!(
                e,
                SearchError::RateLimited {
                    retry_after_secs: Some(5)
                }
            ),
            "{e}"
        );
    }

    #[test]
    fn backoff_is_bounded() {
        for a in 0u8..=10 {
            assert!(backoff(a) <= Duration::from_secs(8), "attempt {a}");
        }
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(1), Duration::from_secs(2));
    }

    #[test]
    fn minimal_result_maps_without_optional_fields() {
        let raw: wire::WebSearchApiResponse = serde_json::from_str(
            r#"{"web":{"results":[{"title":"T","url":"https://example.com/a"}]}}"#,
        )
        .unwrap();
        let out = map_response(raw, &req());
        assert_eq!(out.results.len(), 1);
        let r = &out.results[0];
        assert_eq!(r.rank, 1);
        assert_eq!(r.title, "T");
        assert_eq!(r.url, "https://example.com/a");
        assert!(r.description.is_none());
        assert!(r.extra_snippets.is_empty());
        assert!(r.source.is_none());
        assert!(r.enrichment.is_none());
        // The query falls back to what Rover sent.
        assert_eq!(out.query.original, "rust async trait");
        assert_eq!(out.provider, "brave");
    }

    #[test]
    fn results_without_a_url_are_dropped_and_ranks_stay_dense() {
        let raw: wire::WebSearchApiResponse = serde_json::from_str(
            r#"{"web":{"results":[
                 {"title":"A","url":"https://a/"},
                 {"title":"no url"},
                 {"title":"B","url":"https://b/"}
               ]}}"#,
        )
        .unwrap();
        let out = map_response(raw, &req());
        assert_eq!(out.results.len(), 2);
        assert_eq!(out.results[0].rank, 1);
        assert_eq!(out.results[1].rank, 2);
        assert_eq!(out.results[1].title, "B");
    }

    #[test]
    fn unknown_provider_fields_never_break_parsing() {
        let raw: wire::WebSearchApiResponse = serde_json::from_str(
            r#"{"brand_new_top_level":{"x":1},
                "query":{"original":"q","brand_new_query_field":true},
                "web":{"results":[
                  {"title":"T","url":"https://a/","brand_new_result_field":[1,2,3]}
                ]}}"#,
        )
        .unwrap();
        let out = map_response(raw, &req());
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.query.original, "q");
    }

    #[test]
    fn enrichment_captures_unmodelled_fields_only_when_requested() {
        let json = r#"{"web":{"results":[{
            "title":"T","url":"https://a/","type":"search_result","is_source_local":false,
            "article":{"author":[{"name":"A"}]},
            "rating":{"value":4.5},
            "schemas":[{"@type":"Article","headline":"H"}]
        }]}}"#;

        let mut r = req();
        r.enrichment = false;
        let out = map_response(serde_json::from_str(json).unwrap(), &r);
        assert!(out.results[0].enrichment.is_none());
        // The typed projection still happens regardless.
        assert_eq!(out.results[0].schema_types, vec!["Article".to_string()]);

        r.enrichment = true;
        let out = map_response(serde_json::from_str(json).unwrap(), &r);
        let e = out.results[0].enrichment.as_ref().unwrap();
        assert!(e.get("article").is_some(), "{e}");
        assert!(e.get("rating").is_some(), "{e}");
        assert!(e.get("schemas").is_some(), "{e}");
        // Bookkeeping and already-typed keys stay out of the bag.
        assert!(e.get("type").is_none(), "{e}");
        assert!(e.get("is_source_local").is_none(), "{e}");
        assert!(e.get("title").is_none(), "{e}");
        assert!(e.get("url").is_none(), "{e}");
    }

    #[test]
    fn rich_result_metadata_is_preserved() {
        let raw: wire::WebSearchApiResponse = serde_json::from_str(
            r#"{"web":{"results":[{
                "title":"Rust","url":"https://doc.rust-lang.org/",
                "description":"The book","extra_snippets":["one","two"],
                "age":"2 days ago","page_age":"2026-09-01T00:00:00",
                "page_fetched":"2026-09-05T00:00:00","fetched_content_timestamp":1757030400,
                "language":"en","family_friendly":true,"subtype":"generic","is_live":false,
                "content_type":"text/html",
                "profile":{"name":"Rust Docs","long_name":"The Rust Documentation",
                           "url":"https://doc.rust-lang.org/","img":"https://i/x.png"},
                "meta_url":{"scheme":"https","netloc":"doc.rust-lang.org",
                            "hostname":"doc.rust-lang.org","favicon":"https://f/i.ico","path":"› book"},
                "thumbnail":{"src":"https://t/s.png","original":"https://o/s.png",
                             "alt":"cover","width":100,"height":50,"logo":false},
                "icons":[{"href":"https://i/32.png","sizes":"32x32","rel":"icon","type":"image/png","ext":"png"}]
            }]}}"#,
        )
        .unwrap();
        let out = map_response(raw, &req());
        let r = &out.results[0];
        assert_eq!(r.description.as_deref(), Some("The book"));
        assert_eq!(r.extra_snippets, vec!["one", "two"]);
        assert_eq!(r.age.as_deref(), Some("2 days ago"));
        assert_eq!(r.page_age.as_deref(), Some("2026-09-01T00:00:00"));
        assert_eq!(r.page_fetched.as_deref(), Some("2026-09-05T00:00:00"));
        assert_eq!(r.fetched_content_timestamp, Some(1757030400));
        assert_eq!(r.language.as_deref(), Some("en"));
        assert_eq!(r.family_friendly, Some(true));
        assert_eq!(r.subtype.as_deref(), Some("generic"));
        assert_eq!(r.is_live, Some(false));
        assert_eq!(r.content_type.as_deref(), Some("text/html"));
        let s = r.source.as_ref().unwrap();
        assert_eq!(s.name.as_deref(), Some("Rust Docs"));
        assert_eq!(s.long_name.as_deref(), Some("The Rust Documentation"));
        assert_eq!(s.hostname.as_deref(), Some("doc.rust-lang.org"));
        assert_eq!(s.favicon.as_deref(), Some("https://f/i.ico"));
        let t = r.thumbnail.as_ref().unwrap();
        assert_eq!(t.src.as_deref(), Some("https://t/s.png"));
        assert_eq!(t.alt.as_deref(), Some("cover"));
        assert_eq!(t.width, Some(100));
        assert_eq!(r.icons.len(), 1);
        assert_eq!(r.icons[0].href, "https://i/32.png");
        assert_eq!(r.icons[0].icon_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn query_metadata_is_preserved() {
        let raw: wire::WebSearchApiResponse = serde_json::from_str(
            r#"{"query":{
                "original":"teh rust book","altered":"the rust book","cleaned":"the rust book",
                "safesearch":true,"show_strict_warning":false,"is_navigational":true,
                "is_geolocal":false,"is_trending":false,"is_news_breaking":false,
                "more_results_available":true,"country":"us",
                "language":{"main":"en"},"related_queries":["rust by example"],
                "search_operators":{"applied":true,"cleaned_query":"rust book","sites":["docs.rs"]}
            },"web":{"results":[]}}"#,
        )
        .unwrap();
        let out = map_response(raw, &req());
        let q = &out.query;
        assert_eq!(q.original, "teh rust book");
        assert_eq!(q.altered.as_deref(), Some("the rust book"));
        assert_eq!(q.cleaned.as_deref(), Some("the rust book"));
        assert_eq!(q.safe_search_active, Some(true));
        assert_eq!(q.strict_filter_warning, Some(false));
        assert_eq!(q.is_navigational, Some(true));
        assert_eq!(q.more_results_available, Some(true));
        assert_eq!(q.country.as_deref(), Some("us"));
        assert_eq!(q.language.as_deref(), Some("en"));
        assert_eq!(q.related_queries, vec!["rust by example"]);
        let ops = q.operators.as_ref().unwrap();
        assert!(ops.applied);
        assert_eq!(ops.cleaned_query.as_deref(), Some("rust book"));
        assert_eq!(ops.sites, vec!["docs.rs"]);
        // The request's paging parameters are echoed back.
        assert_eq!(q.count, 10);
        assert_eq!(q.offset, 0);
    }

    #[test]
    fn missing_web_block_yields_zero_results_not_an_error() {
        let raw: wire::WebSearchApiResponse =
            serde_json::from_str(r#"{"query":{"original":"q"}}"#).unwrap();
        let out = map_response(raw, &req());
        assert!(out.results.is_empty());
        assert_eq!(out.query.original, "q");
    }

    #[test]
    fn schema_types_are_extracted_and_deduped() {
        let v = serde_json::json!([
            {"@type": "Article", "author": {"@type": "Person", "name": "A"}},
            {"@type": "Article"}
        ]);
        assert_eq!(
            extract_schema_types(Some(&v)),
            vec!["Article".to_string(), "Person".to_string()]
        );
        assert!(extract_schema_types(None).is_empty());
    }
}

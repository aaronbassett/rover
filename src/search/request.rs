//! The normalised, validated search request.
//!
//! Callers (the MCP `search` tool, `rover search`) hand in
//! [`SearchOverrides`] — everything optional — and this module folds it
//! over the `[search]` config defaults, validates the result, and produces
//! a [`SearchRequest`] that the provider layer can serialise directly.
//!
//! Validation happens *before* any network call, so a bad argument costs a
//! clear error rather than a billable request.
//!
//! This module is compiled unconditionally: `[search]` config validation
//! runs in every build, so a single `rover.toml` stays valid whether or not
//! `web-search` was compiled in.

use crate::config::SearchConfig;
use crate::search::SearchError;

/// Brave's hard ceiling on results per page.
pub const MAX_COUNT: u8 = 20;

/// Brave's hard ceiling on the page index.
pub const MAX_OFFSET: u8 = 9;

/// Sanity cap on `[search] max_retries`. Every retry is a *billable* search
/// request, so an unbounded (or merely large) value turns a transient
/// provider blip into a bill.
pub const MAX_RETRIES: u8 = 5;

/// Sanity cap on `[search] requests_per_minute`. Client-side pacing exists
/// to stay under the provider's limit; a value this far above any published
/// tier is a typo, not a plan.
pub const MAX_REQUESTS_PER_MINUTE: u32 = 6000;

/// Brave's documented ceiling on simultaneously applied Goggles.
pub const MAX_GOGGLES: usize = 3;

/// Brave's documented query limits.
pub const MAX_QUERY_CHARS: usize = 600;
pub const MAX_QUERY_WORDS: usize = 75;

/// Brave's documented `country` values.
pub const COUNTRIES: &[&str] = &[
    "AR", "AU", "AT", "BE", "BR", "CA", "CL", "DK", "FI", "FR", "DE", "GR", "HK", "IN", "ID", "IT",
    "JP", "KR", "MY", "MX", "NL", "NZ", "NO", "CN", "PL", "PT", "PH", "RU", "SA", "ZA", "ES", "SE",
    "CH", "TW", "TR", "GB", "US", "ALL",
];

/// Brave's documented `search_lang` values.
pub const LANGUAGES: &[&str] = &[
    "ar", "eu", "bn", "bg", "ca", "zh-hans", "zh-hant", "hr", "cs", "da", "nl", "en", "en-gb",
    "et", "fi", "fr", "gl", "de", "el", "gu", "he", "hi", "hu", "is", "it", "ja", "jp", "kn", "ko",
    "lv", "lt", "ms", "ml", "mr", "nb", "pl", "pt-br", "pt-pt", "pa", "ro", "ru", "sr", "sk", "sl",
    "es", "sv", "ta", "te", "th", "tr", "uk", "vi",
];

/// Brave's documented `ui_lang` values.
pub const UI_LANGUAGES: &[&str] = &[
    "es-AR", "en-AU", "de-AT", "nl-BE", "fr-BE", "pt-BR", "en-CA", "fr-CA", "es-CL", "da-DK",
    "fi-FI", "fr-FR", "de-DE", "el-GR", "zh-HK", "en-IN", "en-ID", "it-IT", "ja-JP", "ko-KR",
    "en-MY", "es-MX", "nl-NL", "en-NZ", "no-NO", "zh-CN", "pl-PL", "en-PH", "ru-RU", "en-ZA",
    "es-ES", "sv-SE", "fr-CH", "de-CH", "zh-TW", "tr-TR", "en-GB", "en-US", "es-US",
];

/// Adult-content filtering level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeSearch {
    /// No filtering.
    Off,
    /// Filter explicit media but allow adult domains. Brave's default.
    Moderate,
    /// Drop all adult content.
    Strict,
}

impl SafeSearch {
    pub fn parse(s: &str) -> Result<Self, SearchError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "moderate" => Ok(Self::Moderate),
            "strict" => Ok(Self::Strict),
            other => Err(SearchError::InvalidRequest(format!(
                "unknown safe_search `{other}` (expected one of: off, moderate, strict)"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Moderate => "moderate",
            Self::Strict => "strict",
        }
    }
}

/// A page-age filter.
///
/// Rover accepts human names (`day`, `week`, `month`, `year`), Brave's own
/// short codes (`pd`, `pw`, `pm`, `py`), and an explicit date range written
/// either Rover-style (`2024-01-01..2024-06-30`) or Brave-style
/// (`2024-01-01to2024-06-30`). Everything normalises to the wire form Brave
/// expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// Pages aged 24 hours or less.
    Day,
    /// Pages aged 7 days or less.
    Week,
    /// Pages aged 31 days or less.
    Month,
    /// Pages aged 365 days or less.
    Year,
    /// An inclusive `start..end` calendar-date range.
    Range { start: String, end: String },
}

impl Freshness {
    pub fn parse(s: &str) -> Result<Self, SearchError> {
        let raw = s.trim();
        let lower = raw.to_ascii_lowercase();
        match lower.as_str() {
            "day" | "pd" | "24h" => return Ok(Self::Day),
            "week" | "pw" => return Ok(Self::Week),
            "month" | "pm" => return Ok(Self::Month),
            "year" | "py" => return Ok(Self::Year),
            _ => {}
        }

        // A range: `START..END` or `STARTtoEND`.
        let split = split_range(raw, &lower).ok_or_else(|| {
            SearchError::InvalidRequest(format!(
                "unknown freshness `{raw}` (expected day, week, month, year, or a date range \
                 like 2024-01-01..2024-06-30)"
            ))
        })?;
        let (start, end) = (split.0.trim(), split.1.trim());
        let start_d = parse_date(start)?;
        let end_d = parse_date(end)?;
        if start_d > end_d {
            return Err(SearchError::InvalidRequest(format!(
                "freshness range start `{start}` is after end `{end}`"
            )));
        }
        Ok(Self::Range {
            start: start.to_string(),
            end: end.to_string(),
        })
    }

    /// The value to put on the wire.
    pub fn to_wire(&self) -> String {
        match self {
            Self::Day => "pd".to_string(),
            Self::Week => "pw".to_string(),
            Self::Month => "pm".to_string(),
            Self::Year => "py".to_string(),
            Self::Range { start, end } => format!("{start}to{end}"),
        }
    }
}

/// Split a freshness range into its two halves.
///
/// `..` is unambiguous: whatever follows it is meant as a date, so a bad
/// half should be reported as a bad *date*. `to` is not — it is a common
/// substring of ordinary words ("october", "tomorrow"), and splitting on it
/// unconditionally answered "invalid date `oc`" to someone who simply
/// mistyped a shorthand. So the `to` form is only taken when both halves
/// actually parse as dates; anything else falls through to the "unknown
/// freshness" diagnostic, which is the one that helps.
///
/// `lower` is `raw` lowercased with [`str::to_ascii_lowercase`], so byte
/// offsets into it index `raw` identically — which is what lets `TO` work
/// while the returned halves keep the caller's original casing.
fn split_range<'a>(raw: &'a str, lower: &str) -> Option<(&'a str, &'a str)> {
    if let Some(i) = lower.find("..") {
        return Some((&raw[..i], &raw[i + 2..]));
    }
    let mut from = 0;
    while let Some(rel) = lower[from..].find("to") {
        let i = from + rel;
        let (a, b) = (&raw[..i], &raw[i + 2..]);
        if parse_date(a.trim()).is_ok() && parse_date(b.trim()).is_ok() {
            return Some((a, b));
        }
        from = i + 2;
    }
    None
}

fn parse_date(s: &str) -> Result<jiff::civil::Date, SearchError> {
    s.parse::<jiff::civil::Date>().map_err(|_| {
        SearchError::InvalidRequest(format!(
            "invalid date `{s}` in freshness range (expected YYYY-MM-DD)"
        ))
    })
}

/// Per-call overrides. Every field is optional; anything left `None` falls
/// back to the corresponding `[search]` config default.
#[derive(Debug, Clone, Default)]
pub struct SearchOverrides {
    pub count: Option<u8>,
    pub offset: Option<u8>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub ui_language: Option<String>,
    pub safe_search: Option<String>,
    pub freshness: Option<String>,
    pub extra_snippets: Option<bool>,
    pub spellcheck: Option<bool>,
    pub include_fetch_metadata: Option<bool>,
    pub enrichment: Option<bool>,
    /// Replaces the config's default Goggles when `Some` (including
    /// `Some(vec![])`, which turns default ranking rules off for this call).
    pub goggles: Option<Vec<String>>,
    /// Restrict results to these domains. Composed into the query as
    /// `site:` operators.
    pub site: Vec<String>,
    /// Drop results from these domains. Composed into the query as
    /// `NOT site:` operators.
    pub exclude_sites: Vec<String>,
}

/// A validated, ready-to-send search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The final query string, including any operators Rover composed in.
    pub query: String,
    pub count: u8,
    pub offset: u8,
    pub country: String,
    pub language: String,
    pub ui_language: String,
    pub safe_search: SafeSearch,
    pub freshness: Option<Freshness>,
    pub extra_snippets: bool,
    pub spellcheck: bool,
    pub include_fetch_metadata: bool,
    pub goggles: Vec<String>,
    /// Not sent to the provider: whether the caller wants the untyped
    /// enrichment bag kept on each result.
    pub enrichment: bool,
}

impl SearchRequest {
    /// Fold `overrides` over `cfg` and validate the result.
    pub fn build(
        query: &str,
        cfg: &SearchConfig,
        overrides: SearchOverrides,
    ) -> Result<Self, SearchError> {
        let base = query.trim();
        if base.is_empty() {
            return Err(SearchError::InvalidRequest(
                "query must not be empty".to_string(),
            ));
        }

        let query = compose_query(base, &overrides.site, &overrides.exclude_sites)?;

        let count = overrides.count.unwrap_or(cfg.count);
        if !(1..=MAX_COUNT).contains(&count) {
            return Err(SearchError::InvalidRequest(format!(
                "count must be between 1 and {MAX_COUNT} (got {count})"
            )));
        }

        let offset = overrides.offset.unwrap_or(0);
        if offset > MAX_OFFSET {
            return Err(SearchError::InvalidRequest(format!(
                "offset must be between 0 and {MAX_OFFSET} (got {offset}); each page is a \
                 separate billable request, so check `more_results_available` before paging"
            )));
        }

        let country = normalise_country(
            overrides.country.as_deref().unwrap_or(&cfg.country),
            "country",
        )?;
        let language = normalise_from(
            overrides.language.as_deref().unwrap_or(&cfg.language),
            LANGUAGES,
            "language",
            false,
        )?;
        let ui_language = normalise_from(
            overrides.ui_language.as_deref().unwrap_or(&cfg.ui_language),
            UI_LANGUAGES,
            "ui_language",
            true,
        )?;
        let safe_search =
            SafeSearch::parse(overrides.safe_search.as_deref().unwrap_or(&cfg.safe_search))?;
        let freshness = match overrides.freshness.as_deref() {
            Some(f) if !f.trim().is_empty() => Some(Freshness::parse(f)?),
            _ => None,
        };

        let goggles = overrides.goggles.unwrap_or_else(|| cfg.goggles.clone());
        validate_goggles(&goggles)?;

        Ok(Self {
            query,
            count,
            offset,
            country,
            language,
            ui_language,
            safe_search,
            freshness,
            extra_snippets: overrides.extra_snippets.unwrap_or(cfg.extra_snippets),
            spellcheck: overrides.spellcheck.unwrap_or(cfg.spellcheck),
            include_fetch_metadata: overrides
                .include_fetch_metadata
                .unwrap_or(cfg.include_fetch_metadata),
            goggles,
            enrichment: overrides.enrichment.unwrap_or(cfg.enrichment),
        })
    }
}

/// Append `site:` / `NOT site:` operators to the user's query, then enforce
/// the provider's query-length limits on the composed result.
///
/// The operators are the ones Brave documents; a caller who wants something
/// more elaborate can type operators straight into `query` instead.
fn compose_query(
    base: &str,
    site: &[String],
    exclude_sites: &[String],
) -> Result<String, SearchError> {
    let mut q = base.to_string();

    if !site.is_empty() {
        let terms: Vec<String> = site
            .iter()
            .map(|d| validate_domain(d).map(|d| format!("site:{d}")))
            .collect::<Result<_, _>>()?;
        // `OR` binds tighter than the implicit `AND` between terms, so an
        // unparenthesised alternation splits the whole query in two:
        // `rust async site:a.com OR site:b.com` asks for (rust AND async AND
        // site:a.com) OR (site:b.com) — the second domain comes back
        // unfiltered by the query. The parens keep the alternation to the
        // domains, which is the only thing the caller meant to alternate.
        // Grouping is safe because `validate_domain` rejects parentheses, so
        // a domain can never close the one opened here.
        q.push(' ');
        if terms.len() == 1 {
            q.push_str(&terms[0]);
        } else {
            q.push_str(&format!("({})", terms.join(" OR ")));
        }
    }
    // Exclusions need no grouping: each `NOT site:x` negates the single term
    // it prefixes and joins the rest by the implicit `AND`, with no `OR` in
    // the chain to reassociate it. They follow the group rather than sit
    // inside it, so an exclusion applies to the whole query and not to one
    // branch of the alternation.
    for d in exclude_sites {
        let d = validate_domain(d)?;
        q.push_str(&format!(" NOT site:{d}"));
    }

    let chars = q.chars().count();
    if chars > MAX_QUERY_CHARS {
        return Err(SearchError::InvalidRequest(format!(
            "composed query is {chars} characters; the provider's limit is {MAX_QUERY_CHARS}"
        )));
    }
    let words = q.split_whitespace().count();
    if words > MAX_QUERY_WORDS {
        return Err(SearchError::InvalidRequest(format!(
            "composed query is {words} words; the provider's limit is {MAX_QUERY_WORDS}"
        )));
    }
    Ok(q)
}

/// A domain for a `site:` operator. Rejecting whitespace and the operator
/// delimiters keeps a caller from smuggling extra operators (or a whole
/// second clause) in through what is meant to be one hostname.
fn validate_domain(d: &str) -> Result<String, SearchError> {
    let d = d.trim();
    if d.is_empty() {
        return Err(SearchError::InvalidRequest(
            "site/exclude_sites entries must not be empty".to_string(),
        ));
    }
    let ok = d
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !ok {
        return Err(SearchError::InvalidRequest(format!(
            "`{d}` is not a bare domain; site/exclude_sites take hostnames like `docs.rs` \
             (put anything more elaborate directly in the query as a search operator)"
        )));
    }
    Ok(d.to_ascii_lowercase())
}

fn validate_goggles(goggles: &[String]) -> Result<(), SearchError> {
    if goggles.len() > MAX_GOGGLES {
        return Err(SearchError::InvalidRequest(format!(
            "at most {MAX_GOGGLES} goggles may be applied at once (got {})",
            goggles.len()
        )));
    }
    for g in goggles {
        if g.trim().is_empty() {
            return Err(SearchError::InvalidRequest(
                "goggles entries must not be empty (each is a Goggle URL or an inline \
                 Goggle definition)"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn normalise_country(value: &str, field: &str) -> Result<String, SearchError> {
    let upper = value.trim().to_ascii_uppercase();
    if COUNTRIES.contains(&upper.as_str()) {
        return Ok(upper);
    }
    Err(unknown_value(field, value, COUNTRIES))
}

/// Match `value` case-insensitively against `allowed`, returning the
/// canonically-cased entry. `preserve_case` distinguishes the two shapes
/// Brave uses: `search_lang` values are lowercase (`pt-br`), `ui_lang`
/// values are mixed (`pt-BR`).
fn normalise_from(
    value: &str,
    allowed: &'static [&'static str],
    field: &str,
    preserve_case: bool,
) -> Result<String, SearchError> {
    let v = value.trim();
    if let Some(hit) = allowed.iter().find(|a| a.eq_ignore_ascii_case(v)) {
        return Ok(if preserve_case {
            (*hit).to_string()
        } else {
            hit.to_ascii_lowercase()
        });
    }
    Err(unknown_value(field, value, allowed))
}

/// A diagnostic that names the field and shows a usable prefix of the
/// accepted set without dumping fifty codes into a terminal.
fn unknown_value(field: &str, value: &str, allowed: &[&str]) -> SearchError {
    let sample: Vec<&str> = allowed.iter().take(8).copied().collect();
    SearchError::InvalidRequest(format!(
        "unknown {field} `{value}`; the provider accepts {} values including {} \
         (see https://rover-fetch.com/docs/web-search for the full list)",
        allowed.len(),
        sample.join(", "),
    ))
}

/// Whether sending the subscription token to `url` would put it on the wire
/// in the clear.
///
/// `http` cannot simply be rejected: pointing `base_url` at a local mock is
/// how the whole search test suite runs, and fronting the API with a
/// loopback proxy is a legitimate deployment. Neither leaves the machine, so
/// neither is warned about — only `http` to a host that is somewhere else.
fn sends_credential_in_cleartext(url: &url::Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(d)) => !d.eq_ignore_ascii_case("localhost"),
        // `is_loopback` rather than an equality test: the whole 127.0.0.0/8
        // block is local, and a mock server is not obliged to bind .0.1.
        Some(url::Host::Ipv4(ip)) => !ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => !ip.is_loopback(),
        None => true,
    }
}

/// Validate a `[search]` block. Called from `config::validate` so a typo in
/// `rover.toml` fails at load time in every build — including one compiled
/// without `web-search`, so a single config file stays portable.
pub fn validate_config(cfg: &mut SearchConfig) -> Result<(), String> {
    if cfg.api_key_env.trim().is_empty() {
        return Err("search.api_key_env must not be empty".to_string());
    }
    let url = url::Url::parse(&cfg.base_url)
        .map_err(|e| format!("search.base_url `{}` is not a URL: {e}", cfg.base_url))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "search.base_url `{}` must be http or https",
            cfg.base_url
        ));
    }
    if sends_credential_in_cleartext(&url) {
        tracing::warn!(
            target: "rover::search",
            base_url = %cfg.base_url,
            "search.base_url is http:// to a non-local host; the subscription token is sent in \
             cleartext on every search. Use https, or front the API on loopback.",
        );
    }
    if !(1..=MAX_COUNT).contains(&cfg.count) {
        return Err(format!(
            "search.count must be between 1 and {MAX_COUNT} (got {})",
            cfg.count
        ));
    }
    cfg.country = normalise_country(&cfg.country, "search.country").map_err(|e| e.to_string())?;
    cfg.language = normalise_from(&cfg.language, LANGUAGES, "search.language", false)
        .map_err(|e| e.to_string())?;
    cfg.ui_language = normalise_from(&cfg.ui_language, UI_LANGUAGES, "search.ui_language", true)
        .map_err(|e| e.to_string())?;
    let safe = SafeSearch::parse(&cfg.safe_search).map_err(|e| e.to_string())?;
    cfg.safe_search = safe.as_str().to_string();
    validate_goggles(&cfg.goggles).map_err(|e| e.to_string())?;
    if cfg.timeout_secs == 0 {
        return Err("search.timeout_secs must be > 0".to_string());
    }
    if cfg.max_retries > MAX_RETRIES {
        return Err(format!(
            "search.max_retries ({}) exceeds sanity cap {MAX_RETRIES} — every retry is a \
             billable search request",
            cfg.max_retries
        ));
    }
    if cfg.requests_per_minute == 0 {
        return Err("search.requests_per_minute must be > 0".to_string());
    }
    if cfg.requests_per_minute > MAX_REQUESTS_PER_MINUTE {
        return Err(format!(
            "search.requests_per_minute ({}) exceeds sanity cap {MAX_REQUESTS_PER_MINUTE}",
            cfg.requests_per_minute
        ));
    }
    if cfg.retry_after_ceiling.is_zero() {
        return Err("search.retry_after_ceiling must be > 0".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SearchConfig {
        SearchConfig::default()
    }

    #[test]
    fn defaults_flow_through_when_nothing_is_overridden() {
        let r =
            SearchRequest::build("rust async trait", &cfg(), SearchOverrides::default()).unwrap();
        assert_eq!(r.query, "rust async trait");
        assert_eq!(r.count, 10);
        assert_eq!(r.offset, 0);
        assert_eq!(r.country, "US");
        assert_eq!(r.language, "en");
        assert_eq!(r.ui_language, "en-US");
        assert_eq!(r.safe_search, SafeSearch::Moderate);
        assert!(r.freshness.is_none());
        assert!(!r.extra_snippets);
        assert!(r.spellcheck);
        assert!(!r.include_fetch_metadata);
        assert!(!r.enrichment);
        assert!(r.goggles.is_empty());
    }

    #[test]
    fn overrides_win_over_config() {
        let o = SearchOverrides {
            count: Some(3),
            offset: Some(2),
            country: Some("gb".into()),
            language: Some("FR".into()),
            ui_language: Some("fr-fr".into()),
            safe_search: Some("STRICT".into()),
            freshness: Some("week".into()),
            extra_snippets: Some(true),
            spellcheck: Some(false),
            include_fetch_metadata: Some(true),
            enrichment: Some(true),
            ..Default::default()
        };
        let r = SearchRequest::build("q", &cfg(), o).unwrap();
        assert_eq!(r.count, 3);
        assert_eq!(r.offset, 2);
        // Case is normalised to the provider's canonical spelling.
        assert_eq!(r.country, "GB");
        assert_eq!(r.language, "fr");
        assert_eq!(r.ui_language, "fr-FR");
        assert_eq!(r.safe_search, SafeSearch::Strict);
        assert_eq!(r.freshness, Some(Freshness::Week));
        assert!(r.extra_snippets);
        assert!(!r.spellcheck);
        assert!(r.include_fetch_metadata);
        assert!(r.enrichment);
    }

    #[test]
    fn empty_query_is_rejected() {
        let e = SearchRequest::build("   ", &cfg(), SearchOverrides::default()).unwrap_err();
        assert!(matches!(e, SearchError::InvalidRequest(_)));
        assert!(e.to_string().contains("empty"));
    }

    #[test]
    fn count_bounds_are_enforced() {
        for bad in [0u8, 21, 255] {
            let o = SearchOverrides {
                count: Some(bad),
                ..Default::default()
            };
            let e = SearchRequest::build("q", &cfg(), o).unwrap_err();
            assert!(e.to_string().contains("count must be"), "{e}");
        }
        for good in [1u8, 20] {
            let o = SearchOverrides {
                count: Some(good),
                ..Default::default()
            };
            assert!(SearchRequest::build("q", &cfg(), o).is_ok());
        }
    }

    #[test]
    fn offset_bounds_are_enforced() {
        let o = SearchOverrides {
            offset: Some(10),
            ..Default::default()
        };
        let e = SearchRequest::build("q", &cfg(), o).unwrap_err();
        assert!(e.to_string().contains("offset must be"), "{e}");
        let o = SearchOverrides {
            offset: Some(9),
            ..Default::default()
        };
        assert!(SearchRequest::build("q", &cfg(), o).is_ok());
    }

    #[test]
    fn unknown_enum_values_are_rejected_before_the_network() {
        for (field, o) in [
            (
                "country",
                SearchOverrides {
                    country: Some("XX".into()),
                    ..Default::default()
                },
            ),
            (
                "language",
                SearchOverrides {
                    language: Some("klingon".into()),
                    ..Default::default()
                },
            ),
            (
                "ui_language",
                SearchOverrides {
                    ui_language: Some("xx-YY".into()),
                    ..Default::default()
                },
            ),
            (
                "safe_search",
                SearchOverrides {
                    safe_search: Some("maybe".into()),
                    ..Default::default()
                },
            ),
        ] {
            let e = SearchRequest::build("q", &cfg(), o).unwrap_err();
            assert!(matches!(e, SearchError::InvalidRequest(_)), "{field}: {e}");
        }
    }

    #[test]
    fn freshness_accepts_names_codes_and_ranges() {
        let cases = [
            ("day", "pd"),
            ("DAY", "pd"),
            ("pd", "pd"),
            ("week", "pw"),
            ("month", "pm"),
            ("year", "py"),
            ("2024-01-01..2024-06-30", "2024-01-01to2024-06-30"),
            ("2024-01-01to2024-06-30", "2024-01-01to2024-06-30"),
        ];
        for (input, wire) in cases {
            let f = Freshness::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(f.to_wire(), wire, "input {input}");
        }
    }

    #[test]
    fn freshness_rejects_nonsense_and_inverted_ranges() {
        assert!(Freshness::parse("fortnight").is_err());
        assert!(Freshness::parse("2024-13-01..2024-06-30").is_err());
        let e = Freshness::parse("2024-06-30..2024-01-01").unwrap_err();
        assert!(e.to_string().contains("after end"), "{e}");
    }

    /// `to` is a substring of ordinary words. Splitting on it unconditionally
    /// answered a mistyped shorthand with a date-format complaint about a
    /// date the user never wrote.
    #[test]
    fn a_word_containing_to_is_not_read_as_a_date_range() {
        for word in ["october", "tomorrow", "notice", "stop"] {
            let e = Freshness::parse(word).unwrap_err();
            assert!(e.to_string().contains("unknown freshness"), "{word}: {e}");
        }
    }

    /// The shorthands match case-insensitively, so the range separator must
    /// too — it is the same string the user typed.
    #[test]
    fn an_uppercase_to_separator_is_accepted() {
        let f = Freshness::parse("2024-01-01TO2024-06-30").unwrap();
        assert_eq!(f.to_wire(), "2024-01-01to2024-06-30");
    }

    /// A single domain needs no alternation, so it gets no parentheses.
    #[test]
    fn one_site_composes_a_bare_operator() {
        let o = SearchOverrides {
            site: vec!["Docs.RS".into()],
            ..Default::default()
        };
        let r = SearchRequest::build("async trait", &cfg(), o).unwrap();
        assert_eq!(r.query, "async trait site:docs.rs");
    }

    /// The alternation must be parenthesised. Unparenthesised, `OR` binds
    /// tighter than the implicit `AND`, so everything after the first `OR`
    /// becomes a separate branch that the user's terms never constrain —
    /// silently wrong results rather than an error.
    #[test]
    fn site_and_exclude_sites_compose_documented_operators() {
        let o = SearchOverrides {
            site: vec!["docs.rs".into(), "Doc.Rust-Lang.org".into()],
            exclude_sites: vec!["pinterest.com".into()],
            ..Default::default()
        };
        let r = SearchRequest::build("async trait", &cfg(), o).unwrap();
        assert_eq!(
            r.query,
            "async trait (site:docs.rs OR site:doc.rust-lang.org) NOT site:pinterest.com"
        );
    }

    #[test]
    fn three_sites_stay_inside_one_group() {
        let o = SearchOverrides {
            site: vec!["a.com".into(), "b.com".into(), "c.com".into()],
            exclude_sites: vec!["x.com".into(), "y.com".into()],
            ..Default::default()
        };
        let r = SearchRequest::build("rust async", &cfg(), o).unwrap();
        // Each `NOT` is its own conjunct alongside the group, so an
        // exclusion applies to the whole query, not to one branch of it.
        assert_eq!(
            r.query,
            "rust async (site:a.com OR site:b.com OR site:c.com) NOT site:x.com NOT site:y.com"
        );
    }

    #[test]
    fn site_entries_cannot_smuggle_extra_operators() {
        for bad in ["docs.rs OR site:evil.com", "a b", "\"x\"", "foo.com)"] {
            let o = SearchOverrides {
                site: vec![bad.into()],
                ..Default::default()
            };
            let e = SearchRequest::build("q", &cfg(), o).unwrap_err();
            assert!(e.to_string().contains("bare domain"), "{bad}: {e}");
        }
    }

    #[test]
    fn composed_query_length_limits_are_enforced() {
        let long = "x".repeat(MAX_QUERY_CHARS + 1);
        let e = SearchRequest::build(&long, &cfg(), SearchOverrides::default()).unwrap_err();
        assert!(e.to_string().contains("characters"), "{e}");

        let many_words = "w ".repeat(MAX_QUERY_WORDS + 1);
        let e = SearchRequest::build(&many_words, &cfg(), SearchOverrides::default()).unwrap_err();
        assert!(e.to_string().contains("words"), "{e}");
    }

    #[test]
    fn goggles_default_from_config_and_can_be_cleared_per_call() {
        let mut c = cfg();
        c.goggles = vec!["https://example.com/a.goggle".into()];
        let r = SearchRequest::build("q", &c, SearchOverrides::default()).unwrap();
        assert_eq!(r.goggles.len(), 1);

        let o = SearchOverrides {
            goggles: Some(vec![]),
            ..Default::default()
        };
        let r = SearchRequest::build("q", &c, o).unwrap();
        assert!(r.goggles.is_empty(), "explicit empty list clears defaults");
    }

    #[test]
    fn more_than_three_goggles_is_rejected() {
        let o = SearchOverrides {
            goggles: Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]),
            ..Default::default()
        };
        let e = SearchRequest::build("q", &cfg(), o).unwrap_err();
        assert!(e.to_string().contains("at most 3"), "{e}");
    }

    #[test]
    fn config_validation_normalises_case() {
        let mut c = SearchConfig {
            country: "gb".into(),
            language: "EN-GB".into(),
            ui_language: "en-gb".into(),
            safe_search: "STRICT".into(),
            ..Default::default()
        };
        validate_config(&mut c).unwrap();
        assert_eq!(c.country, "GB");
        assert_eq!(c.language, "en-gb");
        assert_eq!(c.ui_language, "en-GB");
        assert_eq!(c.safe_search, "strict");
    }

    #[test]
    fn config_validation_rejects_bad_values() {
        let bad: Vec<(&str, SearchConfig)> = vec![
            (
                "api_key_env",
                SearchConfig {
                    api_key_env: "  ".into(),
                    ..Default::default()
                },
            ),
            (
                "base_url",
                SearchConfig {
                    base_url: "not a url".into(),
                    ..Default::default()
                },
            ),
            (
                "base_url scheme",
                SearchConfig {
                    base_url: "ftp://example.com/".into(),
                    ..Default::default()
                },
            ),
            (
                "count",
                SearchConfig {
                    count: 0,
                    ..Default::default()
                },
            ),
            (
                "country",
                SearchConfig {
                    country: "ZZ".into(),
                    ..Default::default()
                },
            ),
            (
                "max_retries",
                SearchConfig {
                    max_retries: 6,
                    ..Default::default()
                },
            ),
            (
                "requests_per_minute",
                SearchConfig {
                    requests_per_minute: 0,
                    ..Default::default()
                },
            ),
            (
                "timeout_secs",
                SearchConfig {
                    timeout_secs: 0,
                    ..Default::default()
                },
            ),
        ];
        for (name, mut c) in bad {
            assert!(
                validate_config(&mut c).is_err(),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn default_config_validates() {
        let mut c = SearchConfig::default();
        validate_config(&mut c).unwrap();
    }

    /// The predicate behind the cleartext-credential warning. Asserted here
    /// rather than by capturing log output: nothing in the test suite
    /// installs a subscriber, and the decision — not the formatting — is
    /// what must not regress. A false positive on loopback would make every
    /// wiremock-backed test noisy.
    #[test]
    fn http_to_a_remote_host_is_flagged_but_loopback_is_not() {
        let flagged = |u: &str| sends_credential_in_cleartext(&url::Url::parse(u).unwrap());

        assert!(flagged("http://search-proxy.internal/v1/web/search"));
        assert!(flagged("http://api.search.brave.com/res/v1/web/search"));
        // A private address is still off-machine.
        assert!(flagged("http://10.0.0.5:8080/search"));

        assert!(!flagged("http://127.0.0.1:1234/search"));
        assert!(!flagged("http://127.0.0.2:1234/search"));
        assert!(!flagged("http://localhost:1234/search"));
        assert!(!flagged("http://LocalHost:1234/search"));
        assert!(!flagged("http://[::1]:1234/search"));
        // https never sends the token in the clear, wherever it points.
        assert!(!flagged("https://search-proxy.internal/v1/web/search"));
    }
}

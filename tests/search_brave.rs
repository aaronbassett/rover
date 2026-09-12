//! End-to-end tests for the Brave search provider against a mock server.
//!
//! No test here ever reaches the real Brave API: every request goes to a
//! `wiremock` server via `[search] base_url`, and the credential is a
//! throwaway value in a test-specific environment variable. That is
//! deliberate — the live API is metered and billed, so CI must never touch
//! it.

#![cfg(feature = "web-search")]

use rover::config::SearchConfig;
use rover::search::{SearchError, SearchOverrides, SearchService};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "test-subscription-token-do-not-use";

/// Build a service pointed at `server`, authenticated by a test-specific
/// environment variable so parallel tests never fight over one name.
fn service(server: &MockServer, env_var: &'static str) -> SearchService {
    // SAFETY: `env_var` is unique per test, so no other thread reads or
    // writes it. Values are never removed — the process is a test binary.
    unsafe { std::env::set_var(env_var, KEY) };
    let cfg = SearchConfig {
        api_key_env: env_var.to_string(),
        base_url: format!("{}/res/v1/web/search", server.uri()),
        // Keep the retry loop from adding seconds to a failure test.
        max_retries: 0,
        ..Default::default()
    };
    SearchService::new(&cfg, "rover-test/0")
}

fn ok_body(json: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json)
}

async fn mount_ok(server: &MockServer, json: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(ok_body(json))
        .mount(server)
        .await;
}

async fn mount_status(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(status)
                .insert_header("Content-Type", "application/json")
                .set_body_string(body.to_string()),
        )
        .mount(server)
        .await;
}

/// The single request the server saw, as a parsed URL.
async fn sole_request(server: &MockServer) -> Request {
    let reqs = server.received_requests().await.expect("recorded requests");
    assert_eq!(reqs.len(), 1, "expected exactly one request");
    reqs.into_iter().next().unwrap()
}

fn query_map(req: &Request) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = Default::default();
    for (k, v) in req.url.query_pairs() {
        out.entry(k.into_owned()).or_default().push(v.into_owned());
    }
    out
}

fn minimal_response() -> serde_json::Value {
    serde_json::json!({
        "type": "search",
        "query": {"original": "rust async trait"},
        "web": {"results": [
            {"title": "Async trait", "url": "https://docs.rs/async-trait/"}
        ]}
    })
}

// ---------------------------------------------------------------- requests

#[tokio::test]
async fn request_carries_the_token_as_a_header_never_in_the_url() {
    let server = MockServer::start().await;
    mount_ok(&server, minimal_response()).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_HEADER");

    svc.search_with("rust async trait", SearchOverrides::default())
        .await
        .expect("search");

    let req = sole_request(&server).await;
    let header = req
        .headers
        .get("x-subscription-token")
        .expect("token header present");
    assert_eq!(header.to_str().unwrap(), KEY);
    assert_eq!(
        req.headers.get("accept").unwrap().to_str().unwrap(),
        "application/json"
    );
    // The credential must never be reachable from the URL — that is what
    // ends up in logs, HAR files, and proxy access logs.
    let url = req.url.as_str();
    assert!(!url.contains(KEY), "credential leaked into the URL: {url}");
}

#[tokio::test]
async fn default_request_maps_config_defaults_onto_provider_parameters() {
    let server = MockServer::start().await;
    mount_ok(&server, minimal_response()).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_DEFAULTS");

    svc.search_with("rust async trait", SearchOverrides::default())
        .await
        .expect("search");

    let q = query_map(&sole_request(&server).await);
    assert_eq!(q["q"], vec!["rust async trait"]);
    assert_eq!(q["count"], vec!["10"]);
    assert_eq!(q["offset"], vec!["0"]);
    assert_eq!(q["country"], vec!["US"]);
    assert_eq!(q["search_lang"], vec!["en"]);
    assert_eq!(q["ui_lang"], vec!["en-US"]);
    assert_eq!(q["safesearch"], vec!["moderate"]);
    assert_eq!(q["spellcheck"], vec!["true"]);
    assert_eq!(q["result_filter"], vec!["query,web"]);
    // Rover always asks for undecorated snippets: highlight markers are
    // noise to a model reading them as prose.
    assert_eq!(q["text_decorations"], vec!["false"]);
    // Opt-in parameters stay off.
    assert!(!q.contains_key("extra_snippets"));
    assert!(!q.contains_key("include_fetch_metadata"));
    assert!(!q.contains_key("freshness"));
    assert!(!q.contains_key("goggles"));
}

#[tokio::test]
async fn every_override_reaches_the_provider() {
    let server = MockServer::start().await;
    mount_ok(&server, minimal_response()).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_OVERRIDES");

    svc.search_with(
        "async trait",
        SearchOverrides {
            count: Some(3),
            offset: Some(2),
            country: Some("gb".into()),
            language: Some("fr".into()),
            ui_language: Some("fr-FR".into()),
            safe_search: Some("strict".into()),
            freshness: Some("2024-01-01..2024-06-30".into()),
            extra_snippets: Some(true),
            spellcheck: Some(false),
            include_fetch_metadata: Some(true),
            goggles: Some(vec![
                "https://g/a.goggle".into(),
                "https://g/b.goggle".into(),
            ]),
            site: vec!["docs.rs".into()],
            exclude_sites: vec!["spam.example".into()],
            ..Default::default()
        },
    )
    .await
    .expect("search");

    let q = query_map(&sole_request(&server).await);
    assert_eq!(q["count"], vec!["3"]);
    assert_eq!(q["offset"], vec!["2"]);
    assert_eq!(q["country"], vec!["GB"]);
    assert_eq!(q["search_lang"], vec!["fr"]);
    assert_eq!(q["ui_lang"], vec!["fr-FR"]);
    assert_eq!(q["safesearch"], vec!["strict"]);
    assert_eq!(q["freshness"], vec!["2024-01-01to2024-06-30"]);
    assert_eq!(q["extra_snippets"], vec!["true"]);
    assert_eq!(q["spellcheck"], vec!["false"]);
    assert_eq!(q["include_fetch_metadata"], vec!["true"]);
    assert_eq!(
        q["goggles"],
        vec!["https://g/a.goggle", "https://g/b.goggle"]
    );
    // site / exclude_sites are composed into the query as documented
    // operators, not sent as separate parameters.
    assert_eq!(
        q["q"],
        vec!["async trait site:docs.rs NOT site:spam.example"]
    );
}

#[tokio::test]
async fn freshness_shorthands_all_map_to_provider_codes() {
    for (input, wire) in [
        ("day", "pd"),
        ("week", "pw"),
        ("month", "pm"),
        ("year", "py"),
    ] {
        let server = MockServer::start().await;
        mount_ok(&server, minimal_response()).await;
        let svc = service(&server, "ROVER_TEST_BRAVE_KEY_FRESHNESS");
        svc.search_with(
            "q",
            SearchOverrides {
                freshness: Some(input.into()),
                ..Default::default()
            },
        )
        .await
        .expect("search");
        let q = query_map(&sole_request(&server).await);
        assert_eq!(q["freshness"], vec![wire], "input {input}");
    }
}

#[tokio::test]
async fn invalid_arguments_never_reach_the_network() {
    let server = MockServer::start().await;
    // Deliberately no mock mounted: any request at all fails the test.
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_NOREQ");

    for (label, o) in [
        (
            "count",
            SearchOverrides {
                count: Some(50),
                ..Default::default()
            },
        ),
        (
            "offset",
            SearchOverrides {
                offset: Some(99),
                ..Default::default()
            },
        ),
        (
            "country",
            SearchOverrides {
                country: Some("ZZ".into()),
                ..Default::default()
            },
        ),
        (
            "freshness",
            SearchOverrides {
                freshness: Some("fortnight".into()),
                ..Default::default()
            },
        ),
        (
            "goggles",
            SearchOverrides {
                goggles: Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]),
                ..Default::default()
            },
        ),
    ] {
        let e = svc.search_with("q", o).await.expect_err(label);
        assert!(matches!(e, SearchError::InvalidRequest(_)), "{label}: {e}");
    }

    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "a rejected argument must not cost a billable request"
    );
}

// --------------------------------------------------------------- responses

#[tokio::test]
async fn minimal_response_parses_with_every_optional_field_absent() {
    let server = MockServer::start().await;
    mount_ok(&server, minimal_response()).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_MINIMAL");

    let r = svc
        .search_with("rust async trait", SearchOverrides::default())
        .await
        .expect("search");
    assert_eq!(r.provider, "brave");
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].rank, 1);
    assert_eq!(r.results[0].url, "https://docs.rs/async-trait/");
    assert!(r.results[0].description.is_none());
    assert!(r.results[0].source.is_none());
    assert!(r.results[0].enrichment.is_none());
    assert_eq!(r.query.original, "rust async trait");
}

#[tokio::test]
async fn rich_response_preserves_result_and_query_metadata() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        serde_json::json!({
            "query": {
                "original": "teh rust book",
                "altered": "the rust book",
                "cleaned": "the rust book",
                "safesearch": true,
                "show_strict_warning": true,
                "is_navigational": true,
                "is_geolocal": false,
                "is_trending": true,
                "is_news_breaking": false,
                "more_results_available": true,
                "country": "us",
                "language": {"main": "en"},
                "related_queries": ["rust by example", "rustlings"],
                "search_operators": {
                    "applied": true,
                    "cleaned_query": "rust book",
                    "sites": ["doc.rust-lang.org"]
                }
            },
            "web": {"results": [{
                "title": "The Rust Programming Language",
                "url": "https://doc.rust-lang.org/book/",
                "description": "The book.",
                "extra_snippets": ["Chapter 1", "Chapter 2"],
                "age": "3 days ago",
                "page_age": "2026-09-01T12:00:00",
                "page_fetched": "2026-09-05T09:00:00",
                "fetched_content_timestamp": 1757062800,
                "language": "en",
                "family_friendly": true,
                "type": "search_result",
                "subtype": "generic",
                "is_live": false,
                "content_type": "text/html",
                "profile": {
                    "name": "Rust Docs",
                    "long_name": "The Rust Documentation",
                    "url": "https://doc.rust-lang.org/",
                    "img": "https://img.example/logo.png"
                },
                "meta_url": {
                    "scheme": "https",
                    "netloc": "doc.rust-lang.org",
                    "hostname": "doc.rust-lang.org",
                    "favicon": "https://f.example/icon.ico",
                    "path": "› book"
                },
                "thumbnail": {
                    "src": "https://t.example/s.png",
                    "original": "https://o.example/s.png",
                    "alt": "book cover",
                    "width": 320, "height": 180, "logo": false
                },
                "icons": [{"href": "https://i.example/32.png", "sizes": "32x32",
                           "rel": "icon", "type": "image/png", "ext": "png"}],
                "schemas": [{"@type": "Book", "author": {"@type": "Person", "name": "Steve"}}]
            }]}
        }),
    )
    .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_RICH");

    let r = svc
        .search_with("teh rust book", SearchOverrides::default())
        .await
        .expect("search");

    // Query-level metadata.
    assert_eq!(r.query.altered.as_deref(), Some("the rust book"));
    assert_eq!(r.query.cleaned.as_deref(), Some("the rust book"));
    assert_eq!(r.query.safe_search_active, Some(true));
    assert_eq!(r.query.strict_filter_warning, Some(true));
    assert_eq!(r.query.is_navigational, Some(true));
    assert_eq!(r.query.is_trending, Some(true));
    assert_eq!(r.query.more_results_available, Some(true));
    assert_eq!(r.query.language.as_deref(), Some("en"));
    assert_eq!(r.query.country.as_deref(), Some("us"));
    assert_eq!(r.query.related_queries.len(), 2);
    let ops = r.query.operators.as_ref().expect("operators");
    assert!(ops.applied);
    assert_eq!(ops.sites, vec!["doc.rust-lang.org"]);

    // Per-result metadata.
    let item = &r.results[0];
    assert_eq!(item.description.as_deref(), Some("The book."));
    assert_eq!(item.extra_snippets, vec!["Chapter 1", "Chapter 2"]);
    assert_eq!(item.age.as_deref(), Some("3 days ago"));
    assert_eq!(item.page_age.as_deref(), Some("2026-09-01T12:00:00"));
    assert_eq!(item.page_fetched.as_deref(), Some("2026-09-05T09:00:00"));
    assert_eq!(item.fetched_content_timestamp, Some(1757062800));
    assert_eq!(item.language.as_deref(), Some("en"));
    assert_eq!(item.family_friendly, Some(true));
    assert_eq!(item.subtype.as_deref(), Some("generic"));
    assert_eq!(item.is_live, Some(false));
    assert_eq!(item.content_type.as_deref(), Some("text/html"));
    assert_eq!(item.schema_types, vec!["Book", "Person"]);

    let src = item.source.as_ref().expect("source");
    assert_eq!(src.name.as_deref(), Some("Rust Docs"));
    assert_eq!(src.long_name.as_deref(), Some("The Rust Documentation"));
    assert_eq!(src.hostname.as_deref(), Some("doc.rust-lang.org"));
    assert_eq!(src.favicon.as_deref(), Some("https://f.example/icon.ico"));

    let th = item.thumbnail.as_ref().expect("thumbnail");
    assert_eq!(th.src.as_deref(), Some("https://t.example/s.png"));
    assert_eq!(th.alt.as_deref(), Some("book cover"));
    assert_eq!(th.width, Some(320));

    assert_eq!(item.icons.len(), 1);
    assert_eq!(item.icons[0].href, "https://i.example/32.png");
}

/// The forward-compatibility contract: Brave adding a field — at any level —
/// must never break a Rover release already in the field.
#[tokio::test]
async fn unexpected_provider_fields_do_not_break_parsing() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        serde_json::json!({
            "type": "search",
            "brand_new_vertical": {"results": [{"x": 1}]},
            "query": {"original": "q", "brand_new_query_field": {"nested": true}},
            "web": {
                "brand_new_web_field": 42,
                "results": [{
                    "title": "T", "url": "https://a.example/",
                    "brand_new_result_field": ["a", "b"],
                    "another_new_one": {"deep": {"deeper": 1}}
                }]
            }
        }),
    )
    .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_FORWARD");

    let r = svc
        .search_with("q", SearchOverrides::default())
        .await
        .expect("unknown fields must not fail the parse");
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].title, "T");
}

/// The other half of the forward-compatibility contract: a field whose
/// *type* changes upstream must cost that field, not the whole search.
/// `#[serde(default)]` only covers a key going missing; every wire field is
/// additionally `deserialize_with = "lenient"` so a reshaped one degrades to
/// absent. Without that, a single upstream change turns every search into
/// `search_malformed_response`.
#[tokio::test]
async fn drifted_provider_field_types_do_not_break_parsing() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        serde_json::json!({
            "query": {
                "original": "q",
                // Object → bare string.
                "language": "en",
                // Bool → string.
                "more_results_available": "yes",
                // Array of strings → array of objects.
                "related_queries": [{"q": "other"}],
            },
            "web": {"results": [
                {
                    "title": "Kept",
                    "url": "https://a.example/",
                    // Object → string.
                    "meta_url": "https://a.example/",
                    // Array → object.
                    "extra_snippets": {"0": "nope"},
                    // Number → string.
                    "fetched_content_timestamp": "1717200000",
                    // Well-formed neighbours survive alongside the drift.
                    "description": "still here",
                    "language": "en",
                },
                // A whole element of the wrong shape is dropped, not fatal.
                "a bare string where a result object belongs",
                {"title": "Also kept", "url": "https://b.example/"},
            ]}
        }),
    )
    .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_DRIFT");

    let r = svc
        .search_with("q", SearchOverrides::default())
        .await
        .expect("a reshaped field must not fail the whole response");

    // The two well-shaped results survive; the bare string is dropped.
    assert_eq!(r.results.len(), 2);
    assert_eq!(r.results[0].title, "Kept");
    assert_eq!(r.results[1].title, "Also kept");
    // Ranks stay contiguous over the dropped element.
    assert_eq!(r.results[0].rank, 1);
    assert_eq!(r.results[1].rank, 2);

    // Fields that drifted read as absent...
    assert!(r.results[0].source.is_none());
    assert!(r.results[0].extra_snippets.is_empty());
    assert!(r.results[0].fetched_content_timestamp.is_none());
    assert!(r.query.language.is_none());
    assert_eq!(r.query.more_results_available, None);
    assert!(r.query.related_queries.is_empty());

    // ...while their well-formed neighbours are untouched.
    assert_eq!(r.results[0].description.as_deref(), Some("still here"));
    assert_eq!(r.results[0].language.as_deref(), Some("en"));
    assert_eq!(r.query.original, "q");
}

#[tokio::test]
async fn enrichment_is_opt_in_and_carries_unmodelled_structure() {
    let body = serde_json::json!({
        "query": {"original": "q"},
        "web": {"results": [{
            "title": "T", "url": "https://a.example/",
            "type": "search_result",
            "article": {"author": [{"name": "A"}], "date": "2026-01-01"},
            "rating": {"ratingValue": 4.5, "bestRating": 5},
            "product": {"price": "9.99"},
            "schemas": [{"@type": "Article"}]
        }]}
    });

    // Off by default: the response stays lean.
    let server = MockServer::start().await;
    mount_ok(&server, body.clone()).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_ENRICH_OFF");
    let r = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap();
    assert!(r.results[0].enrichment.is_none());
    // The typed projection still happens.
    assert_eq!(r.results[0].schema_types, vec!["Article"]);

    // On request: everything Rover does not normalise is preserved verbatim.
    let server = MockServer::start().await;
    mount_ok(&server, body).await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_ENRICH_ON");
    let r = svc
        .search_with(
            "q",
            SearchOverrides {
                enrichment: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let e = r.results[0].enrichment.as_ref().expect("enrichment");
    assert_eq!(e["article"]["date"], "2026-01-01");
    assert_eq!(e["rating"]["ratingValue"], 4.5);
    assert_eq!(e["product"]["price"], "9.99");
    assert!(e.get("schemas").is_some());
    // Fields Rover already types, and provider bookkeeping, stay out.
    assert!(e.get("title").is_none(), "{e}");
    assert!(e.get("type").is_none(), "{e}");
}

#[tokio::test]
async fn an_empty_result_set_is_success_not_an_error() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        serde_json::json!({"query": {"original": "q"}, "web": {"results": []}}),
    )
    .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_EMPTY");
    let r = svc
        .search_with("q", SearchOverrides::default())
        .await
        .expect("empty results are a valid answer");
    assert!(r.results.is_empty());
}

// ----------------------------------------------------------------- errors

#[tokio::test]
async fn http_failures_map_onto_typed_errors() {
    struct Case {
        status: u16,
        body: &'static str,
        label: &'static str,
    }
    let cases = [
        Case {
            status: 401,
            body: r#"{"error":{"code":"SUBSCRIPTION_TOKEN_INVALID","detail":"bad token"}}"#,
            label: "auth",
        },
        Case {
            status: 403,
            body: r#"{"error":{"code":"PLAN_NOT_ALLOWED","detail":"upgrade required"}}"#,
            label: "subscription",
        },
        Case {
            status: 422,
            body: r#"{"error":{"code":"VALIDATION","detail":"count out of range"}}"#,
            label: "validation",
        },
        Case {
            status: 422,
            body: r#"{"error":{"code":"QUOTA_LIMITED","detail":"monthly quota reached"}}"#,
            label: "quota",
        },
        Case {
            status: 500,
            body: r#"{"error":{"code":"INTERNAL","detail":"boom"}}"#,
            label: "upstream",
        },
    ];
    for c in cases {
        let server = MockServer::start().await;
        mount_status(&server, c.status, c.body).await;
        let svc = service(&server, "ROVER_TEST_BRAVE_KEY_ERRORS");
        let e = svc
            .search_with("q", SearchOverrides::default())
            .await
            .expect_err(c.label);
        let ok = match c.label {
            "auth" => matches!(e, SearchError::AuthFailed { .. }),
            "subscription" => matches!(e, SearchError::SubscriptionDenied { .. }),
            "validation" => matches!(e, SearchError::InvalidRequest(_)),
            "quota" => matches!(e, SearchError::QuotaExhausted { .. }),
            "upstream" => matches!(e, SearchError::Upstream { .. }),
            _ => unreachable!(),
        };
        assert!(ok, "{}: got {e:?}", c.label);
        // The credential never appears in an error the caller can see.
        assert!(!e.to_string().contains(KEY), "{}: {e}", c.label);
        assert!(!format!("{e:?}").contains(KEY), "{}: {e:?}", c.label);
    }
}

#[tokio::test]
async fn rate_limiting_surfaces_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_string(r#"{"error":{"code":"RATE_LIMITED"}}"#),
        )
        .mount(&server)
        .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_429");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            SearchError::RateLimited {
                retry_after_secs: Some(7)
            }
        ),
        "{e:?}"
    );
}

#[tokio::test]
async fn malformed_json_is_reported_as_such_not_as_an_empty_result_set() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/json")
                .set_body_string("{not json at all"),
        )
        .mount(&server)
        .await;
    let svc = service(&server, "ROVER_TEST_BRAVE_KEY_MALFORMED");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(matches!(e, SearchError::MalformedResponse(_)), "{e:?}");
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_network_error() {
    // Port 1 on loopback: nothing listens there.
    // SAFETY: a variable unique to this test.
    unsafe { std::env::set_var("ROVER_TEST_BRAVE_KEY_DEAD", KEY) };
    let cfg = SearchConfig {
        api_key_env: "ROVER_TEST_BRAVE_KEY_DEAD".into(),
        base_url: "http://127.0.0.1:1/res/v1/web/search".into(),
        max_retries: 0,
        ..Default::default()
    };
    let svc = SearchService::new(&cfg, "rover-test/0");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(matches!(e, SearchError::Network { .. }), "{e:?}");
}

#[tokio::test]
async fn a_slow_provider_times_out_rather_than_hanging() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(ok_body(minimal_response()).set_delay(std::time::Duration::from_secs(3)))
        .mount(&server)
        .await;
    // SAFETY: a variable unique to this test.
    unsafe { std::env::set_var("ROVER_TEST_BRAVE_KEY_TIMEOUT", KEY) };
    let cfg = SearchConfig {
        api_key_env: "ROVER_TEST_BRAVE_KEY_TIMEOUT".into(),
        base_url: format!("{}/res/v1/web/search", server.uri()),
        timeout_secs: 1,
        max_retries: 0,
        ..Default::default()
    };
    let svc = SearchService::new(&cfg, "rover-test/0");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(matches!(e, SearchError::Timeout { secs: 1 }), "{e:?}");
}

// ------------------------------------------------------------------ retry

/// A retryable failure is retried, and the loop is bounded by
/// `[search] max_retries`. This is the cost-control guarantee: an
/// unbounded loop against a metered API multiplies the bill.
#[tokio::test]
async fn retries_are_bounded_by_max_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("Content-Type", "application/json")
                .set_body_string(r#"{"error":{"code":"INTERNAL"}}"#),
        )
        .mount(&server)
        .await;
    // SAFETY: a variable unique to this test.
    unsafe { std::env::set_var("ROVER_TEST_BRAVE_KEY_RETRY", KEY) };
    let cfg = SearchConfig {
        api_key_env: "ROVER_TEST_BRAVE_KEY_RETRY".into(),
        base_url: format!("{}/res/v1/web/search", server.uri()),
        max_retries: 2,
        ..Default::default()
    };
    let svc = SearchService::new(&cfg, "rover-test/0");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(matches!(e, SearchError::Upstream { .. }), "{e:?}");
    // 1 initial attempt + 2 retries. Never more.
    let n = server.received_requests().await.unwrap().len();
    assert_eq!(n, 3, "expected exactly 3 billable requests, got {n}");
}

/// Terminal failures must not be retried at all: a bad key or a bad
/// argument costs exactly one request, not `max_retries + 1`.
#[tokio::test]
async fn terminal_failures_are_never_retried() {
    for (status, body) in [
        (401, r#"{"error":{"code":"SUBSCRIPTION_TOKEN_INVALID"}}"#),
        (422, r#"{"error":{"code":"VALIDATION"}}"#),
        (422, r#"{"error":{"code":"QUOTA_LIMITED"}}"#),
    ] {
        let server = MockServer::start().await;
        mount_status(&server, status, body).await;
        // SAFETY: a variable unique to this test.
        unsafe { std::env::set_var("ROVER_TEST_BRAVE_KEY_NORETRY", KEY) };
        let cfg = SearchConfig {
            api_key_env: "ROVER_TEST_BRAVE_KEY_NORETRY".into(),
            base_url: format!("{}/res/v1/web/search", server.uri()),
            max_retries: 3,
            ..Default::default()
        };
        let svc = SearchService::new(&cfg, "rover-test/0");
        let _ = svc.search_with("q", SearchOverrides::default()).await;
        let n = server.received_requests().await.unwrap().len();
        assert_eq!(n, 1, "status {status} {body}: expected 1 request, got {n}");
    }
}

#[tokio::test]
async fn a_retry_that_succeeds_returns_the_result() {
    let server = MockServer::start().await;
    // First call 503, then success. `up_to_n_times` + `mount_as_scoped`
    // ordering: wiremock matches mocks in registration order.
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string("{}"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_ok(&server, minimal_response()).await;

    // SAFETY: a variable unique to this test.
    unsafe { std::env::set_var("ROVER_TEST_BRAVE_KEY_RECOVER", KEY) };
    let cfg = SearchConfig {
        api_key_env: "ROVER_TEST_BRAVE_KEY_RECOVER".into(),
        base_url: format!("{}/res/v1/web/search", server.uri()),
        max_retries: 2,
        ..Default::default()
    };
    let svc = SearchService::new(&cfg, "rover-test/0");
    let r = svc
        .search_with("rust async trait", SearchOverrides::default())
        .await
        .expect("second attempt should succeed");
    assert_eq!(r.results.len(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

// --------------------------------------------------------- configuration

#[tokio::test]
async fn a_missing_credential_fails_before_any_request() {
    let server = MockServer::start().await;
    // No mock mounted: a request would fail the test.
    // SAFETY: a variable unique to this test.
    unsafe { std::env::remove_var("ROVER_TEST_BRAVE_KEY_ABSENT") };
    let cfg = SearchConfig {
        api_key_env: "ROVER_TEST_BRAVE_KEY_ABSENT".into(),
        base_url: format!("{}/res/v1/web/search", server.uri()),
        ..Default::default()
    };
    let svc = SearchService::new(&cfg, "rover-test/0");
    let e = svc
        .search_with("q", SearchOverrides::default())
        .await
        .unwrap_err();
    assert!(matches!(e, SearchError::NotConfigured { .. }), "{e:?}");
    assert!(e.to_string().contains("ROVER_TEST_BRAVE_KEY_ABSENT"), "{e}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

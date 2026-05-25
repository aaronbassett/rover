//! Integration tests for the M5 robots gate.
//!
//! The production code fetches `https://{host}/robots.txt`, which does not
//! include the wiremock server's port. That means a robots.txt route mounted
//! on wiremock cannot be reached by `ensure_entry`. Instead, we exercise the
//! gate by pre-seeding `storage::robots` with a fresh entry in the desired
//! state and then invoking `fetch_with_cache`: the cache hit short-circuits
//! `ensure_entry` and the gate's `evaluate()` runs against our seeded body.
//!
//! The 404 / 5xx classification paths (allow_all / disallow_all sentinels)
//! are covered by the unit tests in `src/fetcher/robots.rs`. Here we verify
//! the wiring: the gate calls `evaluate`, returns the right `FetcherError`
//! variant, and respects the `ignore_robots` flag.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::FetcherError;
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::robots::{self, RobotsEntry, RobotsState};
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_ok() -> impl FnMut(&str, &Url) -> Result<ExtractResult, FetcherError> {
    move |_b: &str, _u: &Url| {
        Ok(ExtractResult {
            title: None,
            body_md: "ok".into(),
            content_hash: "sha256:0".into(),
            metadata: rover::extractor::ExtractedMetadata::default(),
        })
    }
}

#[allow(clippy::type_complexity)]
async fn rig() -> (MockServer, Db, Arc<Pacer>, reqwest::Client, RateLimitConfig) {
    let server = MockServer::start().await;
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    std::mem::forget(tmp);
    let rate = RateLimitConfig::default();
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    (server, db, pacer, client, rate)
}

/// Seed a fresh robots_cache row for `host` so the gate hits the cache and
/// does not attempt to fetch robots.txt over the network.
async fn seed_robots(db: &Db, host: &str, body: Option<String>, state: RobotsState) {
    let now = Timestamp::now().as_second();
    robots::upsert(
        db,
        RobotsEntry {
            host: host.to_string(),
            body,
            fetched_at: now,
            expires_at: now + 3600,
            state,
        },
    )
    .await
    .expect("seed robots_cache");
}

#[tokio::test]
async fn robots_disallow_admin_refuses_fetch() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/admin/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>nope</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/admin/x", server.uri())).unwrap();
    seed_robots(
        &db,
        url.host_str().unwrap(),
        Some("User-agent: *\nDisallow: /admin/\n".into()),
        RobotsState::Parsed,
    )
    .await;

    let robots_cfg = RobotsConfig::default();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots_cfg,
        &url,
        &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .expect_err("disallowed path must error");
    assert!(
        matches!(err, FetcherError::RobotsDisallowed { .. }),
        "expected RobotsDisallowed, got {err:?}"
    );
    // The HTTP route must not have been called.
    let received = server.received_requests().await.unwrap_or_default();
    assert!(
        received.is_empty(),
        "robots gate should refuse before any HTTP request"
    );
}

#[tokio::test]
async fn robots_allow_all_lets_fetch_proceed() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/anything"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>hi</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/anything", server.uri())).unwrap();
    // Equivalent to the cached state produced by a 404 on robots.txt.
    seed_robots(&db, url.host_str().unwrap(), None, RobotsState::AllowAll).await;

    let robots_cfg = RobotsConfig::default();
    let cf = Config::default();
    fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots_cfg,
        &url,
        &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .expect("allow-all should let fetch proceed");
    // Confirm the cached row stayed AllowAll.
    let entry = robots::lookup(&db, url.host_str().unwrap())
        .await
        .unwrap()
        .expect("seeded entry present");
    assert_eq!(entry.state, RobotsState::AllowAll);
    assert!(entry.body.is_none());
}

#[tokio::test]
async fn robots_disallow_all_refuses_fetch() {
    let (_server, db, pacer, client, rate) = rig().await;
    // Use a URL whose host has a seeded DisallowAll row. The gate runs
    // before any HTTP traffic, so the route doesn't have to exist.
    let url = Url::parse("http://127.0.0.1:1/anything").unwrap();
    seed_robots(&db, url.host_str().unwrap(), None, RobotsState::DisallowAll).await;

    let robots_cfg = RobotsConfig::default();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots_cfg,
        &url,
        &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: false,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .expect_err("disallow-all sentinel must refuse");
    match err {
        FetcherError::RobotsDisallowed { url: u, .. } => assert_eq!(u, url.as_str()),
        other => panic!("expected RobotsDisallowed, got {other:?}"),
    }
    let entry = robots::lookup(&db, url.host_str().unwrap())
        .await
        .unwrap()
        .expect("seeded entry present");
    assert_eq!(entry.state, RobotsState::DisallowAll);
}

#[test]
fn robots_fetch_failed_display_renders_inner_cause() {
    use rover::fetcher::FetcherError;
    let inner = FetcherError::Decode;
    let outer = FetcherError::RobotsFetchFailed {
        host: "example.com".to_string(),
        source: Box::new(inner),
    };
    let rendered = outer.to_string();
    assert!(
        rendered.contains("response decoding failed"),
        "expected inner Decode error in {rendered}",
    );
    assert!(rendered.contains("example.com"));
}

#[tokio::test]
async fn ignore_robots_flag_skips_gate() {
    let (server, db, pacer, client, rate) = rig().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>x</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    // robots.txt would disallow if consulted.
    seed_robots(
        &db,
        url.host_str().unwrap(),
        Some("User-agent: *\nDisallow: /\n".into()),
        RobotsState::Parsed,
    )
    .await;

    let robots_cfg = RobotsConfig::default();
    let cf = Config::default();
    let result = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots_cfg,
        &url,
        &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_ok(),
    )
    .await
    .expect("ignore_robots: true should bypass gate");
    assert_eq!(result.page.title, None);
}

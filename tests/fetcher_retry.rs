//! Integration tests for the M5 retry loop: 429 / 5xx → backoff + retry,
//! exhaustion → `RetryExhausted`, 4xx other than 429 → no retry.
//!
//! Robots respect is disabled because wiremock cannot satisfy the HTTPS
//! robots.txt URL the production code builds. Robots-gate behaviour is
//! exercised in `tests/fetcher_robots.rs`.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::FetcherError;
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_stub() -> impl FnMut(&str, &Url) -> Result<ExtractResult, FetcherError> {
    move |_body: &str, _base: &Url| {
        Ok(ExtractResult {
            title: None,
            body_md: "ok".into(),
            content_hash: "sha256:0".into(),
            metadata: rover::extractor::ExtractedMetadata::default(),
        })
    }
}

#[allow(clippy::type_complexity)]
async fn rig() -> (
    MockServer,
    Db,
    Arc<Pacer>,
    reqwest::Client,
    RateLimitConfig,
    RobotsConfig,
) {
    let server = MockServer::start().await;
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    std::mem::forget(tmp);
    let rate = RateLimitConfig {
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
        requests_per_minute_per_domain: 6000,
        ..RateLimitConfig::default()
    };
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let robots = RobotsConfig {
        respect: false,
        ..RobotsConfig::default()
    };
    (server, db, pacer, client, rate, robots)
}

fn opts() -> FetchOptions {
    FetchOptions {
        force_refresh: true,
        ssrf_level: SsrfLevel::Loopback,
        ssrf_project_root: None,
        har_recorder: None,
        ignore_robots: true,
        user_agent: "test/0.1".into(),
    }
}

#[tokio::test]
async fn http_429_with_retry_after_succeeds_on_retry() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    // First response: 429 with Retry-After: 0; second response: 200.
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>ok</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    let cf = Config::default();
    let result = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url,
        &cf.cache,
        opts(),
        extract_stub(),
    )
    .await
    .expect("retry should succeed");
    assert_eq!(result.page.title, None);
}

#[tokio::test]
async fn http_500_retries_exhaust_yields_retry_exhausted() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    Mock::given(method("GET"))
        .and(path("/y"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/y", server.uri())).unwrap();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url,
        &cf.cache,
        opts(),
        extract_stub(),
    )
    .await
    .expect_err("should not succeed after exhausting retries");
    match err {
        FetcherError::RetryExhausted { attempts, .. } => {
            // 3 retries on top of the initial attempt == 4 total.
            assert_eq!(attempts, 4, "expected 4 total attempts");
        }
        other => panic!("expected RetryExhausted, got {other:?}"),
    }
}

#[tokio::test]
async fn http_404_is_not_retried() {
    let (server, db, pacer, client, rate, robots) = rig().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let url = Url::parse(&format!("{}/missing", server.uri())).unwrap();
    let cf = Config::default();
    let err = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url,
        &cf.cache,
        opts(),
        extract_stub(),
    )
    .await
    .expect_err("404 should propagate immediately");
    assert!(
        matches!(err, FetcherError::Status { status: 404, .. }),
        "expected Status {{ status: 404, .. }}, got {err:?}"
    );
    // Sanity: exactly one request was issued.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(received.len(), 1, "404 must not retry");
}

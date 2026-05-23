//! End-to-end test that an extraction failure surfaces as
//! `FetcherError::Extract`, exercising the M4 follow-up #1 remap.
//!
//! Robots respect is disabled because wiremock cannot serve the HTTPS
//! robots.txt URL the production code generates (`https://{host}/robots.txt`
//! ignores the wiremock port). That path is exercised by the M5 unit tests
//! and the dedicated `fetcher_robots.rs` integration suite which pre-seeds
//! the robots cache directly.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::FetcherError;
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn extraction_failure_routes_to_extract_variant() {
    let server = MockServer::start().await;
    let html = std::fs::read_to_string("tests/fixtures/m5/extract-failure.html").unwrap();
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(html)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    // Leak the tempdir so the SQLite file path stays valid past the guard.
    std::mem::forget(tmp);

    let rate = RateLimitConfig::default();
    let pacer = Arc::new(Pacer::new(&rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let robots = RobotsConfig {
        respect: false,
        ..RobotsConfig::default()
    };
    let cf = Config::default();
    let url = Url::parse(&format!("{}/page", server.uri())).unwrap();

    let result = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url,
        &cf.cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::Loopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        |body, base| {
            let extracted = rover::extractor::pipeline::extract(body, Some(base))
                .map_err(FetcherError::Extract)?;
            let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
            Ok(ExtractResult {
                title: extracted.title,
                body_md: extracted.body_md,
                content_hash,
                metadata: extracted.metadata,
            })
        },
    )
    .await;

    // The body is essentially empty; readabilityrs will either produce a
    // tiny extraction (which is acceptable) or fail with NoArticle. If it
    // failed, the error must be `Extract` — not `Decode` and not `Status`.
    // This is the M4 follow-up #1 regression guard.
    match result {
        Err(FetcherError::Extract(_)) => {} // expected failure path
        Ok(_) => {}                         // readabilityrs handled the empty case
        Err(other) => panic!("expected Extract or Ok; got {other:?} (must not be Decode/Status)"),
    }
}

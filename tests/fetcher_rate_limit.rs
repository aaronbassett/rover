//! Integration tests for the M5 rate limiter, layered concurrency, and the
//! per-host token bucket. All paths use `--features test-loopback`.
//!
//! Robots respect is disabled in every test because wiremock cannot serve
//! the HTTPS robots.txt URL the production code generates
//! (`https://{host}/robots.txt` does not include the wiremock port). The
//! robots gate has dedicated coverage in `tests/fetcher_robots.rs`.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rover::config::{Config, RateLimitConfig, RobotsConfig};
use rover::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn extract_stub() -> impl FnMut(&str, &Url) -> Result<ExtractResult, rover::fetcher::FetcherError> {
    move |_body: &str, _base: &Url| {
        Ok(ExtractResult {
            title: Some("t".into()),
            body_md: "# t".into(),
            content_hash: "sha256:0".into(),
            metadata: rover::extractor::ExtractedMetadata::default(),
        })
    }
}

async fn setup(rate: &RateLimitConfig) -> (Db, Arc<Pacer>, reqwest::Client) {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    // Leak the tempdir so the on-disk SQLite file remains valid for the
    // duration of the test.
    std::mem::forget(tmp);
    let pacer = Arc::new(Pacer::new(rate));
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    (db, pacer, client)
}

fn robots_off() -> RobotsConfig {
    RobotsConfig {
        respect: false,
        ..RobotsConfig::default()
    }
}

#[tokio::test]
async fn pacing_at_60_rpm_paces_consecutive_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>hi</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    // rpm=60 means one token per second on average after the initial burst
    // is consumed. Burning the burst on the first 60 requests and then
    // issuing a 61st should force the governor to wait at least one
    // replenishment interval (~1s) before letting the request through.
    let rate = RateLimitConfig {
        requests_per_minute_per_domain: 60,
        per_domain_concurrency: 32,
        global_concurrency: 32,
        ..RateLimitConfig::default()
    };
    let (db, pacer, client) = setup(&rate).await;
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    let robots = robots_off();

    let start = Instant::now();
    for _ in 0..61 {
        fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate,
            &robots,
            &url,
            &Config::default().cache,
            FetchOptions {
                force_refresh: true,
                ssrf_level: SsrfLevel::TestLoopback,
                ignore_robots: true,
                user_agent: "test/0.1".into(),
            },
            extract_stub(),
        )
        .await
        .expect("fetch ok");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(500),
        "61 sequential requests at 60 rpm should pace; elapsed = {elapsed:?}"
    );
}

#[tokio::test]
async fn per_host_isolation_does_not_pace_other_hosts() {
    // Two mock servers — different ports == different hosts from the
    // per-host limiter's POV. Burning host A's burst should not slow B at
    // all because the token bucket is keyed on (host, port).
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    for s in [&server_a, &server_b] {
        Mock::given(method("GET"))
            .and(path("/p"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body>hi</body></html>")
                    .insert_header("content-type", "text/html; charset=utf-8"),
            )
            .mount(s)
            .await;
    }

    let rate = RateLimitConfig {
        requests_per_minute_per_domain: 60,
        per_domain_concurrency: 32,
        global_concurrency: 32,
        ..RateLimitConfig::default()
    };
    let (db, pacer, client) = setup(&rate).await;
    let robots = robots_off();

    // Burn host A's burst. The per-host token bucket keys on
    // `Url::host_str()`, so two wiremock servers both bound to 127.0.0.1
    // would collide on the same key (the port is not part of the key).
    // We rewrite host A to `localhost` (still loopback per SSRF, distinct
    // string per limiter) to give the test two genuinely independent
    // host keys.
    let port_a = server_a.address().port();
    let url_a = Url::parse(&format!("http://localhost:{port_a}/p")).unwrap();
    for _ in 0..60 {
        fetch_with_cache(
            &db,
            &client,
            &pacer,
            &rate,
            &robots,
            &url_a,
            &Config::default().cache,
            FetchOptions {
                force_refresh: true,
                ssrf_level: SsrfLevel::TestLoopback,
                ignore_robots: true,
                user_agent: "test/0.1".into(),
            },
            extract_stub(),
        )
        .await
        .expect("fetch ok");
    }

    // Host B: untouched bucket, should be quick.
    let url_b = Url::parse(&format!("{}/p", server_b.uri())).unwrap();
    let start = Instant::now();
    fetch_with_cache(
        &db,
        &client,
        &pacer,
        &rate,
        &robots,
        &url_b,
        &Config::default().cache,
        FetchOptions {
            force_refresh: true,
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
        },
        extract_stub(),
    )
    .await
    .expect("fetch ok");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "host B should not be paced by host A; elapsed = {elapsed:?}"
    );
}

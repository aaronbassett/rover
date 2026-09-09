//! `HeadlessMode::Auto` must degrade, not fail, when the renderer cannot
//! launch.
//!
//! The documented contract (`site/docs/dynamic-pages.md`) is that "auto only
//! ever promised to render if it could". The container target makes the
//! could-not case routine rather than exotic: run `runtime-headless` without
//! `--security-opt seccomp=chrome.json` and every launch returns
//! `HeadlessError::SandboxUnavailable`.
//!
//! Before the fix, `fetch_with_cache` propagated that error with `?`, throwing
//! away a plain-HTTP fetch and extraction that had already succeeded — so a
//! missing seccomp profile turned every SPA-shaped page into a hard error
//! instead of an unrendered one. The bot-challenge path in the same function
//! already handled the identical error the other way (catch, warn, degrade).
//!
//! No browser ever runs here, so this is not the refuse-to-render property
//! being weakened: that property is about launching a browser without a
//! sandbox, and it is untouched.
//!
//! A launch failure is forced by pointing `chrome_executable` at a path that
//! does not exist, so the test needs no browser and is deterministic.

#![cfg(all(feature = "headless", feature = "test-loopback"))]

use std::sync::Arc;
use std::time::Duration;

use rover::config::{Config, HeadlessConfig, RateLimitConfig, RobotsConfig};
use rover::fetcher::FetcherError;
use rover::fetcher::cached::{
    CachedFetch, ExtractResult, FetchOptions, HeadlessMode, fetch_with_cache, sha256_hex,
};
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::headless::HeadlessHandle;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use tempfile::tempdir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Trips `detect_spa` on at least two heuristics: an `id="root"` SPA marker
/// and a sub-300-character extraction. Body text is kept deliberately thin so
/// the extraction stays short.
const SPA_HTML: &str = r#"<!doctype html>
<html><head><title>App</title></head>
<body>
  <div id="root"></div>
  <script>window.__BOOT__ = 1; document.getElementById('root').textContent = 'rendered';</script>
</body></html>"#;

/// A handle whose `get()` can only ever fail: the configured executable does
/// not exist, so chromiumoxide's spawn fails before any browser process runs.
fn unlaunchable_handle() -> HeadlessHandle {
    HeadlessHandle::new(HeadlessConfig {
        chrome_executable: "/nonexistent/rover-test-no-such-browser".into(),
        // Keep the Auto-mode pre-render pause out of the test's runtime.
        launch_delay_secs: 0,
        ..HeadlessConfig::default()
    })
}

async fn fetch_spa_page_with_unlaunchable_renderer() -> Result<CachedFetch, FetcherError> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/app"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SPA_HTML)
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
    let url = Url::parse(&format!("{}/app", server.uri())).unwrap();

    fetch_with_cache(
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
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: true,
            user_agent: "test/0.1".into(),
            headless: Some(unlaunchable_handle()),
            headless_mode: HeadlessMode::Auto,
            synchronous_revalidation: true,
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
    .await
}

/// The regression guard. With `?` restored on the `h.get()` call this returns
/// `Err(FetcherError::Headless(LaunchFailed(..)))` and the assertion below
/// fails — the mutation test that proves this test is load-bearing.
#[tokio::test]
async fn auto_mode_degrades_to_the_http_result_when_the_renderer_cannot_launch() {
    let result = fetch_spa_page_with_unlaunchable_renderer().await;

    let cached = match result {
        Ok(c) => c,
        Err(e) => panic!(
            "auto mode discarded a successful HTTP fetch because the renderer \
             would not launch; expected a degraded result, got: {e}"
        ),
    };

    // The returned page is the plain-HTTP extraction, not a render.
    assert_eq!(
        cached.page.render_reason, None,
        "no browser ran, so the row must not claim it was rendered: {:?}",
        cached.page.render_reason
    );
    assert!(
        cached.page.title.as_deref() == Some("App"),
        "expected the HTTP-fetched document, got title {:?}",
        cached.page.title
    );
}

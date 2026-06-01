//! Headless smoketests. Require a real Chrome/Chromium installed locally.
//! `#[ignore]` by default; opt in via `cargo test --features headless -- --ignored`.

#![cfg(feature = "headless")]

use rover::config::HeadlessConfig;
use rover::fetcher::headless::HeadlessRenderer;
use rover::fetcher::ssrf::SsrfLevel;

use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg() -> HeadlessConfig {
    HeadlessConfig {
        timeout_secs: 10,
        ..HeadlessConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn renders_static_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            // Serve as text/html: wiremock's `set_body_string` defaults to
            // `text/plain`, which Chrome renders as a literal `<pre>` dump and
            // never executes — so the headless render wouldn't exercise the
            // real HTML/JS pipeline these tests exist to cover.
            ResponseTemplate::new(200).set_body_raw(
                "<html><body><h1>hello</h1></body></html>".as_bytes(),
                "text/html",
            ),
        )
        .mount(&server)
        .await;
    let renderer = HeadlessRenderer::new(&cfg()).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer
        .render(&url, SsrfLevel::Loopback, None)
        .await
        .expect("render");
    assert!(rendered.html.contains("hello"));
    renderer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn auto_mode_triggers_on_short_extraction() {
    // Serve an SPA shell that extracts to almost nothing.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<html><head></head><body><div id="root"></div><script>document.getElementById('root').innerText='hydrated content'</script></body></html>"#.as_bytes(),
            "text/html",
        ))
        .mount(&server).await;
    let renderer = HeadlessRenderer::new(&cfg()).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer
        .render(&url, SsrfLevel::Loopback, None)
        .await
        .expect("render");
    // After JS execution, the page text should contain "hydrated content".
    assert!(rendered.html.contains("hydrated content"));
    renderer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn block_list_fulfills_not_aborts() {
    // Serve a page that references a font URL. Assert the page renders
    // (no font-load error) even though the font request is blocked.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<html><head><link rel="stylesheet" href="/styles.css"></head><body>OK</body></html>"#.as_bytes(),
            "text/html",
        ))
        .mount(&server).await;
    // No /styles.css mock — the request hits 404 normally, but our intercept
    // turns it into empty 200 before chromiumoxide can fail.
    let mut c = cfg();
    c.block_css = true;
    let renderer = HeadlessRenderer::new(&c).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer
        .render(&url, SsrfLevel::Loopback, None)
        .await
        .expect("render");
    assert!(rendered.html.contains("OK"));
    renderer.shutdown().await;
}

/// `networkidle0` must wait past `domcontentloaded` for in-flight XHRs to
/// finish. The shell injects its real content only after a deliberately
/// slow `/data` fetch resolves; a renderer that stopped at domcontentloaded
/// (the old `sleep(500ms)` approximation, with the XHR delayed well beyond
/// that) — or one using `networkidle2`, whose ≤2 tolerance treats the lone
/// pending XHR as "idle" — would miss it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn networkidle0_waits_for_delayed_xhr() {
    let server = MockServer::start().await;
    // The SPA shell: empty until the XHR to /data resolves.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<html><head></head><body><div id="root"></div>
            <script>
              fetch('/data')
                .then(r => r.text())
                .then(t => { document.getElementById('root').innerHTML = t; });
            </script></body></html>"#
                    .as_bytes(),
                "text/html",
            ),
        )
        .mount(&server)
        .await;
    // The XHR payload, delayed ~1.2s — comfortably past the old 500ms sleep.
    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1200))
                .set_body_string("<p>xhr-loaded-content</p>"),
        )
        .mount(&server)
        .await;

    let mut c = cfg();
    c.default_wait = "networkidle0".to_string();
    let renderer = HeadlessRenderer::new(&c).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer
        .render(&url, SsrfLevel::Loopback, None)
        .await
        .expect("render");
    assert!(
        rendered.html.contains("xhr-loaded-content"),
        "networkidle0 should have waited for the delayed XHR; got: {}",
        rendered.html
    );
    renderer.shutdown().await;
}

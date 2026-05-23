//! Integration tests for the fetch pipeline.

use rover::fetcher::{client::build_http_client, fetch::fetch_url, ssrf::SsrfLevel};
use std::time::Duration;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetches_simple_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><head><title>hi</title></head><body>hi there</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse(&format!("{}/article", server.uri())).unwrap();

    let page = fetch_url(&client, &url, SsrfLevel::Loopback, None)
        .await
        .expect("fetch ok");
    assert_eq!(page.final_url.as_str(), url.as_str());
    assert!(page.body.contains("hi there"));
}

#[tokio::test]
async fn rejects_private_ip_at_ssrf_check() {
    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse("http://10.0.0.1/").unwrap();
    let result = fetch_url(&client, &url, SsrfLevel::Strict, None).await;
    assert!(matches!(result, Err(rover::fetcher::FetcherError::Ssrf(_))));
}

#[tokio::test]
async fn follows_redirects_and_records_final_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>destination</body></html>")
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let start = Url::parse(&format!("{}/redirect", server.uri())).unwrap();
    let page = fetch_url(&client, &start, SsrfLevel::Loopback, None)
        .await
        .expect("fetch ok");
    assert!(page.final_url.path().ends_with("/final"));
    assert_eq!(page.canonical_url.path(), "/final");
}

#[tokio::test]
async fn extracts_canonical_from_html() {
    let server = MockServer::start().await;
    let canonical = format!("{}/canon", server.uri());
    let html = format!(
        r#"<html><head><link rel="canonical" href="{}"></head><body>x</body></html>"#,
        canonical
    );
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(html)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let client = build_http_client("test/0.1", Duration::from_secs(5));
    let url = Url::parse(&format!("{}/page", server.uri())).unwrap();
    let page = fetch_url(&client, &url, SsrfLevel::Loopback, None)
        .await
        .expect("fetch ok");
    assert_eq!(page.canonical_url.as_str(), canonical);
}

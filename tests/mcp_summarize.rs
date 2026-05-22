//! End-to-end MCP test for `summarize`.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

fn html_body() -> &'static str {
    "<html><head><title>Hello</title></head><body>\
     <article>\
     <h1>Hello</h1>\
     <p>The Midnight Network is a privacy-preserving blockchain. It uses zero-knowledge proofs.</p>\
     <p>The native token is NIGHT. STAR is the unit of account for transaction fees.</p>\
     </article>\
     </body></html>"
}

#[tokio::test]
async fn summarize_returns_extractive_output_on_cache_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article", server.uri());
    let mut params = CallToolRequestParams::new("summarize_tool".to_string());
    let args = json!({
        "url": url,
        "mode": "extractive",
        "target_tokens": 50,
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.expect("summarize succeeded");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("\"summary_md\""),
        "missing summary_md: {blob}"
    );
    assert!(
        blob.contains("\"mode\":\"extractive\""),
        "expected mode=extractive: {blob}"
    );
    assert!(
        blob.contains("\"cache_status\":\"miss\""),
        "expected cache_status=miss: {blob}"
    );
    assert!(
        blob.contains("\"backend\":\"default\""),
        "expected backend=default: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn summarize_second_call_hits_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article2", server.uri());
    let args = json!({
        "url": url,
        "mode": "extractive",
    });

    let mut p1 = CallToolRequestParams::new("summarize_tool".to_string());
    if let Some(obj) = args.as_object().cloned() {
        p1 = p1.with_arguments(obj);
    }
    let _ = client.call_tool(p1).await.unwrap();

    let mut p2 = CallToolRequestParams::new("summarize_tool".to_string());
    if let Some(obj) = args.as_object().cloned() {
        p2 = p2.with_arguments(obj);
    }
    let res = client.call_tool(p2).await.unwrap();
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("\"cache_status\":\"hit\""),
        "expected cache_status=hit on second call: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

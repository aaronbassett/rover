//! End-to-end MCP tests for `count_tokens { mode: "estimates" }` (Task 12b).
//!
//! Covers:
//!  - the four-count shape (`extracted_md`, `summary_short`, `summary_medium`,
//!    optional `raw_html`) returned by the `estimates` mode;
//!  - the legacy `mode = "single"` default still returns the historical
//!    single-count shape;
//!  - `mode = "estimates"` rejects `text` input with `invalid_args`.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

const HTML_BODY: &str = "<html><head><title>Estimates Fixture</title></head>\
    <body><article>\
    <h1>Estimates Fixture</h1>\
    <p>Rover counts tokens. The estimates mode returns four numbers in one round-trip.</p>\
    <p>It tokenizes the extracted markdown and produces two summary estimates.</p>\
    <p>One target is around two hundred fifty tokens; the other is around seven hundred fifty.</p>\
    <p>Raw HTML is optional and only present when the cache stores it.</p>\
    </article></body></html>";

async fn start_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(HTML_BODY),
        )
        .mount(&server)
        .await;
    server
}

async fn call_tool(
    client: &RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: serde_json::Value,
) -> rmcp::model::CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_string());
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    client.call_tool(params).await.expect("call_tool")
}

async fn call_tool_any(
    client: &RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: serde_json::Value,
) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
    let mut params = CallToolRequestParams::new(name.to_string());
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    client.call_tool(params).await.map_err(|e| match e {
        rmcp::ServiceError::McpError(data) => data,
        other => panic!("unexpected client error: {other:?}"),
    })
}

/// Helper: extract the inner JSON object that the tool returned.
fn inner_json(res: &rmcp::model::CallToolResult) -> serde_json::Value {
    let blob = serde_json::to_string(res).unwrap();
    let outer: serde_json::Value = serde_json::from_str(&blob).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("tool returned text content block");
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn estimates_mode_returns_four_counts() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(
        &client,
        "count_tokens_tool",
        json!({ "url": server.uri(), "mode": "estimates" }),
    )
    .await;
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let v = inner_json(&res);
    // Estimates shape: `url`, `tokenizer`, `estimates { ... }`.
    assert!(v["url"].is_string(), "missing url: {v}");
    assert!(v["tokenizer"].is_string(), "missing tokenizer: {v}");
    let est = &v["estimates"];
    assert!(est.is_object(), "missing estimates: {v}");
    let extracted = est["extracted_md"].as_u64().expect("extracted_md");
    let short = est["summary_short"].as_u64().expect("summary_short");
    let medium = est["summary_medium"].as_u64().expect("summary_medium");
    assert!(extracted > 0, "extracted_md was 0: {v}");
    assert!(short > 0, "summary_short was 0: {v}");
    assert!(medium > 0, "summary_medium was 0: {v}");
    // Default config has `store_raw_html = false`, so the field is absent or null.
    let raw = &est["raw_html"];
    assert!(raw.is_null() || est.get("raw_html").is_none());

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn single_mode_remains_default_and_unchanged() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(&client, "count_tokens_tool", json!({ "url": server.uri() })).await;
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let v = inner_json(&res);
    // Single shape: top-level `tokens`, no `estimates` object.
    assert!(v["tokens"].as_u64().is_some(), "missing tokens: {v}");
    assert!(v.get("estimates").is_none(), "unexpected estimates: {v}");

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn estimates_mode_rejects_text_input() {
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let result = call_tool_any(
        &client,
        "count_tokens_tool",
        json!({ "text": "hi", "mode": "estimates" }),
    )
    .await;
    let err = result.expect_err("expected invalid_args JSON-RPC error");
    let blob = serde_json::to_string(&err).unwrap();
    assert!(blob.contains("invalid_args"), "blob: {blob}");

    client.cancel().await.unwrap();
}

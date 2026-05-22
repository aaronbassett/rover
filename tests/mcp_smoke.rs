//! End-to-end smoke test for `rover mcp` via an rmcp client + child process.
//!
//! These tests spawn the test-built `rover` binary, speak MCP over stdio,
//! and exercise the `fetch` and `count_tokens` tools against a `wiremock`
//! server. The binary is built with `--features test-loopback` so SSRF
//! allows the wiremock loopback address.

mod common;

use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

const HTML_BODY: &str = "<html><head><title>Sample</title></head>\
                          <body><article><h1>Sample</h1>\
                          <p>Hello world from wiremock.</p></article></body></html>";

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

/// Invoke a tool, capturing either a normal `CallToolResult` or a JSON-RPC
/// `ErrorData` rejection. Rover surfaces structured error envelopes via the
/// `ErrorData.data` payload, so tests need to look on both sides.
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

#[tokio::test]
async fn lists_three_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"fetch_tool"),
        "missing fetch_tool: {names:?}"
    );
    assert!(
        names.contains(&"count_tokens_tool"),
        "missing count_tokens_tool: {names:?}"
    );
    assert!(
        names.contains(&"get_metadata_tool"),
        "missing get_metadata_tool: {names:?}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn fetch_against_wiremock_returns_markdown() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(&client, "fetch_tool", json!({"url": server.uri()})).await;
    assert!(
        !res.is_error.unwrap_or(false),
        "tool returned error: {res:?}"
    );
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("Sample"),
        "expected title in markdown: {blob}"
    );
    assert!(
        blob.contains("cache_status"),
        "expected cache_status: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn count_only_returns_count_envelope() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let res = call_tool(
        &client,
        "fetch_tool",
        json!({"url": server.uri(), "count_only": true}),
    )
    .await;
    assert!(!res.is_error.unwrap_or(false));
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"tokens\""), "missing tokens: {blob}");
    assert!(
        !blob.contains("\"markdown\""),
        "unexpected markdown: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn max_tokens_exceeded_is_structured_error() {
    // Custom server with a single sentence that still exceeds a 1-token
    // budget even after auto-summarize. `max_tokens: 0` is no longer a
    // valid trick (it now returns InvalidArgs), so we ensure the body
    // itself is meaningfully over-budget.
    let server = MockServer::start().await;
    let html = "<html><head><title>Over</title></head><body><article>\
                <p>This sentence alone clearly contains many more than five \
                tokens worth of content for our test budget.</p></article></body></html>";
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html),
        )
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let result = call_tool_any(
        &client,
        "fetch_tool",
        json!({"url": server.uri(), "max_tokens": 1}),
    )
    .await;
    let err = result.expect_err("expected MaxTokensExceeded JSON-RPC error");
    let blob = serde_json::to_string(&err).unwrap();
    assert!(
        blob.contains("max_tokens_exceeded"),
        "expected code in payload: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn count_tokens_with_text_works() {
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;
    let res = call_tool(&client, "count_tokens_tool", json!({"text": "hello world"})).await;
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"tokens\""));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn count_tokens_neither_arg_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let client = spawn_client(tmp.path()).await;
    let result = call_tool_any(&client, "count_tokens_tool", json!({})).await;
    let err = result.expect_err("expected InvalidArgs JSON-RPC error");
    let blob = serde_json::to_string(&err).unwrap();
    assert!(blob.contains("invalid_args"), "blob: {blob}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn second_fetch_reports_cache_hit() {
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let _first = call_tool(&client, "fetch_tool", json!({"url": server.uri()})).await;
    let second = call_tool(&client, "fetch_tool", json!({"url": server.uri()})).await;
    let blob = serde_json::to_string(&second).unwrap();
    assert!(blob.contains("\"hit\""), "expected hit, blob: {blob}");

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn servers_row_is_cleaned_up_on_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let client = spawn_client(tmp.path()).await;
        let _ = client.list_all_tools().await.unwrap();
        client.cancel().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let db = rover::storage::Db::open(tmp.path().join("rover.db"))
        .await
        .unwrap();
    let rows = db.list_servers().await.unwrap();
    assert!(
        rows.is_empty(),
        "expected servers table empty, got {rows:?}"
    );
}

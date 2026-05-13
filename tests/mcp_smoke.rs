//! End-to-end smoke test for `rover mcp` via an rmcp client + child process.
//!
//! These tests spawn the test-built `rover` binary, speak MCP over stdio,
//! and exercise the `fetch` and `count_tokens` tools against a `wiremock`
//! server. The binary is built with `--features test-loopback` so SSRF
//! allows the wiremock loopback address.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::json;
use tokio::process::Command;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn bin_path() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("rover")
}

/// Pre-seed the o200k tokenizer (the config default) inside `data_dir` so the
/// child `rover mcp` process never tries to download from HuggingFace.
fn seed_default_tokenizer(data_dir: &std::path::Path) {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer/tiny.json");
    let dest_dir = data_dir.join("tokenizers").join("o200k");
    std::fs::create_dir_all(&dest_dir).unwrap();
    let dest = dest_dir.join("tokenizer.json");
    std::fs::copy(&fixture, &dest).unwrap();
}

async fn spawn_client(data_dir: &std::path::Path) -> RunningService<rmcp::RoleClient, ()> {
    let mut cmd = Command::new(bin_path());
    cmd.arg("mcp");
    cmd.env("ROVER_DATA_DIR", data_dir);
    cmd.env("ROVER_MCP_SSRF", "test_loopback");
    cmd.env("RUST_LOG", "info,rover=debug");
    let proc = TokioChildProcess::new(cmd).expect("spawn rover mcp");
    ().serve(proc).await.expect("client handshake")
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
async fn lists_two_tools() {
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
    let server = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    // `max_tokens: 0` guarantees the limit is exceeded for any non-empty
    // extracted markdown regardless of the (fixture) tokenizer's vocabulary.
    let result = call_tool_any(
        &client,
        "fetch_tool",
        json!({"url": server.uri(), "max_tokens": 0}),
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

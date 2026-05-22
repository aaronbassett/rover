//! `fetch.max_tokens` auto-summarizes instead of erroring when the
//! extracted body exceeds the budget. Single-shot; if the summary itself
//! is still over the budget, the original `MaxTokensExceeded` error is
//! returned.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

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

fn big_html() -> String {
    let mut s = String::from("<html><head><title>Big</title></head><body><article>");
    for i in 0..40 {
        s.push_str(&format!(
            "<p>Sentence number {i} contains a discrete fact about the test corpus.</p>",
        ));
    }
    s.push_str("</article></body></html>");
    s
}

#[tokio::test]
async fn fetch_max_tokens_triggers_auto_summarize() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(big_html()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/big", server.uri());
    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    let args = json!({"url": url, "max_tokens": 200});
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client
        .call_tool(params)
        .await
        .expect("fetch with max_tokens");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let outer: serde_json::Value = serde_json::to_value(&res).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        v["auto_summarized"], true,
        "expected auto_summarized=true: {v}"
    );
    assert!(
        v.get("summarized").is_none(),
        "summarized should be absent when only the auto path ran: {v}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn fetch_max_tokens_returns_error_when_summary_still_overshoots() {
    let server = MockServer::start().await;
    // A short doc whose single sentence already exceeds the tiny budget.
    let html = "<html><head><title>Over</title></head><body><article>\
                <p>This sentence alone clearly contains many more than five \
                tokens worth of content for our test budget.</p></article></body></html>";
    Mock::given(method("GET"))
        .and(path("/over"))
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

    let url = format!("{}/over", server.uri());
    let result = call_tool_any(&client, "fetch_tool", json!({"url": url, "max_tokens": 5})).await;
    let err = result.expect_err("expected MaxTokensExceeded JSON-RPC error");
    let blob = serde_json::to_string(&err).unwrap();
    assert!(
        blob.contains("max_tokens_exceeded"),
        "expected stable code in payload: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

//! `fetch.summarize` arg produces a real summary instead of being ignored.
//!
//! Task 10 promoted the previously accept-no-op `summarize` field on
//! `FetchArgs` to a typed `InlineSummarizeArgs`. When the agent supplies
//! it, the returned `markdown` is the summary (not the extracted body)
//! and the response envelope carries `summarized: true`.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

fn html() -> &'static str {
    "<html><head><title>Doc</title></head><body><article>\
     <h1>Doc</h1>\
     <p>First sentence here describing the topic in some detail.</p>\
     <p>Second sentence with additional context for the reader.</p>\
     <p>Third sentence wrapping up the introduction nicely.</p>\
     </article></body></html>"
}

#[tokio::test]
async fn fetch_with_summarize_arg_returns_summary_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/p", server.uri());
    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    let args = json!({
        "url": url,
        "summarize": {
            "mode": "extractive",
            "target_tokens": 20,
            "style": "prose"
        }
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.expect("fetch+summarize ok");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let outer: serde_json::Value = serde_json::to_value(&res).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(v["summarized"], true, "expected summarized=true: {v}");
    assert!(
        v.get("auto_summarized").is_none(),
        "auto_summarized should be absent when agent supplied summarize arg: {v}"
    );
    assert!(
        !v["markdown"].as_str().unwrap().is_empty(),
        "summary markdown should be non-empty: {v}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

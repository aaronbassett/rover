//! End-to-end HAR recording via `[debug] har_path`.
#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client_with_config};

#[tokio::test]
async fn fetch_writes_har_entry_when_har_path_set() {
    let server = MockServer::start().await;
    // Readability requires a non-trivial body to detect an article; pad with
    // paragraph text so `fetch` succeeds end-to-end. The HAR recorder runs
    // before extraction either way, but a successful tool call keeps the test
    // signal focused on HAR output rather than extractor edge cases.
    let body = format!(
        "<html><head><title>Test Page</title></head><body>\
         <article><h1>Test Page</h1>{}</article></body></html>",
        "<p>This is a paragraph of test content used to give the article \
         body extractor enough material to detect a real article.</p>"
            .repeat(20),
    );
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let har_path = tmp.path().join("rover.har");
    let cfg = format!(
        r#"
[robots]
respect = false

[ssrf]
level = "loopback"

[debug]
har_path = "{}"
"#,
        har_path.display(),
    );
    let client = spawn_client_with_config(tmp.path(), &cfg).await;

    let url = format!("{}/p", server.uri());
    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    let args = json!({ "url": url });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let _ = client.call_tool(params).await.expect("fetch ok");

    // Cancel the client; serve_stdio's shutdown path flushes the HAR file
    // before returning, so the JSON should be on disk by the time the child
    // exits.
    client.cancel().await.unwrap();
    // Small grace period for the child process to finish its shutdown
    // sequence (final flush + db cleanup).
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    assert!(
        har_path.exists(),
        "expected HAR file at {}",
        har_path.display()
    );
    let text = std::fs::read_to_string(&har_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["log"]["version"], "1.2");
    let entries = parsed["log"]["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "expected at least one HAR entry: {text}"
    );
    assert!(
        entries[0]["request"]["url"]
            .as_str()
            .unwrap()
            .contains("/p"),
        "first entry url should contain /p: {}",
        entries[0]["request"]["url"]
    );
}

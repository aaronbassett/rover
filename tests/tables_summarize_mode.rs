//! End-to-end MCP test for `fetch.tables.mode = "summarize"`.
//!
//! Task 11 wires `TablesMode::Summarize` through `SummarizerService` so a
//! per-table summary replaces each `<table>` block in the response body
//! and is recorded in the rendered frontmatter under `tables_transformed`.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

fn html_with_table() -> &'static str {
    "<html><head><title>Doc</title></head><body><article>\
     <h1>Doc</h1>\
     <p>Lead-in paragraph describing the dataset that follows.</p>\
     <table>\
     <thead><tr><th>Region</th><th>Sales</th></tr></thead>\
     <tbody>\
     <tr><td>North</td><td>12</td></tr>\
     <tr><td>South</td><td>47</td></tr>\
     <tr><td>East</td><td>3</td></tr>\
     <tr><td>West</td><td>88</td></tr>\
     </tbody>\
     </table>\
     <p>Closing paragraph after the table.</p>\
     </article></body></html>"
}

#[tokio::test]
async fn fetch_tables_summarize_records_summary_mode_in_frontmatter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_with_table()),
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
        "tables": { "mode": "summarize" },
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client
        .call_tool(params)
        .await
        .expect("fetch+tables=summarize ok");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let outer: serde_json::Value = serde_json::to_value(&res).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();

    let frontmatter = v["frontmatter"]
        .as_str()
        .expect("frontmatter is a string field");
    assert!(
        frontmatter.contains("tables_transformed:"),
        "expected tables_transformed in frontmatter: {frontmatter}",
    );
    assert!(
        frontmatter.contains("mode: summarize"),
        "expected per-table mode=summarize in frontmatter: {frontmatter}",
    );

    // Frontmatter advertises a single transformed table at ordinal 0.
    assert!(
        frontmatter.contains("ordinal: 0"),
        "expected ordinal: 0 in frontmatter: {frontmatter}",
    );

    // Surrounding prose is preserved verbatim.
    let body = v["markdown"].as_str().expect("markdown body");
    assert!(
        body.contains("Lead-in paragraph"),
        "lead-in paragraph should be preserved: {body}",
    );

    client.cancel().await.unwrap();
    drop(server);
}

//! End-to-end: spawn a real `rover mcp` subprocess, call `batch_fetch` over
//! stdio rmcp, then run `rover batch <id> --monitor` as a second subprocess
//! and assert the ndjson stream ends in `task_completed` and exits zero.
//!
//! This is the M6 acceptance gate: it proves the MCP server hands off a
//! background task whose progress is observable from the CLI in a separate
//! process, which is the user-visible contract for batch_fetch.

#![cfg(feature = "test-loopback")]

use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use tempfile::tempdir;
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

const HTML_BODY: &str = "<html><head><title>T</title></head>\
                          <body><article><h1>T</h1>\
                          <p>Paragraph one with enough text to satisfy readability.</p>\
                          <p>Paragraph two with enough text to satisfy readability.</p>\
                          </article></body></html>";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn monitor_streams_until_terminal_and_exits_zero() {
    // 1. wiremock serves the per-URL pages used by the batch.
    let mock = MockServer::start().await;
    Mock::given(path_regex(r"^/page/\d+$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(HTML_BODY),
        )
        .mount(&mock)
        .await;
    let urls: Vec<String> = (0..3).map(|i| format!("{}/page/{i}", mock.uri())).collect();

    // 2. Spawn `rover mcp` and call batch_fetch via rmcp.
    let data = tempdir().unwrap();
    common::seed_default_tokenizer(data.path());
    let client = common::spawn_client(data.path()).await;

    let mut params = CallToolRequestParams::new("batch_fetch_tool".to_string());
    params = params.with_arguments(
        json!({ "urls": urls, "force_refresh": false })
            .as_object()
            .cloned()
            .unwrap(),
    );
    let result = client
        .call_tool(params)
        .await
        .expect("batch_fetch tool call");

    assert!(
        !result.is_error.unwrap_or(false),
        "batch_fetch returned an error envelope: {result:?}"
    );

    // Extract task_id from the structured payload. The handler returns
    // Json<TaskCreatedResponse>, which rmcp serialises into both
    // `structured_content` and a text content block; we prefer the
    // structured form but fall back to parsing the text block.
    let task_id = result
        .structured_content
        .as_ref()
        .and_then(|v| v.get("task_id"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| {
            result.content.iter().find_map(|c| {
                let text = serde_json::to_string(c).ok()?;
                let v: serde_json::Value = serde_json::from_str(&text).ok()?;
                v.pointer("/raw/text")
                    .and_then(|t| t.as_str())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("task_id").and_then(|t| t.as_str()).map(str::to_owned))
            })
        })
        .unwrap_or_else(|| panic!("could not extract task_id from result: {result:?}"));

    // 3. `rover batch <id> --monitor` as a child process. It exits when
    //    the task reaches a terminal state and closes stdout.
    let output = tokio::task::spawn_blocking({
        let bin = common::bin_path();
        let data_dir = data.path().to_path_buf();
        let cfg = data.path().join("rover.toml");
        let task_id = task_id.clone();
        move || {
            std::process::Command::new(bin)
                .env("ROVER_DATA_DIR", &data_dir)
                .args([
                    "--config",
                    cfg.to_str().unwrap(),
                    "batch",
                    &task_id,
                    "--monitor",
                ])
                .output()
                .expect("spawn rover batch --monitor")
        }
    })
    .await
    .unwrap();

    // Always tear down the rmcp server before asserting, so test failures
    // don't leave a dangling subprocess attached to the data dir.
    let _ = tokio::time::timeout(Duration::from_secs(5), client.cancel()).await;

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        output.status.success(),
        "monitor exited non-zero: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout,
        stderr,
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        !lines.is_empty(),
        "no output from monitor; stderr:\n{stderr}"
    );

    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("line not JSON: {e} (line: {l})"))
        })
        .collect();

    // First event is task_started; final event is task_completed.
    let first_kind = parsed
        .first()
        .and_then(|v| v["kind"].as_str())
        .unwrap_or("");
    let last_kind = parsed.last().and_then(|v| v["kind"].as_str()).unwrap_or("");
    assert_eq!(
        first_kind, "task_started",
        "expected first kind = task_started, got {first_kind} in {parsed:?}"
    );
    assert_eq!(
        last_kind, "task_completed",
        "expected last kind = task_completed, got {last_kind} in {parsed:?}"
    );

    // All three URLs should have produced an item_done.
    let item_done = parsed.iter().filter(|v| v["kind"] == "item_done").count();
    assert_eq!(
        item_done,
        urls.len(),
        "expected {} item_done, got {item_done}",
        urls.len()
    );
}

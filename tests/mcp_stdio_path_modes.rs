//! Regression test: `tables.mode = "csv_file"` and `images.mode =
//! "download"` must keep working over the stdio transport.
//!
//! Task 8 added a guard (`reject_server_path_modes` in
//! `src/mcp/tools/fetch.rs`) that refuses both modes over HTTP, because the
//! absolute server-side paths they write into the response are meaningless
//! to a caller in another container. That guard's very first check is
//! `transport != TransportKind::Http`, so it should never affect the stdio
//! transport — but nothing in the existing suite actually drove either mode
//! through `fetch_tool` over stdio to prove that. (`extractor_tables.rs`
//! calls `extractor::tables::apply` directly, bypassing `fetch_inner`
//! entirely; `tables_summarize_mode.rs` exercises `tables.mode =
//! "summarize"`, not `csv_file`.) This test closes that gap: it drives both
//! modes together through a real `rover mcp` stdio child process (the same
//! `common::spawn_client` helper `tables_summarize_mode.rs` uses) and
//! asserts on the actual server-side paths the response returns, not just
//! on the absence of an error.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

/// Minimal 1x1 transparent PNG. Same bytes used in extractor::images unit tests.
const PROBE_PNG: [u8; 67] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csv_file_and_download_still_work_over_stdio() {
    let server = MockServer::start().await;
    // Both mocks match `method("GET")`, so each also needs a `path()`
    // matcher — otherwise wiremock's first-registered-wins resolution
    // would route the `/probe.png` image request to the HTML mock too,
    // and the "downloaded" bytes would silently be the wrong response.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><body><h1>hi</h1>\
                     <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\
                     <img src=\"/probe.png\" alt=\"probe\"/>\
                     </body></html>",
                ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/probe.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PROBE_PNG.to_vec()))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    // `RoverHandler::fetch_inner` resolves `OutputPaths` from
    // `[output] dir`, which falls back to `ROVER_OUTPUT_DIR` then
    // `paths::data_dir().join("output")`. `spawn_client` sets
    // `ROVER_DATA_DIR` on the child to `tmp.path()`, so table CSVs and
    // downloaded images land under `tmp.path()/output` — asserted below.
    let client = spawn_client(tmp.path()).await;

    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    params = params.with_arguments(
        json!({
            "url": server.uri(),
            "tables": {"mode": "csv_file"},
            "images": {"mode": "download"}
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let result = client
        .call_tool(params)
        .await
        .expect("fetch with csv_file + download must succeed over stdio");

    assert_ne!(
        result.is_error,
        Some(true),
        "csv_file + download must not be refused over stdio, got: {result:?}"
    );

    let outer: serde_json::Value = serde_json::to_value(&result).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    let content = v["content"].as_str().expect("content");

    let output_root = tmp
        .path()
        .join("output")
        .canonicalize()
        .unwrap_or_else(|_| {
            // The output dir is created lazily by `OutputPaths::resolve`, which
            // has definitely run by now, but canonicalize defensively in case
            // of a race on a slow filesystem.
            tmp.path().join("output")
        });

    // --- Table: the response names an absolute, existing CSV path -----
    //
    // The replacement text is `_Table {ordinal} saved to {path}_` — a
    // markdown-emphasis-wrapped line with exactly one leading and one
    // trailing underscore. Extracting via `.find('_')` from the start
    // breaks the moment the path itself contains an underscore (as macOS
    // temp dirs do, e.g. `/private/var/folders/32/_j04f9.../T/...`), so
    // extract by line instead: strip the known prefix, then strip exactly
    // one trailing underscore.
    let saved_marker = "_Table 0 saved to ";
    let csv_line = content
        .lines()
        .find(|l| l.starts_with(saved_marker))
        .unwrap_or_else(|| {
            panic!("expected a line starting with {saved_marker:?} in content:\n{content}")
        });
    let csv_path = csv_line
        .strip_prefix(saved_marker)
        .and_then(|s| s.strip_suffix('_'))
        .unwrap_or_else(|| panic!("malformed table-saved line: {csv_line:?}"));
    assert!(
        csv_path.starts_with(output_root.to_str().unwrap()),
        "csv path {csv_path} should live under {output_root:?}"
    );
    let csv_bytes = std::fs::read_to_string(csv_path)
        .unwrap_or_else(|e| panic!("csv file at {csv_path} should exist: {e}"));
    assert!(csv_bytes.contains("A,B"), "csv header missing: {csv_bytes}");
    assert!(csv_bytes.contains("1,2"), "csv row missing: {csv_bytes}");

    // The frontmatter also records the same path under `tables_transformed`.
    assert!(
        content.contains("path:") && content.contains(csv_path),
        "expected frontmatter `path:` entry naming {csv_path}, got:\n{content}"
    );

    // --- Image: the response rewrites the <img> src to an absolute, ------
    // --- existing downloaded file path ------------------------------------
    let img_marker = "![probe](";
    let img_start = content
        .find(img_marker)
        .unwrap_or_else(|| panic!("expected {img_marker:?} in content:\n{content}"))
        + img_marker.len();
    let img_end = content[img_start..]
        .find(')')
        .unwrap_or_else(|| panic!("expected closing `)` after image path in content:\n{content}"));
    let img_path = &content[img_start..img_start + img_end];
    assert!(
        img_path.starts_with(output_root.to_str().unwrap()),
        "image path {img_path} should live under {output_root:?}"
    );
    let img_bytes = std::fs::read(img_path)
        .unwrap_or_else(|e| panic!("image file at {img_path} should exist: {e}"));
    assert_eq!(
        img_bytes,
        PROBE_PNG.to_vec(),
        "downloaded image bytes should match the served PNG"
    );

    client.cancel().await.unwrap();
}

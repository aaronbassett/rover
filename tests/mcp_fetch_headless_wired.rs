//! Wiring test for the headless feature (M9 fix C1).
//!
//! Verifies the MCP server actually constructs a `HeadlessRenderer` when a
//! request asks for it. Before the C1 fix, `FetchOptions { headless: None,
//! .. }` was hard-coded in `fetch_inner`, so:
//!   - `headless.mode = "on"` returned `headless_renderer_unavailable`, and
//!   - `headless.mode = "auto"` silently fell back to the reqwest result.
//!
//! After the fix, the handler owns a shared `OnceCell<Arc<HeadlessRenderer>>`
//! and lazily initializes it on first use. We assert that no `mode = "on"`
//! call ever produces the `headless_renderer_unavailable` error — the only
//! acceptable terminal states are:
//!   - success (Chrome installed, SPA rendered), or
//!   - `headless_launch_failed` (Chrome missing/broken in this env), or
//!   - `headless_render_timeout` / other downstream headless errors (proving
//!     the renderer was at least constructed).
//!
//! Requires the `headless` Cargo feature and a Chromium-class browser
//! reachable via chromiumoxide's default launch heuristics. Marked
//! `#[ignore]`; CI runs it via Task 53's nightly smoketest workflow
//! (`cargo test --features headless,test-loopback --test \
//! mcp_fetch_headless_wired -- --ignored`).

#![cfg(all(feature = "headless", feature = "test-loopback"))]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

/// The wiring contract: when a client sends `headless.mode = "on"`, the
/// server MUST attempt to construct a `HeadlessRenderer`. If construction
/// fails (no Chrome in the env), the surfaced error code is
/// `headless_launch_failed` — not `headless_renderer_unavailable`, which
/// would mean construction was never even attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the rover binary to be built with --features headless,test-loopback and a Chromium-class browser available"]
async fn headless_mode_on_attempts_construction() {
    // Tiny static body — we don't care about the response, only about the
    // path the server takes when the client asks for headless rendering.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string("<html><body><h1>hi</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    params = params.with_arguments(
        json!({
            "url": server.uri(),
            "headless": { "mode": "on" }
        })
        .as_object()
        .cloned()
        .unwrap(),
    );

    match client.call_tool(params).await {
        Ok(res) => {
            // Chrome is installed → render succeeded (or failed for an
            // unrelated reason embedded in the success envelope). Either way
            // the renderer was constructed, which is what we're proving.
            let blob = serde_json::to_string(&res).unwrap();
            assert!(
                !blob.contains("headless_renderer_unavailable"),
                "renderer construction was skipped on a `mode: on` call: {blob}",
            );
        }
        Err(rmcp::ServiceError::McpError(err)) => {
            let blob = serde_json::to_string(&err).unwrap();
            assert!(
                !blob.contains("headless_renderer_unavailable"),
                "wiring regression: server returned `headless_renderer_unavailable` on `mode: on` — the renderer is not being constructed at runtime: {blob}",
            );
            // Acceptable failure modes: chrome not installed, render
            // timeout, page closed, internal CDP error. All of them prove
            // the renderer construction code path was reached.
            let saw_expected_error = blob.contains("headless_launch_failed")
                || blob.contains("headless_render_timeout")
                || blob.contains("headless_page_closed")
                || blob.contains("headless_internal_error");
            assert!(
                saw_expected_error,
                "expected a downstream headless_* error code, got: {blob}",
            );
        }
        Err(other) => panic!("unexpected transport-level error: {other:?}"),
    }

    client.cancel().await.unwrap();
    drop(server);
}

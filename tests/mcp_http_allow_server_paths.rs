//! `allow_server_paths` escape-hatch coverage, isolated in its own binary.
//!
//! This test mutates the process environment (`ROVER_DATA_DIR`) via
//! `std::env::set_var`, which is unsound to do next to concurrently running
//! sibling tests: libtest runs every test in a binary on its own thread by
//! default, `setenv` is not thread-safe against a concurrent `getenv`
//! (POSIX leaves the interleaving undefined, and glibc/musl can hand back a
//! torn pointer), and this binary's CI job (`.github/workflows/ci.yml`) is
//! PR-gating — a torn read here is a rare, unreproducible segfault landing on
//! someone else's unrelated PR, not a local reproduction.
//!
//! `tests/mcp_http_transport.rs` used to hold this test alongside 15 others
//! that construct `reqwest::Client`s (which scan proxy env vars on
//! construction) and `tempfile::tempdir()`s (which reads `TMPDIR`) — both are
//! `getenv` call sites that could race the `set_var` here. Cargo compiles
//! each file under `tests/` as its own binary, so putting this test alone in
//! its own file removes the sibling threads entirely: nothing else in this
//! process ever calls `getenv` while this test's `set_var` is in flight.

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::mcp_request;
use tower::ServiceExt as _;

/// The escape hatch: with `[http] allow_server_paths = true`, a `csv_file`
/// call that would otherwise be refused over HTTP (see
/// `mcp_http_transport.rs`'s `csv_file_tables_mode_is_refused_over_http`)
/// must go through instead. Unlike that refusal test, this call is not
/// hermetic — the guard now lets it reach the network — so it's pointed at a
/// local `wiremock` origin rather than `https://example.com/`, and
/// `ROVER_DATA_DIR` is set so `tokenizer::ensure_loaded` (reached via
/// `fetch_inner` after the guard) finds the fixture tokenizer
/// `common::http_state_with` seeds, instead of resolving the developer's
/// real data directory.
#[tokio::test]
async fn csv_file_tables_mode_is_allowed_when_allow_server_paths_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test in this binary (see module docs above),
    // so there is nothing else in this process racing this mutation.
    unsafe { std::env::set_var("ROVER_DATA_DIR", tmp.path()) };

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string("<html><body><h1>hello from wiremock</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let http_cfg = rover::config::HttpConfig {
        allow_server_paths: true,
        ..Default::default()
    };
    let app = rover::mcp::http::router(common::http_state_with(tmp.path(), None, http_cfg).await);

    let body = Body::from(format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"fetch_tool","arguments":{{"url":"{}","tables":{{"mode":"csv_file"}}}}}}}}"#,
        server.uri(),
    ));
    let res = app.oneshot(mcp_request("POST", None, body)).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("response body was not valid JSON: {e}"));
    assert!(
        json.get("error").is_none(),
        "allow_server_paths=true must not refuse the call, got: {json}"
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("hello from wiremock"),
        "expected the fetched page content to come through, got: {text}"
    );
}

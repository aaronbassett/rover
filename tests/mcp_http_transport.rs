//! Router-level tests for the HTTP transport, driven in-process via
//! `tower::ServiceExt::oneshot` — no sockets, no ports, no flakiness.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt as _;

const TOKEN: &str = "test-token-with-enough-entropy";

/// rmcp's `validate_dns_rebinding_headers` requires a `Host` header (or an
/// `:authority` pseudo-header) on every request before it even looks at the
/// allow-list — confirmed against
/// `rmcp-1.7.0/src/transport/streamable_http_server/tower.rs`'s
/// `parse_host_header`. In-process `oneshot` testing builds the `Request`
/// directly in Rust, skipping the HTTP/1.1 wire parsing that would normally
/// populate `Host` from a real client, so it must be set explicitly here or
/// every request 400s with `Bad Request: missing Host header` regardless of
/// `allowed_hosts`. `common::http_state` binds a non-loopback address, which
/// makes `resolve_allowed_hosts` return `None` (validation disabled), so any
/// well-formed host value is accepted.
fn mcp_request(method: &str, auth: Option<&str>, body: Body) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri("/mcp")
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(a) = auth {
        b = b.header(header::AUTHORIZATION, a);
    }
    b.body(body).unwrap()
}

#[tokio::test]
async fn healthz_returns_200_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let state = common::http_state(tmp.path(), None).await;
    let app = rover::mcp::http::router(state);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "healthz body should carry the version, got {text:?}"
    );
}

#[tokio::test]
async fn readyz_returns_200_when_db_is_open() {
    let tmp = tempfile::tempdir().unwrap();
    let state = common::http_state(tmp.path(), None).await;
    let app = rover::mcp::http::router(state);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.starts_with("ready schema_version="),
        "readyz body should carry the ready+schema_version contract, got {text:?}"
    );
}

/// Forces `Db::schema_version()` into its `Err` arm by corrupting the
/// on-disk SQLite files out from under the already-open connection, then
/// asserts `readyz` reports 503.
///
/// No production code changes and no test-only hooks: `HttpState`/`router`
/// are exercised exactly as `healthz_returns_200_and_version` and
/// `readyz_returns_200_when_db_is_open` exercise them above. The only extra
/// step is filesystem tampering, done through `Db::path()`'s public path
/// (the same file `common::http_state` already opened via
/// `data_dir.join("rover.db")`) plus its WAL/SHM siblings — `open_with_migrations`
/// enables WAL mode, so recently-committed pages live in `-wal`, and leaving
/// it intact lets SQLite serve the read from a still-valid WAL snapshot
/// instead of ever touching the corrupted main file. Overwriting all three
/// in place (same inode, so the connection's open file descriptors see the
/// new bytes) reliably drives the next query into `SqliteFailure(NotADatabase)`.
/// Verified during development (not asserted blind) by installing a tracing
/// subscriber and confirming the exact log line `readyz probe failed
/// error=Backend(Error(SqliteFailure(Error { code: NotADatabase, ... },
/// Some("file is not a database"))))` fires before the 503 is observed, and
/// by re-running the test five times to rule out a page-cache race that
/// only fails intermittently.
#[tokio::test]
async fn readyz_returns_503_when_db_query_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let state = common::http_state(tmp.path(), None).await;
    let app = rover::mcp::http::router(state);

    let db_path = tmp.path().join("rover.db");
    for suffix in ["", "-wal", "-shm"] {
        let mut p = db_path.clone().into_os_string();
        p.push(suffix);
        let p = std::path::PathBuf::from(p);
        if p.exists() {
            std::fs::write(
                &p,
                b"not a sqlite database, deliberately corrupted for this test",
            )
            .unwrap();
        }
    }

    let res = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"not ready\n");
}

#[tokio::test]
async fn initialize_over_http_returns_json() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let body = Body::from(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );
    let res = app.oneshot(mcp_request("POST", None, body)).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        ct.starts_with("application/json"),
        "stateless + json_response must yield JSON, got {ct:?}"
    );
}

#[tokio::test]
async fn get_and_delete_on_mcp_are_405_with_a_valid_token() {
    // Deliberately authenticated: with auth on and no header this would be
    // 401, which is the middleware's answer, not the transport's.
    let tmp = tempfile::tempdir().unwrap();
    let state = common::http_state(tmp.path(), Some(TOKEN)).await;
    let app = rover::mcp::http::router(state);
    let auth = format!("Bearer {TOKEN}");

    for method in ["GET", "DELETE"] {
        let res = app
            .clone()
            .oneshot(mcp_request(method, Some(&auth), Body::empty()))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} /mcp should be 405"
        );
    }
}

#[tokio::test]
async fn accept_without_event_stream_is_406() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json")
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn wrong_content_type_is_415() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(Body::from("hello"))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn undeserialisable_body_is_415_not_500() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let res = app
        .oneshot(mcp_request("POST", None, Body::from("{not json")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

/// `RequestBodyLimitLayer` returns 413 early ONLY when a `Content-Length`
/// header is present and exceeds the limit
/// (tower-http-0.6.10/src/limit/service.rs:49-56). Without it the body is
/// merely wrapped, the overflow reaches rmcp's `expect_json` as a collect
/// error, and that arm answers 500. `Request::builder().body(...)` sets no
/// `Content-Length`, so this test MUST set it explicitly or it asserts the
/// wrong code. Real MCP clients always send it.
#[tokio::test]
async fn oversize_body_with_content_length_is_413() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let huge = vec![b'x'; 17 * 1024 * 1024];
    let len = huge.len();
    let mut req = mcp_request("POST", None, Body::from(huge));
    req.headers_mut().insert(
        header::CONTENT_LENGTH,
        axum::http::HeaderValue::from_str(&len.to_string()).unwrap(),
    );

    let res = app.oneshot(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

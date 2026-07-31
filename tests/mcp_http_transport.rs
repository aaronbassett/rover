//! Router-level tests for the HTTP transport, driven in-process via
//! `tower::ServiceExt::oneshot` — no sockets, no ports, no flakiness.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

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

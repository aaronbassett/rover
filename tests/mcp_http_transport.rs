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
}

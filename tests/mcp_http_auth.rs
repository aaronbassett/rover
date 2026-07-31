//! Bearer-token middleware tests. `/mcp` is gated; the health probes are not.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt as _;

const TOKEN: &str = "test-token-with-enough-entropy";

#[tokio::test]
async fn absent_token_is_rejected_when_auth_is_on() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            None,
            Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        res.headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );
}

#[tokio::test]
async fn wrong_token_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some("Bearer not-the-token"),
            Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn correct_token_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some(&format!("Bearer {TOKEN}")),
            Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        ))
        .await
        .unwrap();

    assert_ne!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a correct token must pass the middleware"
    );
}

#[tokio::test]
async fn health_probes_are_never_behind_the_token_wall() {
    let tmp = tempfile::tempdir().unwrap();
    let state = common::http_state(tmp.path(), Some(TOKEN)).await;
    let app = rover::mcp::http::router(state);

    for path in ["/healthz", "/readyz"] {
        let res = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path} must not require auth");
    }
}

#[tokio::test]
async fn no_token_configured_means_no_auth() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), None).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            None,
            Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        ))
        .await
        .unwrap();

    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
}

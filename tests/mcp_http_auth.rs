//! Bearer-token middleware tests. `/mcp` is gated; the health probes are not.

mod common;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use tower::ServiceExt as _;

const TOKEN: &str = "test-token-with-enough-entropy";

fn ping_body() -> Body {
    Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
}

/// Read a response down to its full shape: status, `WWW-Authenticate`
/// header value, and body bytes. Used to assert the 401 contract in full,
/// not just the status code — a header or body change is exactly the kind
/// of regression a status-only assertion would miss.
async fn response_shape(res: Response<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let status = res.status();
    let www_authenticate = res
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, www_authenticate, body)
}

#[tokio::test]
async fn absent_token_is_rejected_when_auth_is_on() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request("POST", None, ping_body()))
        .await
        .unwrap();

    let (status, www_authenticate, body) = response_shape(res).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(www_authenticate.as_deref(), Some("Bearer"));
    assert_eq!(body, b"unauthorized\n");
}

#[tokio::test]
async fn wrong_token_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some("Bearer not-the-token"),
            ping_body(),
        ))
        .await
        .unwrap();

    let (status, www_authenticate, body) = response_shape(res).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(www_authenticate.as_deref(), Some("Bearer"));
    assert_eq!(body, b"unauthorized\n");
}

/// States the "absent" vs "wrong" invariant directly rather than by
/// coincidence of two separate assertions elsewhere: someone could add
/// distinguishing detail (e.g. an RFC-6750-style `error="invalid_token"` on
/// the wrong-token path but not the absent-token path) and still leave
/// `absent_token_is_rejected_when_auth_is_on` and `wrong_token_is_rejected`
/// both green, since neither compares against the other. This test would
/// catch that.
#[tokio::test]
async fn absent_and_wrong_token_responses_are_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let absent = app
        .clone()
        .oneshot(common::mcp_request("POST", None, ping_body()))
        .await
        .unwrap();
    let wrong = app
        .oneshot(common::mcp_request(
            "POST",
            Some("Bearer not-the-token"),
            ping_body(),
        ))
        .await
        .unwrap();

    let a = response_shape(absent).await;
    let w = response_shape(wrong).await;
    assert_eq!(
        a, w,
        "absent-token and wrong-token responses must be identical in status, \
         WWW-Authenticate, and body — the reject path must not leak which \
         case occurred"
    );
}

#[tokio::test]
async fn correct_token_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some(&format!("Bearer {TOKEN}")),
            ping_body(),
        ))
        .await
        .unwrap();

    assert_ne!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a correct token must pass the middleware"
    );
}

/// RFC 9110 §11.1 / RFC 6750 §2.1: the auth-scheme token is case-insensitive.
/// A conforming client sending `bearer <token>` (or any other casing) with a
/// correct token must not be rejected.
#[tokio::test]
async fn lowercase_bearer_scheme_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some(&format!("bearer {TOKEN}")),
            ping_body(),
        ))
        .await
        .unwrap();

    assert_ne!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a lowercase `bearer` scheme with a correct token must pass the middleware"
    );
}

/// Pins token comparison as case-sensitive, distinct from the scheme match
/// above. Making the *scheme* case-insensitive (`eq_ignore_ascii_case` on
/// `"Bearer"`) is correct per RFC 9110 §11.1 / RFC 6750 §2.1 — but widening
/// that same treatment to the *token* itself would be an easy, plausible
/// mistake when skimming those RFCs quickly, and it would silently cut the
/// token's effective entropy. Do not "fix" a failure here by relaxing the
/// token comparison — the token must stay byte-exact; only the scheme name
/// is case-insensitive.
#[tokio::test]
async fn token_comparison_is_case_sensitive() {
    let tmp = tempfile::tempdir().unwrap();
    let app = rover::mcp::http::router(common::http_state(tmp.path(), Some(TOKEN)).await);

    let flipped = TOKEN.to_uppercase();
    assert_ne!(
        flipped, TOKEN,
        "sanity check: the flipped token must actually differ from TOKEN"
    );

    let res = app
        .oneshot(common::mcp_request(
            "POST",
            Some(&format!("Bearer {flipped}")),
            ping_body(),
        ))
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a case-flipped token must be rejected — only the Bearer scheme is \
         case-insensitive, never the token"
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
        .oneshot(common::mcp_request("POST", None, ping_body()))
        .await
        .unwrap();

    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
}

/// Pins the unrouted-path side effect the brief calls out explicitly: with a
/// token configured, `Router::layer` also wraps the fallback, so any
/// unrouted path 401s instead of 404ing. Without a token, the fallback is
/// unwrapped and unrouted paths 404 normally. Task 6 restructures `router()`
/// to mount `/mcp` — this test exists so that restructure cannot silently
/// change this behaviour without a test failing.
#[tokio::test]
async fn unrouted_path_401s_with_token_but_404s_without() {
    let with_token_tmp = tempfile::tempdir().unwrap();
    let with_token_app =
        rover::mcp::http::router(common::http_state(with_token_tmp.path(), Some(TOKEN)).await);
    let res = with_token_app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "with a token configured, the auth layer also guards the fallback"
    );

    let without_token_tmp = tempfile::tempdir().unwrap();
    let without_token_app =
        rover::mcp::http::router(common::http_state(without_token_tmp.path(), None).await);
    let res = without_token_app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "without a token, unrouted paths 404 normally"
    );
}

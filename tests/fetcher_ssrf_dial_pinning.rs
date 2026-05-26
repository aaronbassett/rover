//! End-to-end check that the custom DNS resolver re-applies the SSRF
//! address policy at dial time, closing the resolve-then-dial TOCTOU
//! window described in `docs/security.md` §"DNS rebinding".
//!
//! We can't easily stand up a malicious authoritative DNS server in a
//! unit test, but we can exercise the same code path by having the
//! request target a hostname whose system resolution lands on a
//! loopback address. Under `SsrfLevel::Strict` the resolver must
//! reject the dial; under `Loopback` it must allow it.

use std::time::Duration;

use rover::fetcher::client::build_http_client;
use rover::fetcher::dns::{SSRF_LEVEL, dial_blocked_cause};
use rover::fetcher::ssrf::SsrfLevel;

#[tokio::test]
async fn strict_level_blocks_loopback_dial() {
    let client = build_http_client("rover-test/0.1", Duration::from_secs(5));
    // `localhost` resolves to 127.0.0.1 / ::1 — both rejected at Strict.
    let result = SSRF_LEVEL
        .scope(SsrfLevel::Strict, async {
            client.get("http://localhost:9/").send().await
        })
        .await;
    let err = result.expect_err("strict must reject loopback at dial time");
    assert!(
        dial_blocked_cause(&err).is_some(),
        "expected DialBlocked in error chain, got: {err}",
    );
}

#[tokio::test]
async fn loopback_level_permits_loopback_dial() {
    // Start a one-shot TCP listener so the dial actually succeeds at the
    // socket layer (the server doesn't speak HTTP, so the request itself
    // will fail — we just need to confirm we reached the loopback IP).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accept_task = tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let client = build_http_client("rover-test/0.1", Duration::from_secs(2));
    let url = format!("http://localhost:{port}/");
    let result = SSRF_LEVEL
        .scope(SsrfLevel::Loopback, async { client.get(&url).send().await })
        .await;
    // Either Ok (junk response) or Err for an HTTP-layer reason — what
    // must NOT happen is DialBlocked at the resolver layer.
    if let Err(e) = &result {
        assert!(
            dial_blocked_cause(e).is_none(),
            "loopback level must not be blocked at dial time, got: {e}",
        );
    }
    accept_task.abort();
}

#[tokio::test]
async fn no_scope_means_no_policy_enforcement() {
    // Without an SSRF_LEVEL scope active, the resolver falls through to a
    // plain lookup. This documents the resolver's pass-through behaviour
    // for callers (cloud captioner, summariser) that don't go through the
    // policed fetch path.
    let client = build_http_client("rover-test/0.1", Duration::from_secs(2));
    let result = client.get("http://127.0.0.1:1/").send().await;
    if let Err(e) = &result {
        assert!(
            dial_blocked_cause(e).is_none(),
            "no scope must not block; got: {e}",
        );
    }
}

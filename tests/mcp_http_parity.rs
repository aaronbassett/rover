//! End-to-end HTTP transport tests over a real socket, plus CLI wiring.

mod common;

#[test]
fn bind_without_http_is_a_parse_error() {
    let out = std::process::Command::new(common::bin_path())
        .args(["mcp", "--bind", "0.0.0.0:7683"])
        .output()
        .expect("run rover");

    assert!(!out.status.success(), "--bind without --http must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--http"),
        "error should name the required flag, got: {stderr}"
    );
}

const CFG: &str = "[robots]\nrespect = false\n\n[ssrf]\nlevel = \"loopback\"\n";

#[tokio::test]
async fn full_handshake_and_fetch_over_a_real_socket() {
    common::init_crypto();
    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><head><title>S</title></head><body><article><h1>S</h1>\
                     <p>Hello from wiremock.</p></article></body></html>",
                ),
        )
        .mount(&origin)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let (mut child, base) = common::spawn_http_server(tmp.path(), CFG, None).await;

    let transport = StreamableHttpClientTransport::from_uri(format!("{base}/mcp"));
    let client = ().serve(transport).await.expect("handshake");

    let result = client
        .call_tool(
            CallToolRequestParams::new("fetch_tool".to_string()).with_arguments(
                serde_json::json!({ "url": origin.uri() })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .expect("fetch_tool over http");

    let text = format!("{result:?}");
    assert!(text.contains("Hello from wiremock"), "got: {text}");

    client.cancel().await.ok();
    child.kill().await.ok();
}

/// The same fetch over both transports must produce the same document.
///
/// NOT byte-identical: two things vary per call regardless of transport.
///
/// 1. The injection-guard nonce — 3 random bytes, 6 hex chars, appearing in
///    THREE places: the two tags (`src/guard/wrap.rs:12-15`) and the preamble,
///    where it is emitted bare as `nonce: {nonce}` with no `untrusted-content-`
///    prefix (`src/guard/wrap.rs:31`). Redacting only the tag form leaves the
///    preamble occurrence and the comparison fails.
/// 2. `fetched_at` — `jiff::Timestamp::now()` is called on every render
///    (`src/mcp/tools/fetch.rs:867`), so a cache hit does NOT pin it. Only
///    `content_hash` is stable across the two calls.
///
/// So: redact the nonce in both forms, redact the timestamp, normalise
/// `cache_status`, then compare.
#[tokio::test]
async fn stdio_and_http_produce_the_same_document() {
    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><head><title>Parity</title></head><body><article><h1>Parity</h1>\
                     <p>Same bytes both ways.</p></article></body></html>",
                ),
        )
        .mount(&origin)
        .await;

    common::init_crypto();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("rover.toml"), CFG).unwrap();
    // `spawn_client` does NOT seed the tokenizer — only `spawn_http_server`
    // does, and that runs after the stdio leg. Without this the stdio fetch
    // attempts a live HuggingFace download.
    common::seed_default_tokenizer(tmp.path());

    // A plain `async fn`, not a closure: a closure capturing a `&Running-
    // Service` reference and returning the borrowing `call_tool` future
    // can't be given the higher-ranked lifetime it needs when called at two
    // call sites with distinct concrete lifetimes (`c: &'1 _` would need to
    // outlive the closure's own inferred, single lifetime `'2`) — rustc
    // rejects it with "lifetime may not live long enough". A free `async
    // fn` sidesteps this because its signature is implicitly `for<'a> ...`.
    async fn call(
        c: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
        url: String,
    ) -> Result<rmcp::model::CallToolResult, rmcp::service::ServiceError> {
        c.call_tool(
            CallToolRequestParams::new("fetch_tool".to_string()).with_arguments(
                serde_json::json!({ "url": url })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
    }

    // 1. stdio first — this is the cache MISS.
    let stdio_result = {
        let client = common::spawn_client(tmp.path()).await;
        let r = call(&client, origin.uri()).await.expect("stdio fetch");
        client.cancel().await.ok();
        r
    };

    // 2. HTTP second, same data dir — cache HIT.
    let (mut child, base) = common::spawn_http_server(tmp.path(), CFG, None).await;
    let http_result = {
        let transport = StreamableHttpClientTransport::from_uri(format!("{base}/mcp"));
        let client = ().serve(transport).await.expect("handshake");
        let r = call(&client, origin.uri()).await.expect("http fetch");
        client.cancel().await.ok();
        r
    };
    child.kill().await.ok();

    // Compare the STRUCTURED output, not a Debug string. `fetch_tool` returns
    // `Json<FetchOutput>` (src/mcp/handler.rs:135), which rmcp places in
    // `structured_content`; `FetchOutput` is `#[serde(untagged)]`, so
    // `FetchResponse`'s fields (`content`, `cache_status` —
    // src/mcp/envelope.rs:48-49) sit at the JSON root. Working on the
    // unescaped markdown avoids all backslash arithmetic.
    let parts = |r: &rmcp::model::CallToolResult| {
        let v = r.structured_content.as_ref().expect("structured content");
        (
            v["content"].as_str().expect("content").to_string(),
            v["cache_status"]
                .as_str()
                .expect("cache_status")
                .to_string(),
        )
    };
    let (stdio_doc, stdio_cache) = parts(&stdio_result);
    let (http_doc, http_cache) = parts(&http_result);

    assert_eq!(stdio_cache, "miss", "the stdio call should be a cache miss");
    assert_eq!(http_cache, "hit", "the http call should be a cache hit");

    let norm = |s: &str| {
        let s = regex::Regex::new(r"untrusted-content-[0-9a-f]{6}")
            .unwrap()
            .replace_all(s, "untrusted-content-NONCE")
            .into_owned();
        let s = regex::Regex::new(r"nonce: [0-9a-f]{6}")
            .unwrap()
            .replace_all(&s, "nonce: NONCE")
            .into_owned();
        regex::Regex::new(r#"fetched_at: "[^"]*""#)
            .unwrap()
            .replace_all(&s, r#"fetched_at: "TS""#)
            .into_owned()
    };

    assert_eq!(
        norm(&stdio_doc),
        norm(&http_doc),
        "the same fetch must yield the same document over both transports"
    );
}

/// N concurrent callers all succeed, and the cache is genuinely shared: only
/// the first request reaches the origin.
#[tokio::test]
async fn concurrent_callers_share_one_cache() {
    common::init_crypto();
    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string("<html><body><article><p>shared</p></article></body></html>"),
        )
        .mount(&origin)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let (mut child, base) = common::spawn_http_server(tmp.path(), CFG, None).await;
    let url = origin.uri();

    // Warm the cache once, then fire 8 concurrent callers.
    {
        let t = StreamableHttpClientTransport::from_uri(format!("{base}/mcp"));
        let c = ().serve(t).await.unwrap();
        c.call_tool(
            CallToolRequestParams::new("fetch_tool".to_string()).with_arguments(
                serde_json::json!({ "url": url })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
        c.cancel().await.ok();
    }

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let base = base.clone();
        let url = url.clone();
        set.spawn(async move {
            let t = StreamableHttpClientTransport::from_uri(format!("{base}/mcp"));
            let c = ().serve(t).await.unwrap();
            let r = c
                .call_tool(
                    CallToolRequestParams::new("fetch_tool".to_string()).with_arguments(
                        serde_json::json!({ "url": url })
                            .as_object()
                            .cloned()
                            .unwrap(),
                    ),
                )
                .await;
            c.cancel().await.ok();
            r.is_ok()
        });
    }
    let mut ok = 0;
    while let Some(res) = set.join_next().await {
        if res.unwrap() {
            ok += 1;
        }
    }
    assert_eq!(ok, 8, "all concurrent callers should succeed");

    // One warm-up request only — the eight concurrent ones were cache hits.
    assert_eq!(
        origin.received_requests().await.unwrap().len(),
        1,
        "the shared cache should have absorbed every concurrent repeat"
    );

    child.kill().await.ok();
}

/// SIGTERM must drain cleanly, with no spurious headless-renderer warning.
#[tokio::test]
async fn sigterm_shuts_down_cleanly() {
    common::init_crypto();
    let tmp = tempfile::tempdir().unwrap();
    let (mut child, base) = common::spawn_http_server(tmp.path(), CFG, None).await;

    // Prove it is live before signalling.
    let body = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert!(body.status().is_success());

    let pid = child.id().expect("child pid") as i32;
    unsafe { libc::kill(pid, libc::SIGTERM) };

    let status = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait())
        .await
        .expect("shutdown timed out")
        .expect("wait");
    assert!(status.success(), "clean SIGTERM shutdown should exit 0");
}

/// A non-loopback bind must accept a container-style Host header. If the
/// derivation regresses to rmcp's loopback default, this 403s — and nothing
/// else in the suite would notice until someone deployed it.
#[tokio::test]
async fn container_style_host_header_is_accepted_on_a_public_bind() {
    common::init_crypto();
    let tmp = tempfile::tempdir().unwrap();
    // spawn_http_server binds 127.0.0.1:0, which IS loopback — so force the
    // disabled posture explicitly via config rather than via the bind.
    let cfg = format!("{CFG}\n[http]\nallowed_hosts = [\"*\"]\n");
    let (mut child, base) = common::spawn_http_server(tmp.path(), &cfg, None).await;

    let port = base.rsplit(':').next().unwrap();
    let res = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Host", format!("rover:{port}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
        .send()
        .await
        .expect("request");

    assert_ne!(
        res.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a container-style Host must not be refused"
    );
    child.kill().await.ok();
}

/// The negative case: with `allowed_hosts = ["localhost"]` (i.e. Host
/// validation ENABLED, restricted to `localhost`), a request carrying a
/// disallowed `Host` header must be refused with 403. Without this test,
/// `container_style_host_header_is_accepted_on_a_public_bind` would pass
/// trivially even if Host validation were broken open entirely (e.g. always
/// disabled) — this proves the check is actually enforcing something.
#[tokio::test]
async fn disallowed_host_header_is_refused_with_403() {
    common::init_crypto();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = format!("{CFG}\n[http]\nallowed_hosts = [\"localhost\"]\n");
    let (mut child, base) = common::spawn_http_server(tmp.path(), &cfg, None).await;

    let res = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Host", "evil.example")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
        .send()
        .await
        .expect("request");

    assert_eq!(
        res.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a disallowed Host header must be refused when allowed_hosts is restricted"
    );
    child.kill().await.ok();
}

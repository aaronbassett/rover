//! End-to-end MCP test for `summarize`.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client};

fn html_body() -> &'static str {
    "<html><head><title>Hello</title></head><body>\
     <article>\
     <h1>Hello</h1>\
     <p>The Midnight Network is a privacy-preserving blockchain. It uses zero-knowledge proofs.</p>\
     <p>The native token is NIGHT. STAR is the unit of account for transaction fees.</p>\
     </article>\
     </body></html>"
}

#[tokio::test]
async fn summarize_returns_extractive_output_on_cache_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article", server.uri());
    let mut params = CallToolRequestParams::new("summarize_tool".to_string());
    let args = json!({
        "url": url,
        "mode": "extractive",
        "target_tokens": 50,
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.expect("summarize succeeded");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"content\""), "missing content: {blob}");
    assert!(
        blob.contains("\"mode\":\"extractive\""),
        "expected mode=extractive: {blob}"
    );
    assert!(
        blob.contains("\"cache_status\":\"miss\""),
        "expected cache_status=miss: {blob}"
    );
    assert!(
        blob.contains("\"backend\":\"default\""),
        "expected backend=default: {blob}"
    );

    // Parse the response and assert structured metadata fields propagate from args.
    let outer: serde_json::Value = serde_json::from_str(&blob).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("tool returned text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["metadata"]["target_tokens"], 50);
    assert_eq!(v["metadata"]["backend"], "default");
    assert_eq!(v["metadata"]["mode"], "extractive");
    assert_eq!(v["metadata"]["style"], "prose");
    assert!(
        v["metadata"]["source_url"]
            .as_str()
            .unwrap()
            .contains("/article")
    );
    assert!(
        !v["metadata"]["source_fetched_at"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert_eq!(v["metadata"]["preserve"], serde_json::json!([]));
    assert!(!v["content"].as_str().unwrap().is_empty());

    client.cancel().await.unwrap();
    drop(server);
}

#[tokio::test]
async fn summarize_second_call_hits_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article2", server.uri());
    let args = json!({
        "url": url,
        "mode": "extractive",
    });

    let mut p1 = CallToolRequestParams::new("summarize_tool".to_string());
    if let Some(obj) = args.as_object().cloned() {
        p1 = p1.with_arguments(obj);
    }
    let _ = client.call_tool(p1).await.unwrap();

    let mut p2 = CallToolRequestParams::new("summarize_tool".to_string());
    if let Some(obj) = args.as_object().cloned() {
        p2 = p2.with_arguments(obj);
    }
    let res = client.call_tool(p2).await.unwrap();
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("\"cache_status\":\"hit\""),
        "expected cache_status=hit on second call: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}

/// End-to-end fallback metadata: when the configured "fast" cloud backend
/// fails (wiremock returns 5xx → BackendError::Unavailable), the service
/// falls back to the extractive "default" backend and the response carries
/// `summarizer_fallback { from: "fast", reason: "backend_unavailable" }`.
#[tokio::test]
async fn summarize_falls_back_to_extractive_when_cloud_unavailable() {
    // Wiremock #1: serves the HTML the fetch step consumes.
    let html_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&html_server)
        .await;

    // Wiremock #2: the "cloud" LLM endpoint that returns 5xx so the cloud
    // backend maps the error to BackendError::Unavailable.
    let llm_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
        .mount(&llm_server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());

    // Write a config with two backends: a cloud "fast" pointed at the
    // wiremock LLM endpoint and an extractive "default" that serves as the
    // fallback target. `spawn_client` only writes the default config if
    // none exists, so pre-writing here wins.
    let cfg_path = tmp.path().join("rover.toml");
    let cfg = format!(
        r#"
[robots]
respect = false

[ssrf]
level = "loopback"

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai_compat"
base_url = "{}/v1/"
model = "test-model"
api_key_env = "ROVER_TEST_FAKE_KEY"
"#,
        llm_server.uri()
    );
    std::fs::write(&cfg_path, cfg).unwrap();
    // Set a non-empty API key so genai sends the request. The value doesn't
    // matter — wiremock returns 503 regardless.
    unsafe {
        std::env::set_var("ROVER_TEST_FAKE_KEY", "test-key");
    }

    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article3", html_server.uri());
    let mut params = CallToolRequestParams::new("summarize_tool".to_string());
    let args = json!({
        "url": url,
        "backend": "fast",
        "mode": "abstractive",
        "target_tokens": 50,
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.expect("summarize succeeded");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let outer = serde_json::to_value(&res).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("tool returned text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(
        v["metadata"]["backend"], "default",
        "expected effective backend to be the extractive fallback: {text}"
    );
    let fb = &v["metadata"]["summarizer_fallback"];
    assert_eq!(
        fb["from"], "fast",
        "expected fallback.from = original backend name: {text}"
    );
    let reason = fb["reason"].as_str().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "expected non-empty fallback reason: {text}"
    );
    assert_eq!(
        reason, "backend_unavailable",
        "expected fallback reason from 5xx → Unavailable: {text}"
    );

    client.cancel().await.unwrap();
    drop(html_server);
    drop(llm_server);
}

/// End-to-end fallback metadata: when the configured "fast" cloud backend
/// returns 401 (mapped to BackendError::AuthFailed), the service falls back
/// to the extractive "default" backend and the response carries
/// `summarizer_fallback { from: "fast", reason: "auth_failed" }`.
#[tokio::test]
async fn summarize_falls_back_to_extractive_when_cloud_returns_401() {
    let html_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article4"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html_body()),
        )
        .mount(&html_server)
        .await;

    let llm_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": "invalid api key", "type": "invalid_request_error" }
        })))
        .mount(&llm_server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());

    let cfg_path = tmp.path().join("rover.toml");
    let cfg = format!(
        r#"
[robots]
respect = false

[ssrf]
level = "loopback"

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai_compat"
base_url = "{}/v1/"
model = "test-model"
api_key_env = "ROVER_TEST_FAKE_KEY_401"
"#,
        llm_server.uri()
    );
    std::fs::write(&cfg_path, cfg).unwrap();
    unsafe {
        std::env::set_var("ROVER_TEST_FAKE_KEY_401", "wrong-key");
    }

    let client = spawn_client(tmp.path()).await;

    let url = format!("{}/article4", html_server.uri());
    let mut params = CallToolRequestParams::new("summarize_tool".to_string());
    let args = json!({
        "url": url,
        "backend": "fast",
        "mode": "abstractive",
        "target_tokens": 50,
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.expect("summarize succeeded");
    assert!(!res.is_error.unwrap_or(false), "tool errored: {res:?}");

    let outer = serde_json::to_value(&res).unwrap();
    let text = outer["content"][0]["text"]
        .as_str()
        .expect("tool returned text content block");
    let v: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(
        v["metadata"]["backend"], "default",
        "expected effective backend to be the extractive fallback: {text}"
    );
    let fb = &v["metadata"]["summarizer_fallback"];
    assert_eq!(
        fb["from"], "fast",
        "expected fallback.from = original backend name: {text}"
    );
    let reason = fb["reason"].as_str().unwrap_or_default();
    assert_eq!(
        reason, "auth_failed",
        "expected fallback reason from 401 → AuthFailed: {text}"
    );

    client.cancel().await.unwrap();
    drop(html_server);
    drop(llm_server);
}

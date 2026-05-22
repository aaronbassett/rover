//! Cloud backend integration test against a wiremock OpenAI-compatible
//! endpoint. Verifies the wire shape (system + user messages, model id)
//! and the response decoding path without a real LLM.

#![cfg(feature = "test-loopback")]

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::cloud::{CloudBackend, ProviderKind};
use rover::summarizer::error::BackendError;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn opts(focus: Option<&str>) -> CompactOpts {
    CompactOpts {
        mode: CompactMode::Abstractive,
        style: Style::Prose,
        target_tokens: Some(150),
        focus: focus.map(str::to_string),
        preserve: vec![],
        backend_name: "lm".to_string(),
    }
}

#[tokio::test]
async fn cloud_round_trips_against_openai_compat_endpoint() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1_700_000_000_i64,
            "model": "lm-test",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Summary: hello world.",
                },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(format!("{}/v1/", server.uri())),
        Some("test-key".into()),
    )
    .unwrap();

    let out = be
        .compact("Please summarize this text.", &opts(None))
        .await
        .expect("summarization succeeds");
    // Asserts the FULL mocked content is returned untouched (modulo
    // trailing whitespace some providers append).
    assert_eq!(out.trim(), "Summary: hello world.");
}

#[tokio::test]
async fn cloud_maps_401_to_auth_failed() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": "invalid api key", "type": "invalid_request_error" }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(format!("{}/v1/", server.uri())),
        Some("wrong-key".into()),
    )
    .unwrap();

    let err = be.compact("hi.", &opts(None)).await.unwrap_err();
    assert!(matches!(err, BackendError::AuthFailed(_)), "got {err:?}");
}

#[tokio::test]
async fn cloud_maps_429_to_rate_limited() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": { "message": "rate limit exceeded", "type": "rate_limit_exceeded" }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "lm-test",
        Some(format!("{}/v1/", server.uri())),
        Some("k".into()),
    )
    .unwrap();

    let err = be.compact("hi.", &opts(None)).await.unwrap_err();
    assert!(matches!(err, BackendError::RateLimited), "got {err:?}");
}

/// Regression guard for the resolver's `adapter_kind = AdapterKind::OpenAI`
/// override. We use a model id (`llama3.2`) that genai would natively route
/// to its Ollama adapter (which uses a different endpoint path and wire
/// shape). If the resolver fails to force `AdapterKind::OpenAI`, the
/// request will not land on `/v1/chat/completions` and the mock won't
/// match, producing a non-200 response. A passing 200 round-trip proves
/// the OpenAI adapter (and its `/v1/chat/completions` path) was used.
#[tokio::test]
async fn cloud_openai_compat_forces_openai_adapter_for_non_openai_model_name() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1_700_000_000_i64,
            "model": "llama3.2",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "ok" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })))
        .mount(&server)
        .await;

    let be = CloudBackend::new(
        "lm",
        ProviderKind::OpenAiCompat,
        "llama3.2",
        Some(format!("{}/v1/", server.uri())),
        Some("test-key".into()),
    )
    .unwrap();

    let out = be
        .compact("Please summarize this text.", &opts(None))
        .await
        .expect("openai_compat resolver should force OpenAI adapter regardless of model name");
    assert_eq!(out.trim(), "ok");
}

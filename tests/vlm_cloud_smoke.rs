//! Cloud captioner integration test using wiremock-backed openai_compat.
//! Always compiled (no feature gate) — cloud captioners ship in default builds.

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::summarizer::cloud::ProviderKind;
use rover::vlm::VlmCaptioner;
use rover::vlm::cloud::CloudCaptioner;

// 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiremock_openai_compat_caption_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "test",
            "object": "chat.completion",
            "created": 0,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "A small transparent square."},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server)
        .await;

    let cap = CloudCaptioner::new(
        "test",
        ProviderKind::OpenAiCompat,
        "test-model",
        Some(format!("{}/v1/", server.uri())),
        Some("dummy".into()),
    )
    .unwrap();

    let caption = cap
        .caption(PNG, Some("transparent pixel"), 50)
        .await
        .unwrap();
    assert_eq!(caption, "A small transparent square.");
    let recv = server.received_requests().await.unwrap();
    assert_eq!(recv.len(), 1);
    // Sanity: the request body included an image_url part.
    let body = std::str::from_utf8(&recv[0].body).unwrap();
    assert!(body.contains("image"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_short_circuits_second_call() {
    // Same setup as above; call caption() twice with the same image+params
    // through the cache wrapper; assert the wiremock saw exactly one request.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "test", "object": "chat.completion", "created": 0, "model": "x",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "cached"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server)
        .await;

    let cap = CloudCaptioner::new(
        "test",
        ProviderKind::OpenAiCompat,
        "test-model",
        Some(format!("{}/v1/", server.uri())),
        Some("dummy".into()),
    )
    .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let db = rover::storage::Db::open(tmp.path().join("rover.db"))
        .await
        .unwrap();

    // First call: miss → real wiremock.
    let cached = rover::vlm::cache::lookup(&db, PNG, cap.name(), cap.model_id(), 50)
        .await
        .unwrap();
    assert!(cached.is_none());
    let c1 = cap.caption(PNG, None, 50).await.unwrap();
    rover::vlm::cache::insert(&db, PNG, cap.name(), cap.model_id(), 50, &c1)
        .await
        .unwrap();
    // Second call: must hit cache.
    let cached2 = rover::vlm::cache::lookup(&db, PNG, cap.name(), cap.model_id(), 50)
        .await
        .unwrap();
    assert_eq!(cached2.as_deref(), Some("cached"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

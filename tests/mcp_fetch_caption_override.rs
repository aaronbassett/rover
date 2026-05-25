//! Integration test: per-call `images.captioner` override routes the
//! caption request to the named captioner rather than the configured default.
//!
//! Two wiremock servers act as fake `openai_compat` captioners:
//!   - alpha: responds with "alpha says hi"
//!   - beta:  responds with "beta says hi"
//!
//! Config sets `[image_captions] default = "alpha"`. The fetch call passes
//! `images: { mode: "caption", captioner: "beta" }`. The test asserts that
//! the returned markdown contains "beta says hi", proving the per-call
//! override threaded through the whole pipeline (fetch → images::apply →
//! CaptionerRegistry::get(override)).

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client_with_config};

/// Minimal 1x1 transparent PNG. Same bytes used in extractor::images unit tests.
const PROBE_PNG: [u8; 67] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

/// Build a valid OpenAI-compatible chat completion JSON response whose
/// assistant content is `caption_text`.
fn chat_completion_response(caption_text: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1_700_000_000_i64,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": caption_text
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    })
}

#[tokio::test]
async fn captioner_override_routes_to_named_captioner() {
    // ------------------------------------------------------------------
    // 1. Start two fake captioner servers (alpha and beta).
    // ------------------------------------------------------------------
    let alpha_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(chat_completion_response("alpha says hi")),
        )
        .mount(&alpha_server)
        .await;

    let beta_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(chat_completion_response("beta says hi")),
        )
        .mount(&beta_server)
        .await;

    // ------------------------------------------------------------------
    // 2. Start an image server that serves a tiny PNG.
    //    Also serves HEAD and Range requests for the classify() filters.
    // ------------------------------------------------------------------
    let img_server = MockServer::start().await;
    // HEAD for size gate
    Mock::given(method("HEAD"))
        .and(path("/photo.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", PROBE_PNG.len().to_string().as_str()),
        )
        .mount(&img_server)
        .await;
    // GET range for dimension probe
    Mock::given(method("GET"))
        .and(path("/photo.png"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-type", "image/png")
                .set_body_bytes(PROBE_PNG.to_vec()),
        )
        .mount(&img_server)
        .await;

    // ------------------------------------------------------------------
    // 3. Start the HTML content server.
    //    The img has width/height = 1 in the image bytes; we disable the
    //    dimension gate in config (min_width = 0, min_height = 0) so the
    //    1x1 PNG passes.
    // ------------------------------------------------------------------
    let html_server = MockServer::start().await;
    let img_url = format!("{}/photo.png", img_server.uri());
    let html_body = format!(
        "<html><head><title>Caption Override Test</title></head><body>\
         <article>\
         <h1>Caption Override Test</h1>\
         <p>This article contains an image that should be captioned by the override captioner.</p>\
         <p><img src=\"{img_url}\" alt=\"test image\"></p>\
         <p>There is enough content here for readabilityrs to identify this as a valid article \
         and include the image in the extracted markdown output for further processing.</p>\
         </article>\
         </body></html>"
    );
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(html_body),
        )
        .mount(&html_server)
        .await;

    // ------------------------------------------------------------------
    // 4. Build config with both captioners; default = alpha.
    //    Disable the dimension gate so the 1x1 PNG passes.
    //    max_per_page = 1 to allow one caption per page.
    // ------------------------------------------------------------------
    let alpha_base = format!("{}/v1/", alpha_server.uri());
    let beta_base = format!("{}/v1/", beta_server.uri());

    let config_toml = format!(
        r#"
[robots]
respect = false

[ssrf]
level = "loopback"

[image_captions]
default = "alpha"
min_width = 0
min_height = 0
max_per_page = 1
max_tokens = 50

[captioners.alpha]
kind = "cloud"
provider = "openai_compat"
base_url = "{alpha_base}"
model = "test-model"
api_key_env = "ROVER_TEST_ALPHA_KEY"

[captioners.beta]
kind = "cloud"
provider = "openai_compat"
base_url = "{beta_base}"
model = "test-model"
api_key_env = "ROVER_TEST_BETA_KEY"
"#
    );

    // ------------------------------------------------------------------
    // 5. Set API key env vars (child inherits them).
    // ------------------------------------------------------------------
    // SAFETY: integration tests each run in their own process.
    unsafe {
        std::env::set_var("ROVER_TEST_ALPHA_KEY", "alpha-key");
        std::env::set_var("ROVER_TEST_BETA_KEY", "beta-key");
    }

    // ------------------------------------------------------------------
    // 6. Spawn the MCP child and call fetch with captioner = "beta".
    // ------------------------------------------------------------------
    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    let client = spawn_client_with_config(tmp.path(), &config_toml).await;

    let url = format!("{}/article", html_server.uri());
    let mut params = CallToolRequestParams::new("fetch_tool".to_string());
    let args = json!({
        "url": url,
        "images": {
            "mode": "caption",
            "captioner": "beta"
        }
    });
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }

    let res = client
        .call_tool(params)
        .await
        .expect("fetch_tool succeeded");
    assert!(
        !res.is_error.unwrap_or(false),
        "fetch_tool returned an error: {res:?}"
    );

    // ------------------------------------------------------------------
    // 7. Assert beta's caption appears in the response markdown.
    // ------------------------------------------------------------------
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("beta says hi"),
        "expected 'beta says hi' in response (beta captioner should have been used): {blob}"
    );
    assert!(
        !blob.contains("alpha says hi"),
        "unexpected 'alpha says hi' in response (alpha should NOT have been used): {blob}"
    );

    // ------------------------------------------------------------------
    // 8. Verify request counts: beta got the request, alpha got none.
    // ------------------------------------------------------------------
    let beta_reqs = beta_server.received_requests().await.unwrap();
    assert!(
        !beta_reqs.is_empty(),
        "beta captioner should have received at least one request"
    );

    let alpha_reqs = alpha_server.received_requests().await.unwrap();
    assert!(
        alpha_reqs.is_empty(),
        "alpha captioner should NOT have received any requests (override was beta): {alpha_reqs:?}"
    );

    client.cancel().await.unwrap();
    drop(html_server);
    drop(img_server);
    drop(alpha_server);
    drop(beta_server);
}

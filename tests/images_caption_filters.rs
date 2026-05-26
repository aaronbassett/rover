//! Caption filter pipeline (dimensions, size, budget) end-to-end through
//! the extractor::images::apply call path.

use std::sync::Arc;

use async_trait::async_trait;
use rover::extractor::images::apply;
use rover::extractor::options::{ImageCaptionFilters, ImagesMode};
use rover::extractor::output::OutputPaths;
use rover::vlm::{CaptionerRegistry, VlmCaptioner, VlmError};

/// Build a `reqwest::Client` that the test pipeline can drive. Goes through
/// `rover::fetcher::client::build_http_client` so the ring-rustls provider is
/// installed before the client is constructed (otherwise reqwest panics with
/// "No provider set" — see commit a262972).
fn test_client() -> reqwest::Client {
    rover::fetcher::client::build_http_client("rover-test/0.1", std::time::Duration::from_secs(5))
}

/// A captioner that always succeeds with a fixed string. Used to focus
/// these tests on the filter pipeline, not on captioner behavior.
struct AlwaysCaption(String);

#[async_trait]
impl VlmCaptioner for AlwaysCaption {
    fn name(&self) -> &str {
        "test"
    }
    fn model_id(&self) -> &str {
        "test-model"
    }
    async fn caption(
        &self,
        _image_bytes: &[u8],
        _alt: Option<&str>,
        _max_tokens: usize,
    ) -> Result<String, VlmError> {
        Ok(self.0.clone())
    }
}

fn registry_with(cap: Arc<dyn VlmCaptioner>) -> CaptionerRegistry {
    let mut m = std::collections::HashMap::new();
    m.insert("test".to_string(), cap);
    CaptionerRegistry::__test_construct(m, Some("test".into()))
}

fn paths() -> OutputPaths {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    // SAFETY: integration tests each run in a separate process.
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
    OutputPaths::resolve(None).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn below_min_dimensions_skipped() {
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let md = "![icon](https://example.com/icon.svg \"\" width=\"24\" height=\"24\")";
    let filters = ImageCaptionFilters {
        min_width: 200,
        min_height: 200,
        ..Default::default()
    };
    let client = test_client();
    let p = paths();
    let r = apply(
        md,
        &ImagesMode::Caption,
        &p,
        &client,
        Some(&reg),
        &filters,
        None,
    )
    .await
    .unwrap();
    let proc = r.images_processed.first().expect("annotation");
    assert_eq!(proc.decision, "skipped");
    assert_eq!(proc.reason.as_deref(), Some("below_min_dimensions"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn above_max_bytes_skipped() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    // HEAD returns Content-Length = 12 MiB
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "12582912"))
        .mount(&server)
        .await;
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let url = format!("{}/hero.jpg", server.uri());
    let md = format!("![hero]({url} \"\" width=\"800\" height=\"600\")");
    let filters = ImageCaptionFilters {
        max_bytes: 10 * 1024 * 1024,
        ..Default::default()
    };
    let client = test_client();
    let p = paths();
    let r = apply(
        &md,
        &ImagesMode::Caption,
        &p,
        &client,
        Some(&reg),
        &filters,
        None,
    )
    .await
    .unwrap();
    assert_eq!(r.images_processed[0].decision, "skipped");
    assert_eq!(
        r.images_processed[0].reason.as_deref(),
        Some("above_max_bytes")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_page_budget_respected() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "1000"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(&[0u8; 1000][..]))
        .mount(&server)
        .await;
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let url = format!("{}/img.png", server.uri());
    let md_lines: Vec<String> = (0..15)
        .map(|i| format!("![n{i}]({url}?i={i} \"\" width=\"500\" height=\"500\")"))
        .collect();
    let md = md_lines.join("\n");
    let filters = ImageCaptionFilters {
        max_per_page: 3,
        ..Default::default()
    };
    let client = test_client();
    let p = paths();
    let r = apply(
        &md,
        &ImagesMode::Caption,
        &p,
        &client,
        Some(&reg),
        &filters,
        None,
    )
    .await
    .unwrap();
    let captioned = r
        .images_processed
        .iter()
        .filter(|x| x.decision == "captioned")
        .count();
    let skipped_budget = r
        .images_processed
        .iter()
        .filter(|x| x.reason.as_deref() == Some("per_page_budget"))
        .count();
    assert_eq!(captioned, 3);
    assert_eq!(skipped_budget, 12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dimension_probe_via_partial_fetch() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    // 200x200 PNG header — sufficient to pass the min-dimensions check.
    let png_header_200x200: [u8; 33] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0xc8, 0x00, 0x00, 0x00, 0xc8, // width=200, height=200
        0x08, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(&png_header_200x200[..]))
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "5000"))
        .mount(&server)
        .await;

    let reg = registry_with(Arc::new(AlwaysCaption("captioned!".into())));
    let url = format!("{}/photo.png", server.uri());
    let md = format!("![photo]({url})"); // no width/height attrs
    let filters = ImageCaptionFilters::default();
    let client = test_client();
    let p = paths();
    let r = apply(
        &md,
        &ImagesMode::Caption,
        &p,
        &client,
        Some(&reg),
        &filters,
        None,
    )
    .await
    .unwrap();
    assert_eq!(r.images_processed[0].decision, "captioned");
    assert!(r.markdown.contains("captioned!"));
}

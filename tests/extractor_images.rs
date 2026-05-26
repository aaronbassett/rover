mod common;

use rover::extractor::images;
use rover::extractor::options::{ImageCaptionFilters, ImagesMode};
use rover::extractor::output::OutputPaths;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/m4/small-image-pixel.png"),
    )
    .unwrap()
}

#[tokio::test]
async fn download_writes_to_disk_and_rewrites_markdown() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(png_bytes()),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: each integration test is its own process; env var setting is safe.
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
    let paths = OutputPaths::resolve(None).unwrap();

    let md = format!("![pixel]({}/pixel.png)", server.uri());
    // Rover's ring-rustls switch (commit a262972) means a freshly-built
    // `reqwest::Client::new()` panics with "No provider set" unless the
    // ring provider has been installed for the process. The helper is a
    // `OnceLock`-guarded idempotent install.
    rover::fetcher::client::install_ring_provider();
    let client = reqwest::Client::new();
    let filters = ImageCaptionFilters::default();
    let r = images::apply(
        &md,
        &ImagesMode::Download,
        &paths,
        &client,
        None,
        &filters,
        None,
    )
    .await
    .unwrap();

    assert_eq!(r.images_seen, 1);
    assert_eq!(r.images_downloaded, 1);
    assert_eq!(r.images_failed, 0);
    // The rewritten markdown should point at a local file under tmp.
    assert!(
        r.markdown.contains(tmp.path().to_str().unwrap()),
        "got: {}",
        r.markdown
    );
}

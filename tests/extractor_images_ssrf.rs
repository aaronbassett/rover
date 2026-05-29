//! End-to-end check that `extractor::images::apply` carries the active
//! `SsrfLevel` into the dial-time resolver. Before this, the image-fetch
//! helpers issued un-scoped requests, so SSRF policy never reached them
//! (the gap noted in `docs/security.md` §"DNS rebinding").
//!
//! `apply` swallows per-image download failures (it logs and keeps the
//! original markdown), so we assert behaviourally: a loopback image under
//! `Strict` fails to download, while the same image under `Loopback`
//! succeeds. The precise `DialBlocked` type assertion lives in the
//! `download_one_blocks_loopback_under_strict` unit test.

use std::time::Duration;

use rover::extractor::images;
use rover::extractor::options::{ImageCaptionFilters, ImagesMode};
use rover::extractor::output::OutputPaths;
use rover::fetcher::client::build_http_client;
use rover::fetcher::ssrf::SsrfLevel;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/m4/small-image-pixel.png"),
    )
    .unwrap()
}

async fn run_download(level: SsrfLevel) -> images::ImagesApplied {
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
    // SAFETY: each integration test is its own process.
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
    let paths = OutputPaths::resolve(None).unwrap();

    // The wiremock server binds to a loopback address. Only an SSRF-aware
    // client (built via `build_http_client`) installs the validating
    // resolver that consults `SSRF_LEVEL`.
    let client = build_http_client("rover-test/0.1", Duration::from_secs(5));
    let md = format!("![pixel]({}/pixel.png)", server.uri());
    let filters = ImageCaptionFilters::default();
    images::apply(
        &md,
        &ImagesMode::Download,
        &paths,
        &client,
        None,
        &filters,
        None,
        level,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn download_mode_blocks_loopback_under_strict() {
    let r = run_download(SsrfLevel::Strict).await;
    assert_eq!(r.images_seen, 1);
    assert_eq!(
        r.images_downloaded, 0,
        "strict must block the loopback dial"
    );
    assert_eq!(r.images_failed, 1);
}

#[tokio::test]
async fn download_mode_permits_loopback_under_loopback_level() {
    let r = run_download(SsrfLevel::Loopback).await;
    assert_eq!(r.images_seen, 1);
    assert_eq!(
        r.images_downloaded, 1,
        "loopback level must permit the loopback dial"
    );
    assert_eq!(r.images_failed, 0);
}

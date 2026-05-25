//! Smoketest for the local-vision feature. Loads a real SmolVLM model and
//! captions a tiny solid-color image. `#[ignore]` by default — opt in via
//! `cargo test --features local-vision -- --ignored`. CI: smoketest workflow
//! runs these nightly.

#![cfg(feature = "local-vision")]

use rover::vlm::VlmCaptioner;
use rover::vlm::local::MistralRsCaptioner;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn captions_solid_color_image() {
    let cap =
        MistralRsCaptioner::new("test", "HuggingFaceTB/SmolVLM-256M-Instruct", 2).expect("ctor");

    // 256x256 solid red PNG, generated at runtime via the `image` crate.
    let img = image::RgbImage::from_pixel(256, 256, image::Rgb([255, 0, 0]));
    let mut buf: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder;
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        encoder
            .write_image(&img, 256, 256, image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    let caption = cap
        .caption(&buf, Some("red square"), 50)
        .await
        .expect("caption ok");
    assert!(!caption.is_empty());
    assert!(caption.split_whitespace().count() >= 2, "got '{caption}'");
}

mod common;

use rover::extractor::pipeline::extract_full;
use std::path::Path;
use url::Url;

#[test]
fn base_href_overrides_document_url_for_link_resolution() {
    let html = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m4/with-base-href.html"),
    )
    .unwrap();
    let doc_url = Url::parse("https://example.com/page").unwrap();
    let doc = extract_full(&html, &doc_url).expect("extract");
    // Links should resolve against `https://other.example/`, NOT example.com.
    assert!(
        doc.body_md.contains("https://other.example/docs/intro"),
        "got: {}",
        doc.body_md
    );
    // No example.com links should appear in the body.
    assert!(
        !doc.body_md.contains("example.com/docs/intro"),
        "got: {}",
        doc.body_md
    );
}

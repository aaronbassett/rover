mod common;

use rover::extractor;
use url::Url;

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/m4")
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

fn base() -> Url {
    Url::parse("https://example.com/article").unwrap()
}

#[test]
fn jsonld_og_twitter_precedence() {
    let html = fixture("article-jsonld-og-twitter.html");
    let m = extractor::metadata::extract(&html, &base());
    // JSON-LD title wins
    assert!(m.title.as_deref().unwrap().contains("JSON-LD"));
    // JSON-LD provides Article in schema_types
    assert!(m.schema_types.iter().any(|t| t == "Article"));
    // og_type fills from OG
    assert_eq!(m.og_type.as_deref(), Some("article"));
}

#[test]
fn og_only_page_yields_og_fields() {
    let html = fixture("og-only.html");
    let m = extractor::metadata::extract(&html, &base());
    assert_eq!(m.title.as_deref(), Some("OG Only Title"));
    assert_eq!(m.og_type.as_deref(), Some("article"));
    assert_eq!(m.description.as_deref(), Some("Just OG."));
}

#[test]
fn no_metadata_page_yields_empty_or_just_lang() {
    let html = fixture("no-metadata.html");
    let m = extractor::metadata::extract(&html, &base());
    // No html[lang] in this fixture, no JSON-LD, no OG, no canonical → fully empty.
    assert!(m.is_empty());
}

//! YAML frontmatter envelope writer.
//!
//! Emits the M1 subset of PRD §6.2:
//!   - url
//!   - canonical_url (only when different from url)
//!   - title (when present)
//!   - fetched_at (RFC 3339, UTC)
//!   - content_hash (sha256:...)
//!   - estimated_tokens
//!   - tokenizer
//!
//! M4 expands this with metadata, language, schema_types, tables/images
//! transformations, etc. As of M3, real tokenizers compute `tokens` upstream
//! and pass it in via `PageMeta`; the writer no longer estimates.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

/// Per-image dimension pair carried alongside `ImageProcessed` annotations.
/// Task 11 will add `Serialize` and surface this in the frontmatter sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDims {
    pub width: u32,
    pub height: u32,
}

/// One row of the `images_processed:` frontmatter sidecar (M9). Each `<img>`
/// the caption pipeline observes produces one entry — either `"captioned"`
/// or `"skipped"` with a typed reason. Task 11 expands this with
/// `Serialize` derive and YAML rendering inside `render(&PageMeta)`.
#[derive(Debug, Clone)]
pub struct ImageProcessed {
    pub src: String,
    /// `"captioned"` or `"skipped"`.
    pub decision: String,
    /// Lowercased `SkipReason` variant when `decision == "skipped"`.
    pub reason: Option<String>,
    /// Captioner config-key name when the captioner was attempted.
    pub captioner: Option<String>,
    /// The generated caption when `decision == "captioned"`.
    pub caption: Option<String>,
    pub dimensions: Option<ImageDims>,
    /// Reported byte length (from Content-Length probe) when known.
    pub bytes: Option<u64>,
    /// Human-readable error when the captioner or download failed.
    pub error: Option<String>,
}

/// Inputs for the M4 frontmatter envelope.
pub struct PageMeta<'a> {
    pub url: &'a Url,
    pub canonical_url: &'a Url,
    pub title: Option<&'a str>,
    pub fetched_at: Timestamp,
    pub body: &'a str,
    /// Precomputed token count for `body`, in units of `tokenizer_name`.
    pub tokens: usize,
    /// Short tokenizer family name (e.g. `"o200k"`). Surfaced in the
    /// `tokenizer` frontmatter field so consumers know how `tokens` was
    /// measured.
    pub tokenizer_name: &'a str,
    // ---- M4 additions ----
    pub description: Option<&'a str>,
    pub author: Option<&'a str>,
    pub published: Option<&'a str>,
    pub modified: Option<&'a str>,
    pub image: Option<&'a str>,
    pub og_type: Option<&'a str>,
    pub language: Option<&'a str>,
    pub schema_types: &'a [String],
    pub extraction_quality: f32,
    pub tables_transformed: &'a [crate::extractor::tables::TableTransform],
    pub images_seen: usize,
    pub images_downloaded: usize,
    pub images_failed: usize,
}

/// Render `meta` as a frontmatter-envelope string followed by `body`.
pub fn render(meta: &PageMeta<'_>) -> String {
    let mut buf = String::with_capacity(meta.body.len() + 512);
    buf.push_str("---\n");

    write_field(&mut buf, "url", meta.url.as_str());
    if meta.canonical_url != meta.url {
        write_field(&mut buf, "canonical_url", meta.canonical_url.as_str());
    }
    if let Some(t) = meta.title {
        write_field(&mut buf, "title", t);
    }
    write_field(&mut buf, "fetched_at", &meta.fetched_at.to_string());

    let content_hash = sha256_hex(meta.body.as_bytes());
    let hash_field = format!("sha256:{content_hash}");
    write_field(&mut buf, "content_hash", &hash_field);

    buf.push_str(&format!("estimated_tokens: {}\n", meta.tokens));
    write_field(&mut buf, "tokenizer", meta.tokenizer_name);

    // M4 metadata fields — emit only when present.
    if let Some(v) = meta.description {
        write_field(&mut buf, "description", v);
    }
    if let Some(v) = meta.author {
        write_field(&mut buf, "author", v);
    }
    if let Some(v) = meta.published {
        write_field(&mut buf, "published", v);
    }
    if let Some(v) = meta.modified {
        write_field(&mut buf, "modified", v);
    }
    if let Some(v) = meta.image {
        write_field(&mut buf, "image", v);
    }
    if let Some(v) = meta.og_type {
        write_field(&mut buf, "og_type", v);
    }
    if let Some(v) = meta.language {
        write_field(&mut buf, "language", v);
    }
    if !meta.schema_types.is_empty() {
        buf.push_str("schema_types:\n");
        for s in meta.schema_types {
            buf.push_str("  - ");
            buf.push_str(&yaml_escape(s));
            buf.push('\n');
        }
    }
    buf.push_str(&format!(
        "extraction_quality: {:.2}\n",
        meta.extraction_quality
    ));
    if !meta.tables_transformed.is_empty() {
        buf.push_str("tables_transformed:\n");
        for t in meta.tables_transformed {
            buf.push_str(&format!(
                "  - ordinal: {}\n    mode: {}\n",
                t.ordinal, t.mode
            ));
            if let Some(p) = &t.path {
                buf.push_str(&format!("    path: {:?}\n", p.display().to_string()));
            }
            if let Some(k) = t.kept_rows {
                buf.push_str(&format!("    kept_rows: {k}\n"));
            }
            if let Some(tr) = t.truncated_rows {
                buf.push_str(&format!("    truncated_rows: {tr}\n"));
            }
        }
    }
    if meta.images_seen > 0 {
        buf.push_str(&format!("images_seen: {}\n", meta.images_seen));
    }
    if meta.images_downloaded > 0 {
        buf.push_str(&format!("images_downloaded: {}\n", meta.images_downloaded));
    }
    if meta.images_failed > 0 {
        buf.push_str(&format!("images_failed: {}\n", meta.images_failed));
    }

    buf.push_str("---\n\n");
    buf.push_str(meta.body);
    if !meta.body.ends_with('\n') {
        buf.push('\n');
    }
    buf
}

/// Quote a YAML scalar when it contains characters that would break a
/// plain-style emission (quotes, colons, line breaks) or has surrounding
/// whitespace. Plain ASCII strings pass through unquoted.
fn yaml_escape(s: &str) -> String {
    let needs_quote = s.contains(['"', ':', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ');
    if needs_quote {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' => out.push_str(r"\\"),
                '"' => out.push_str(r#"\""#),
                '\n' => out.push_str(r"\n"),
                '\r' => out.push_str(r"\r"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        s.to_string()
    }
}

/// Emit one scalar field. Strings are double-quoted with backslash-escaping
/// applied to `"` and `\` so any title content survives intact.
fn write_field(buf: &mut String, key: &str, value: &str) {
    buf.push_str(key);
    buf.push_str(": ");
    buf.push('"');
    for c in value.chars() {
        match c {
            '\\' => buf.push_str(r"\\"),
            '"' => buf.push_str(r#"\""#),
            '\n' => buf.push_str(r"\n"),
            '\r' => buf.push_str(r"\r"),
            '\t' => buf.push_str(r"\t"),
            _ => buf.push(c),
        }
    }
    buf.push('"');
    buf.push('\n');
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;

    fn ts() -> Timestamp {
        "2026-05-07T12:34:56Z".parse().unwrap()
    }
    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn meta<'a>(url: &'a Url, body: &'a str) -> PageMeta<'a> {
        PageMeta {
            url,
            canonical_url: url,
            title: Some("Sample"),
            fetched_at: ts(),
            body,
            tokens: 7,
            tokenizer_name: "o200k",
            description: None,
            author: None,
            published: None,
            modified: None,
            image: None,
            og_type: None,
            language: None,
            schema_types: &[],
            extraction_quality: 0.50,
            tables_transformed: &[],
            images_seen: 0,
            images_downloaded: 0,
            images_failed: 0,
        }
    }

    #[test]
    fn emits_required_fields() {
        let url = u("https://example.com/page");
        let body = "# Title\n\nBody.\n";
        let out = render(&meta(&url, body));

        assert!(out.starts_with("---\n"));
        assert!(out.contains(r#"url: "https://example.com/page""#));
        assert!(out.contains(r#"title: "Sample""#));
        assert!(out.contains(r#"fetched_at: "2026-05-07T12:34:56Z""#));
        assert!(out.contains("content_hash: \"sha256:"));
        assert!(out.contains("estimated_tokens: 7"));
        assert!(out.contains(r#"tokenizer: "o200k""#));
        assert!(out.ends_with(body));
    }

    #[test]
    fn omits_canonical_when_same_as_url() {
        let url = u("https://example.com/page");
        let out = render(&PageMeta {
            title: None,
            ..meta(&url, "x")
        });
        assert!(!out.contains("canonical_url"));
    }

    #[test]
    fn includes_canonical_when_different() {
        let url = u("https://example.com/page?utm=1");
        let canon = u("https://example.com/page");
        let out = render(&PageMeta {
            canonical_url: &canon,
            title: None,
            ..meta(&url, "x")
        });
        assert!(out.contains(r#"canonical_url: "https://example.com/page""#));
    }

    #[test]
    fn quotes_in_title_are_escaped() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            title: Some(r#"He said "hi""#),
            ..meta(&url, "x")
        });
        assert!(out.contains(r#"title: "He said \"hi\"""#));
    }

    #[test]
    fn content_hash_is_deterministic() {
        let url = u("https://example.com/p");
        let body = "stable body";
        let a = render(&meta(&url, body));
        let b = render(&meta(&url, body));
        assert_eq!(a, b);
    }

    #[test]
    fn token_count_is_passed_through_verbatim() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            tokens: 1234,
            ..meta(&url, "hello")
        });
        assert!(out.contains("estimated_tokens: 1234"));
    }

    #[test]
    fn body_terminates_with_newline() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            title: None,
            ..meta(&url, "no trailing newline")
        });
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn emits_extraction_quality() {
        let url = Url::parse("https://example.com/p").unwrap();
        let out = render(&meta(&url, "body"));
        assert!(out.contains("extraction_quality: 0.50"));
    }

    #[test]
    fn omits_empty_optional_fields() {
        let url = Url::parse("https://example.com/p").unwrap();
        let out = render(&meta(&url, "body"));
        assert!(!out.contains("description:"));
        assert!(!out.contains("schema_types:"));
        assert!(!out.contains("tables_transformed:"));
        assert!(!out.contains("images_seen:"));
    }

    #[test]
    fn emits_metadata_fields_when_present() {
        let url = Url::parse("https://example.com/p").unwrap();
        let schema_types = vec!["Article".to_string(), "WebPage".to_string()];
        let m = PageMeta {
            description: Some("desc"),
            author: Some("Ada"),
            schema_types: &schema_types,
            ..meta(&url, "body")
        };
        let out = render(&m);
        assert!(out.contains(r#"description: "desc""#));
        assert!(out.contains(r#"author: "Ada""#));
        assert!(out.contains("schema_types:"));
        assert!(out.contains("  - Article"));
        assert!(out.contains("  - WebPage"));
    }
}

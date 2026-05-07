//! YAML frontmatter envelope writer.
//!
//! Emits the M1 subset of PRD §6.2:
//!   - url
//!   - canonical_url (only when different from url)
//!   - title (when present)
//!   - fetched_at (RFC 3339, UTC)
//!   - content_hash (sha256:...)
//!   - estimated_tokens
//!
//! M4 expands this with metadata, language, schema_types, tables/images
//! transformations, etc. M3 swaps the token estimator for real tokenizers.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use url::Url;

/// Inputs for the M1 frontmatter envelope.
pub struct PageMeta<'a> {
    pub url: &'a Url,
    pub canonical_url: &'a Url,
    pub title: Option<&'a str>,
    pub fetched_at: Timestamp,
    pub body: &'a str,
}

/// Render `meta` as a frontmatter-envelope string followed by `body`.
pub fn render(meta: &PageMeta<'_>) -> String {
    let mut buf = String::with_capacity(meta.body.len() + 256);
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

    let est = estimate_tokens(meta.body);
    buf.push_str(&format!("estimated_tokens: {est}\n"));

    buf.push_str("---\n\n");
    buf.push_str(meta.body);
    if !meta.body.ends_with('\n') {
        buf.push('\n');
    }
    buf
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
            '"'  => buf.push_str(r#"\""#),
            '\n' => buf.push_str(r"\n"),
            '\r' => buf.push_str(r"\r"),
            '\t' => buf.push_str(r"\t"),
            _    => buf.push(c),
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
    for b in out { s.push_str(&format!("{b:02x}")); }
    s
}

/// **M1 placeholder.** chars/4. M3 replaces this with real tokenizers via a
/// trait so the call site here doesn't change.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;

    fn ts() -> Timestamp { "2026-05-07T12:34:56Z".parse().unwrap() }
    fn u(s: &str) -> Url { Url::parse(s).unwrap() }

    #[test]
    fn emits_required_fields() {
        let url = u("https://example.com/page");
        let body = "# Title\n\nBody.\n";
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: Some("Sample"),
            fetched_at: ts(),
            body,
        });

        assert!(out.starts_with("---\n"));
        assert!(out.contains(r#"url: "https://example.com/page""#));
        assert!(out.contains(r#"title: "Sample""#));
        assert!(out.contains(r#"fetched_at: "2026-05-07T12:34:56Z""#));
        assert!(out.contains("content_hash: \"sha256:"));
        assert!(out.contains("estimated_tokens: "));
        assert!(out.ends_with(body));
    }

    #[test]
    fn omits_canonical_when_same_as_url() {
        let url = u("https://example.com/page");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: None,
            fetched_at: ts(),
            body: "x",
        });
        assert!(!out.contains("canonical_url"));
    }

    #[test]
    fn includes_canonical_when_different() {
        let url = u("https://example.com/page?utm=1");
        let canon = u("https://example.com/page");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &canon,
            title: None,
            fetched_at: ts(),
            body: "x",
        });
        assert!(out.contains(r#"canonical_url: "https://example.com/page""#));
    }

    #[test]
    fn quotes_in_title_are_escaped() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta {
            url: &url,
            canonical_url: &url,
            title: Some(r#"He said "hi""#),
            fetched_at: ts(),
            body: "x",
        });
        assert!(out.contains(r#"title: "He said \"hi\"""#));
    }

    #[test]
    fn content_hash_is_deterministic() {
        let url = u("https://example.com/p");
        let body = "stable body";
        let a = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body });
        let b = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body });
        assert_eq!(a, b);
    }

    #[test]
    fn estimate_tokens_chars_div_4_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("aaaa"), 1);
        assert_eq!(estimate_tokens("aaaaa"), 2);
    }

    #[test]
    fn body_terminates_with_newline() {
        let url = u("https://example.com/p");
        let out = render(&PageMeta { url: &url, canonical_url: &url, title: None, fetched_at: ts(), body: "no trailing newline" });
        assert!(out.ends_with('\n'));
    }
}

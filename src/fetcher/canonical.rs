//! Canonical URL extraction (PRD §5.2).
//!
//! Resolution order:
//!   1. HTML `<link rel="canonical" href="...">` in `<head>`
//!   2. HTTP `Link: <...>; rel="canonical"` header
//!   3. Final URL after redirects (the request's final response URL)
//!
//! The PRD warns that stale `rel=canonical` exists in the wild; we still
//! return whatever the source claims and let upstream code decide whether to
//! validate. M1 returns the claimed URL without further checks.

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

/// Extract a canonical URL from the response.
///
/// `final_url` is the URL after all redirects. `html` is the decoded body.
/// `link_header` is the raw `Link:` header value, if any.
pub fn extract_canonical_url(html: &str, final_url: &Url, link_header: Option<&str>) -> Url {
    if let Some(url) = canonical_from_html(html, final_url) {
        return url;
    }
    if let Some(header) = link_header
        && let Some(url) = canonical_from_link_header(header, final_url)
    {
        return url;
    }
    final_url.clone()
}

fn canonical_from_html(html: &str, base: &Url) -> Option<Url> {
    static SEL: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse(r#"link[rel~="canonical"][href]"#).unwrap());
    let doc = Html::parse_document(html);
    let el = doc.select(&SEL).next()?;
    let href = el.value().attr("href")?;
    base.join(href).ok()
}

/// Parse RFC 8288 `Link` header values, looking for `rel="canonical"`.
///
/// We accept multiple comma-separated link-values and ignore unrelated rels.
fn canonical_from_link_header(header: &str, base: &Url) -> Option<Url> {
    for value in split_link_values(header) {
        let value = value.trim();
        let (target, params) = match value.split_once(';') {
            Some((t, p)) => (t.trim(), p),
            None => (value, ""),
        };
        let target = target.trim_start_matches('<').trim_end_matches('>');
        // Look for rel="canonical" in params, case-insensitive on `rel`.
        for raw_param in params.split(';') {
            let p = raw_param.trim();
            if let Some(rest) = strip_prefix_ci(p, "rel=") {
                let rest = rest.trim_matches('"');
                if rest
                    .split_whitespace()
                    .any(|tok| tok.eq_ignore_ascii_case("canonical"))
                {
                    return base.join(target).ok();
                }
            }
        }
    }
    None
}

/// Split a Link header on top-level commas (commas inside `<...>` or `"..."`
/// don't count).
fn split_link_values(header: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = header.as_bytes();
    let mut start = 0usize;
    let mut depth_angle = 0i32;
    let mut in_quote = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'<' if !in_quote => depth_angle += 1,
            b'>' if !in_quote => depth_angle -= 1,
            b'"' => in_quote = !in_quote,
            b',' if !in_quote && depth_angle == 0 => {
                out.push(&header[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < header.len() {
        out.push(&header[start..]);
    }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    if s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn returns_final_url_when_no_signal() {
        let final_url = url("https://example.com/page?utm=x");
        let got = extract_canonical_url("<html></html>", &final_url, None);
        assert_eq!(got, final_url);
    }

    #[test]
    fn extracts_from_html_link_canonical() {
        let html =
            r#"<html><head><link rel="canonical" href="https://example.com/page"></head></html>"#;
        let got = extract_canonical_url(html, &url("https://example.com/page?utm=x"), None);
        assert_eq!(got, url("https://example.com/page"));
    }

    #[test]
    fn extracts_from_html_relative_canonical() {
        let html = r#"<html><head><link rel="canonical" href="/page"></head></html>"#;
        let got = extract_canonical_url(html, &url("https://example.com/page?utm=x"), None);
        assert_eq!(got, url("https://example.com/page"));
    }

    #[test]
    fn html_canonical_preferred_over_link_header() {
        let html = r#"<html><head><link rel="canonical" href="https://example.com/from-html"></head></html>"#;
        let got = extract_canonical_url(
            html,
            &url("https://example.com/x"),
            Some(r#"<https://example.com/from-header>; rel="canonical""#),
        );
        assert_eq!(got, url("https://example.com/from-html"));
    }

    #[test]
    fn extracts_from_link_header_when_no_html() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/canon>; rel="canonical""#),
        );
        assert_eq!(got, url("https://example.com/canon"));
    }

    #[test]
    fn link_header_with_multiple_rels() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(
                r#"<https://example.com/p>; rel="prev", <https://example.com/c>; rel="canonical""#,
            ),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn link_header_rel_case_insensitive() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/c>; REL="Canonical""#),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn link_header_with_compound_rel() {
        let got = extract_canonical_url(
            "<html></html>",
            &url("https://example.com/x"),
            Some(r#"<https://example.com/c>; rel="alternate canonical""#),
        );
        assert_eq!(got, url("https://example.com/c"));
    }

    #[test]
    fn falls_back_when_link_header_has_no_canonical() {
        let final_url = url("https://example.com/x");
        let got = extract_canonical_url(
            "<html></html>",
            &final_url,
            Some(r#"<https://example.com/p>; rel="prev""#),
        );
        assert_eq!(got, final_url);
    }
}

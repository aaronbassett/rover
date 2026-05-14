//! Peek `<base href>` from raw HTML before readabilityrs touches it.
//!
//! Per HTML5, `<base>` belongs in `<head>` and the first one wins. We
//! return `None` for relative `<base href>` values (rare, but possible
//! against the document URL) — the caller falls back to the final URL
//! after redirects.

use scraper::{Html, Selector};
use url::Url;

pub fn read_base_href(html: &str) -> Option<Url> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("head > base[href]").ok()?;
    let first = doc.select(&selector).next()?;
    let href = first.value().attr("href")?.trim();
    if href.is_empty() {
        return None;
    }
    Url::parse(href).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_base_href_parsed() {
        let html = r#"<!doctype html><html><head>
            <base href="https://x.example/articles/">
            </head><body></body></html>"#;
        assert_eq!(
            read_base_href(html).map(|u| u.to_string()),
            Some("https://x.example/articles/".to_string())
        );
    }

    #[test]
    fn missing_base_returns_none() {
        let html = "<!doctype html><html><head></head><body></body></html>";
        assert_eq!(read_base_href(html), None);
    }

    #[test]
    fn relative_base_returns_none() {
        let html = r#"<!doctype html><html><head>
            <base href="/articles/">
            </head><body></body></html>"#;
        assert_eq!(read_base_href(html), None);
    }

    #[test]
    fn first_base_wins() {
        let html = r#"<!doctype html><html><head>
            <base href="https://first.example/">
            <base href="https://second.example/">
            </head><body></body></html>"#;
        assert_eq!(
            read_base_href(html).map(|u| u.to_string()),
            Some("https://first.example/".to_string())
        );
    }
}

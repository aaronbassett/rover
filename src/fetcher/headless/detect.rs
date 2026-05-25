//! SPA detection heuristics (PRD §5.7 / spec §3.7).
//!
//! Used by `HeadlessMode::Auto` to decide whether the reqwest result is
//! good enough or whether to re-render via headless. Returns a
//! `HitCount`; the caller compares `hits.total >= 2`.

use std::sync::LazyLock;

use regex::Regex;

const SPA_MARKERS: &[&str] = &[
    r#"<div id="root""#,
    r#"<div id='root'"#,
    r#"<div id="app""#,
    r#"<div id='app'"#,
    r#"<div id="__next""#,
    "__NEXT_DATA__",
    "__NUXT__",
    "window.__INITIAL_STATE__",
];

static JS_REQUIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(javascript|enable js|js required|requires javascript)").unwrap()
});

static NOSCRIPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<noscript>(.*?)</noscript>").unwrap());

static SCRIPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script[^>]*>(.*?)</script>").unwrap());

static HREF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<a\s+[^>]*href\s*=\s*["']([^"']+)["']"#).unwrap());

#[derive(Debug, Clone, Copy)]
pub struct HitCount {
    pub short_extraction: bool,
    pub spa_marker: bool,
    pub high_script_ratio: bool,
    pub only_anchor_links: bool,
    pub noscript_js_required: bool,
    pub total: usize,
}

pub fn detect_spa(html: &str, extracted_md: &str) -> HitCount {
    let short_extraction = extracted_md.chars().count() < 300;
    let spa_marker = SPA_MARKERS.iter().any(|m| html.contains(m));
    let high_script_ratio = script_ratio(html) > 0.5;
    let only_anchor_links = anchors_are_all_routes(html);
    let noscript_js_required = NOSCRIPT_RE
        .captures_iter(html)
        .any(|c| JS_REQUIRED_RE.is_match(&c[1]));

    let mut total = 0;
    if short_extraction {
        total += 1;
    }
    if spa_marker {
        total += 1;
    }
    if high_script_ratio {
        total += 1;
    }
    if only_anchor_links {
        total += 1;
    }
    if noscript_js_required {
        total += 1;
    }

    HitCount {
        short_extraction,
        spa_marker,
        high_script_ratio,
        only_anchor_links,
        noscript_js_required,
        total,
    }
}

fn script_ratio(html: &str) -> f64 {
    if html.is_empty() {
        return 0.0;
    }
    let total = html.len() as f64;
    let script: usize = SCRIPT_RE
        .captures_iter(html)
        .map(|c| c.get(1).map(|m| m.as_str().len()).unwrap_or(0))
        .sum();
    script as f64 / total
}

fn anchors_are_all_routes(html: &str) -> bool {
    let mut total = 0usize;
    let mut routey = 0usize;
    for c in HREF_RE.captures_iter(html) {
        let href = &c[1];
        total += 1;
        if href.starts_with("#/") || href.starts_with("javascript:") {
            routey += 1;
        }
    }
    // Only true when there's at least one anchor AND every one is a route.
    total > 0 && routey == total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_html_yields_short_extraction_only() {
        let h = detect_spa("", "");
        assert!(h.short_extraction);
        assert!(!h.spa_marker);
        // Both extraction-short and noscript_js_required false; total = 1.
        assert_eq!(h.total, 1);
    }

    #[test]
    fn react_root_marker_detected() {
        let html = r#"<html><body><div id="root"></div></body></html>"#;
        let h = detect_spa(html, "");
        assert!(h.spa_marker);
        assert!(h.short_extraction);
        // High script ratio: 0 (no scripts).
        assert!(!h.high_script_ratio);
        assert!(h.total >= 2);
    }

    #[test]
    fn noscript_js_required_detected() {
        let html = "<html><body><noscript>Please enable JavaScript to view this page.</noscript></body></html>";
        let h = detect_spa(
            html,
            "rich extracted markdown here, more than 300 chars long ... \
                                  lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
                                  eiusmod tempor incididunt ut labore et dolore magna aliqua. ut enim \
                                  ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut \
                                  aliquip ex ea commodo consequat. extra padding to reach 300.",
        );
        assert!(h.noscript_js_required);
        assert!(!h.short_extraction);
    }

    #[test]
    fn high_script_ratio_detected() {
        // script content (50 'a's) exceeds 50% of total html length.
        let html = "<html><body><script>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</script>x</body></html>";
        let h = detect_spa(html, "");
        assert!(h.high_script_ratio);
    }

    #[test]
    fn anchor_routes_only_detected() {
        let html = r##"<a href="#/home">x</a> <a href="#/about">y</a>"##;
        let h = detect_spa(html, "");
        assert!(h.only_anchor_links);
    }

    #[test]
    fn mixed_anchors_not_routes_only() {
        let html = r##"<a href="#/home">x</a> <a href="https://example.com">real link</a>"##;
        let h = detect_spa(html, "");
        assert!(!h.only_anchor_links);
    }
}

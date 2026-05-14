//! Structured-metadata extraction (JSON-LD + Open Graph + Twitter Cards).
//!
//! JSON-LD walker flattens `@graph` arrays and nested objects up to depth
//! 8, picks the first node whose `@type` is in the "primary" set, and
//! surfaces its scalar fields. Task 4 adds OG, Twitter Cards, html[lang],
//! meta description, and canonical.

use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

const MAX_DEPTH: usize = 8;

const PRIMARY_TYPES: &[&str] = &[
    "Article",
    "NewsArticle",
    "BlogPosting",
    "WebPage",
    "Product",
];

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExtractedMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub published: Option<String>,
    pub modified: Option<String>,
    pub image: Option<String>,
    pub og_type: Option<String>,
    pub canonical: Option<String>,
    pub language: Option<String>,
    /// Schema.org `@type` values in first-seen order, deduplicated.
    pub schema_types: Vec<String>,
}

impl ExtractedMetadata {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.author.is_none()
            && self.published.is_none()
            && self.modified.is_none()
            && self.image.is_none()
            && self.og_type.is_none()
            && self.canonical.is_none()
            && self.language.is_none()
            && self.schema_types.is_empty()
    }

    /// Fill missing fields from `other`; existing fields are not overwritten.
    fn merge_in(&mut self, other: ExtractedMetadata) {
        if self.title.is_none() {
            self.title = other.title;
        }
        if self.description.is_none() {
            self.description = other.description;
        }
        if self.author.is_none() {
            self.author = other.author;
        }
        if self.published.is_none() {
            self.published = other.published;
        }
        if self.modified.is_none() {
            self.modified = other.modified;
        }
        if self.image.is_none() {
            self.image = other.image;
        }
        if self.og_type.is_none() {
            self.og_type = other.og_type;
        }
        if self.canonical.is_none() {
            self.canonical = other.canonical;
        }
        if self.language.is_none() {
            self.language = other.language;
        }
        for t in other.schema_types {
            if !self.schema_types.contains(&t) {
                self.schema_types.push(t);
            }
        }
    }
}

pub fn extract(html: &str, _base: &Url) -> ExtractedMetadata {
    let doc = Html::parse_document(html);
    let mut out = ExtractedMetadata::default();
    out.merge_in(extract_jsonld(&doc));
    // OG + Twitter + html[lang] + canonical land in Task 4.
    out
}

fn extract_jsonld(doc: &Html) -> ExtractedMetadata {
    let mut out = ExtractedMetadata::default();
    let selector = Selector::parse(r#"script[type="application/ld+json"]"#).unwrap();

    // Collect all @type values across the page; pick the primary node from the first script that has one.
    let mut nodes_with_type: Vec<Value> = Vec::new();
    let mut all_types: Vec<String> = Vec::new();

    for el in doc.select(&selector) {
        let text = el.text().collect::<String>();
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(target: "rover::extractor", err = %e, "malformed JSON-LD block; skipping");
                continue;
            }
        };
        walk(&value, 0, &mut nodes_with_type, &mut all_types);
    }

    // Pick primary node: prefer PRIMARY_TYPES order; else first node with any @type.
    let primary = pick_primary(&nodes_with_type);
    if let Some(node) = primary {
        out.title = scalar(node, "headline").or_else(|| scalar(node, "name"));
        out.description = scalar(node, "description");
        out.author = scalar_or_person_name(node, "author");
        out.published = scalar(node, "datePublished");
        out.modified = scalar(node, "dateModified");
        out.image = scalar_or_image_url(node, "image");
    }

    for t in all_types {
        if !out.schema_types.contains(&t) {
            out.schema_types.push(t);
        }
    }
    out
}

/// Walk a JSON-LD value tree collecting typed nodes and `@type` strings.
///
/// **Recursion policy (deliberate deviation from a naive recurse-everywhere
/// walk):** when an object has `@type`, we record it but only recurse into
/// `@graph` from there. Typed nodes' other properties (`author`, `publisher`,
/// `offers`, etc.) are NOT walked. This keeps `schema_types` focused on
/// page-level classifications rather than leaking nested referenced entities
/// (e.g. an Article's `author: Person` should not surface "Person" as a
/// page type). Untyped containers (the document root, untyped wrappers)
/// still recurse into all children.
///
/// `MAX_DEPTH` caps recursion to defend against pathological inputs.
fn walk(v: &Value, depth: usize, nodes: &mut Vec<Value>, all_types: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Object(map) => {
            let typed = map.get("@type").map(type_names).unwrap_or_default();
            if !typed.is_empty() {
                nodes.push(v.clone());
                for n in typed {
                    all_types.push(n);
                }
                // Don't recurse into a typed node's own properties — but DO follow
                // an explicit @graph if present (some payloads nest a graph inside
                // a typed wrapper).
                if let Some(graph) = map.get("@graph") {
                    walk(graph, depth + 1, nodes, all_types);
                }
            } else {
                // Untyped container: descend into all children (covers top-level
                // wrappers like `{"@context":..., "@graph":[...]}`).
                for (_k, child) in map {
                    walk(child, depth + 1, nodes, all_types);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, depth + 1, nodes, all_types);
            }
        }
        _ => {}
    }
}

fn type_names(t: &Value) -> Vec<String> {
    match t {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn pick_primary(nodes: &[Value]) -> Option<&Value> {
    for want in PRIMARY_TYPES {
        for n in nodes {
            if type_names(&n["@type"]).iter().any(|s| s == *want) {
                return Some(n);
            }
        }
    }
    nodes.first()
}

fn scalar(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn scalar_or_person_name(node: &Value, key: &str) -> Option<String> {
    let v = node.get(key)?;
    if let Some(s) = v.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    if let Some(obj) = v.as_object() {
        if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
            return Some(name.to_string());
        }
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(name) = item.as_str() {
                return Some(name.to_string());
            }
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn scalar_or_image_url(node: &Value, key: &str) -> Option<String> {
    let v = node.get(key)?;
    if let Some(s) = v.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    if let Some(obj) = v.as_object() {
        return obj.get("url").and_then(|u| u.as_str()).map(String::from);
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(s) = item.as_str() {
                return Some(s.to_string());
            }
            if let Some(u) = item.get("url").and_then(|u| u.as_str()) {
                return Some(u.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod jsonld_tests {
    use super::*;
    use url::Url;

    fn base() -> Url {
        Url::parse("https://example.com/article").unwrap()
    }

    const ARTICLE_HTML: &str = r#"<!doctype html><html><head>
        <script type="application/ld+json">
        {
          "@context": "https://schema.org",
          "@type": "Article",
          "headline": "Title from JSON-LD",
          "description": "Desc from JSON-LD",
          "author": {"@type":"Person","name":"Ada Lovelace"},
          "datePublished": "2026-01-01T00:00:00Z",
          "dateModified": "2026-02-01T00:00:00Z",
          "image": "https://example.com/og.png"
        }
        </script></head><body></body></html>"#;

    #[test]
    fn extracts_article_scalar_fields() {
        let m = extract(ARTICLE_HTML, &base());
        assert_eq!(m.title.as_deref(), Some("Title from JSON-LD"));
        assert_eq!(m.description.as_deref(), Some("Desc from JSON-LD"));
        assert_eq!(m.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(m.published.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(m.modified.as_deref(), Some("2026-02-01T00:00:00Z"));
        assert_eq!(m.image.as_deref(), Some("https://example.com/og.png"));
        assert_eq!(m.schema_types, vec!["Article".to_string()]);
    }

    const GRAPH_HTML: &str = r#"<!doctype html><html><head>
        <script type="application/ld+json">
        {"@context":"https://schema.org","@graph":[
            {"@type":"WebPage","name":"Should be skipped"},
            {"@type":"NewsArticle","headline":"News title","author":"Reuters"}
        ]}
        </script></head><body></body></html>"#;

    #[test]
    fn prefers_article_like_type_in_graph() {
        let m = extract(GRAPH_HTML, &base());
        assert_eq!(m.title.as_deref(), Some("News title"));
        assert_eq!(m.author.as_deref(), Some("Reuters"));
        // Both types appear in schema_types
        assert!(m.schema_types.contains(&"WebPage".to_string()));
        assert!(m.schema_types.contains(&"NewsArticle".to_string()));
    }

    #[test]
    fn depth_cap_does_not_stack_overflow() {
        // Build a 20-deep nested object inside @graph so the walker recurses through it.
        // The walker's MAX_DEPTH=8 cap prevents stack overflow.
        let mut chain = String::from(r#"{"@type":"Leaf"}"#);
        for _ in 0..20 {
            chain = format!(r#"{{"nested":{chain}}}"#);
        }
        let payload = format!(r#"{{"@graph":[{chain}]}}"#);
        let html = format!(
            r#"<!doctype html><html><head><script type="application/ld+json">{payload}</script></head><body></body></html>"#
        );
        let m = extract(&html, &base());
        // The walker stops at depth 8, so the deeply-nested Leaf is NOT reached.
        // The test verifies that hitting the cap doesn't panic/stack-overflow.
        assert!(
            m.schema_types.is_empty(),
            "expected cap to prevent deep walk, got {:?}",
            m.schema_types
        );
    }

    #[test]
    fn malformed_jsonld_does_not_panic() {
        let html = r#"<!doctype html><html><head>
            <script type="application/ld+json">{ this is not json }</script>
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert!(m.is_empty()); // soft-fail: empty contribution
    }
}

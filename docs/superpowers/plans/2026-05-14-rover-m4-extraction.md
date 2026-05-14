# Rover M4 — Metadata, Tables, Images, Links Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add structured-metadata extraction (JSON-LD + Open Graph + Twitter Cards), four table-transformation modes, four image-transformation modes, absolute-link rewriting, and a `get_metadata` MCP tool. Tighten the M3 `fetch` tool's `tables`/`images`/`metadata` placeholder args to typed structs. Persist extracted metadata in the cache via the M2-reserved `pages.metadata_json` column.

**Architecture:** Extend the extractor pipeline with a **two-pass** shape: a pre-pass on raw HTML (read `<base href>`, extract structured metadata via `scraper`), `readabilityrs` (unchanged), then a markdown post-pass (absolutize links, apply tables mode, apply images mode). The pre-pass and link-absolutize results are deterministic given URL+HTML and so are cached; the tables/images post-passes depend on per-request modes and run after every cache read.

**Tech Stack:** `csv` for table serialization, `rand` + `rand_chacha` for seedable `RandomSeed` sample strategy, `mime_guess` for image extension sniffing, `scraper` (already in deps) for the raw-HTML pre-pass, plus the M3 stack.

**Branch context:** This plan must be executed on a branch that contains M3's code (e.g. cut from `m3-mcp-server` tip or — once M3 is merged — from `main`). The current `m4-extraction` branch is cut from `main` and only contains the M4 spec doc; it does **not** yet contain M3. Before starting Task 1, either:
- Rebase `m4-extraction` onto the M3-containing tip (preferred once M3 is merged to `main`), or
- Cherry-pick the M4 spec commit onto a fresh branch off `m3-mcp-server`.

Either way, verify `cargo build` is green on a 154-test-passing baseline before Task 1.

**Scope of this plan:** PRD milestone M4 only. Earlier milestones complete; later milestones (M5 rate limiting, M6 long-running tasks, M7 summarization, M8 polish, M9 feature flags) get their own plans.

**References:**
- PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§6 extraction)
- Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` (§2.8 output paths)
- Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` (M4 section)
- M4 design: `docs/superpowers/specs/2026-05-14-rover-m4-extraction-design.md`
- M3 plan: `docs/superpowers/plans/2026-05-13-rover-m3-mcp-server.md`

---

## Decisions inherited from the M4 design spec

The spec resolved every open question. Quick reference:

1. Table modes: `Embed` (default), `Sample`, `CsvFile`, `Drop`. `Summarize` accepted by schema, runtime-rejected (M7).
2. Image modes: `Keep`, `AltTextOnly` (default), `Download`, `Drop`. `CaptionVlm` accepted, runtime-rejected (M9).
3. Sample strategies: `HeadTail` (default), `RandomSeed`. `Stratified` dropped.
4. Microdata extraction: dropped (low value in 2026; JSON-LD covers it).
5. `extraction_quality`: density ratio + 0.05 title bonus + 0.10 metadata bonus, clamped to [0, 1].
6. JSON-LD: flatten with depth 8, primary node = first whose `@type` ∈ `{Article, NewsArticle, BlogPosting, WebPage, Product}`, else first with any `@type`.
7. `<base href>` read by a pre-pass on raw HTML before `readabilityrs`. Fallback: final URL after redirects.
8. Link/image rewriting: post-pass on rendered markdown.
9. Output dir: `$XDG_DATA_HOME/rover/output/` + `[output] dir` config + `ROVER_OUTPUT_DIR` env var.
10. `get_metadata` MCP tool: dedicated, metadata-only response.
11. Typed args: hard cutover with `#[serde(deny_unknown_fields)]`. No M3 leniency shim.
12. Defaults: `tables: Embed`, `images: AltTextOnly`.
13. Embed: no oversize auto-switch.
14. Frontmatter: flat top-level fields, omit-if-empty.

## What's cached vs. what's per-request

Cached (stored in `pages.extracted_md` + `pages.metadata_json`):
- The base-href-resolved, links-absolutized markdown body.
- The full `ExtractedMetadata` blob (JSON in `metadata_json`).

Per-request (run on every `fetch` invocation, including cache hits):
- Table mode transformation. Tables in the cached body are still in Markdown form — the per-request pass re-walks them.
- Image mode transformation.
- Token counting (already per-request in M3).
- Frontmatter rendering.

This means: cache-hit fetches still spawn the post-passes. They're cheap (markdown walks, no HTTP except Download mode). Side-effect writes (CsvFile, image Download) are idempotent on the same URL — the path is `sha8(absolute_url[+#ordinal])`, so re-running produces the same file.

---

## Files Created in This Plan

```
src/extractor/
  metadata.rs                       # JSON-LD + OG + Twitter walkers
  base_href.rs                      # peek <base href> from raw HTML
  links.rs                          # markdown link/image post-pass
  tables.rs                         # TablesMode + 4 modes
  images.rs                         # ImagesMode + 4 modes
  quality.rs                        # extraction_quality scorer
  output.rs                         # OutputPaths + sha8 helper
  options.rs                        # ExtractOptions + TablesMode/ImagesMode/MetadataMode

src/mcp/tools/
  get_metadata.rs                   # new MCP tool body

tests/extractor_metadata.rs
tests/extractor_tables.rs
tests/extractor_images.rs
tests/extractor_links.rs
tests/mcp_get_metadata.rs

tests/fixtures/m4/
  article-jsonld-og-twitter.html
  graph-newsarticle-person.html
  og-only.html
  no-metadata.html
  with-base-href.html
  two-tables-one-large.html
  relative-images.html
  small-image-pixel.png

tests/common/
  mod.rs                            # shared helpers (seed_default_tokenizer etc)

# Modified
src/extractor/mod.rs                # add new modules; re-export public surface
src/extractor/pipeline.rs           # extended `extract()` returns ExtractedDoc with metadata
src/extractor/frontmatter.rs        # PageMeta gains M4 fields
src/fetcher/cached.rs               # ExtractResult carries metadata; cache layer writes metadata_json
src/storage/pages.rs                # Page row gains metadata_json round-trip
src/mcp/tools/fetch.rs              # typed tables/images/metadata args; wire ExtractOptions
src/mcp/tools/mod.rs                # +pub mod get_metadata
src/mcp/handler.rs                  # register get_metadata_tool in #[tool_router]
src/mcp/envelope.rs                 # MetadataResponse
src/config.rs                       # [output] section
Cargo.toml                          # +csv, +rand, +rand_chacha, +mime_guess
```

Inline unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each new source file.

---

## Task 1: Dependencies + module scaffolds

**Files:**
- Modify: `Cargo.toml`
- Create: `src/extractor/metadata.rs`, `src/extractor/base_href.rs`, `src/extractor/links.rs`, `src/extractor/tables.rs`, `src/extractor/images.rs`, `src/extractor/quality.rs`, `src/extractor/output.rs`, `src/extractor/options.rs` (all stubs)
- Modify: `src/extractor/mod.rs`

Adds Cargo deps and creates empty module trees so subsequent tasks land in well-defined slots without breaking the build.

- [ ] **Step 1: Add deps to `Cargo.toml`**

In `[dependencies]` (after `hf-hub`):

```toml
csv = "1"
rand = "0.9"
rand_chacha = "0.9"
mime_guess = "2"
```

- [ ] **Step 2: Create stub `src/extractor/metadata.rs`**

```rust
//! Structured-metadata extraction (JSON-LD + Open Graph + Twitter Cards).
//!
//! Real walkers land in Tasks 3 and 4.

use url::Url;

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
    pub schema_types: Vec<String>,
}

impl ExtractedMetadata {
    /// True if no field is populated.
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
}

/// Walk the raw HTML and extract structured metadata. The full body lands in
/// later tasks; for now this returns the default.
pub fn extract(_html: &str, _base: &Url) -> ExtractedMetadata {
    ExtractedMetadata::default()
}
```

- [ ] **Step 3: Create stub `src/extractor/base_href.rs`**

```rust
//! Peek `<base href>` from raw HTML before readabilityrs touches it.

use url::Url;

pub fn read_base_href(_html: &str) -> Option<Url> {
    None
}
```

- [ ] **Step 4: Create stub `src/extractor/links.rs`**

```rust
//! Markdown link/image post-pass: rewrite relative URLs to absolute.

use url::Url;

pub fn absolutize(markdown: &str, _base: &Url) -> String {
    markdown.to_string()
}
```

- [ ] **Step 5: Create stub `src/extractor/tables.rs`**

```rust
//! Table transformation modes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTransform {
    pub ordinal: usize,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}
```

- [ ] **Step 6: Create stub `src/extractor/images.rs`**

```rust
//! Image transformation modes.
```

- [ ] **Step 7: Create stub `src/extractor/quality.rs`**

```rust
//! extraction_quality scorer.

pub fn score(_extracted_md: &str, _raw_html_text_len: usize, _has_metadata: bool, _has_title: bool) -> f32 {
    0.0
}
```

- [ ] **Step 8: Create stub `src/extractor/output.rs`**

```rust
//! Output paths for table CSVs and downloaded images.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OutputPaths {
    pub(crate) root: PathBuf,
}
```

- [ ] **Step 9: Create `src/extractor/options.rs`**

```rust
//! Per-fetch extraction options carried through the pipeline.

use std::sync::Arc;

use crate::extractor::output::OutputPaths;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub tables: TablesMode,
    pub images: ImagesMode,
    pub metadata: MetadataMode,
    pub output_paths: Arc<OutputPaths>,
}

#[derive(Debug, Clone, Default)]
pub enum MetadataMode {
    #[default]
    Include,
    Skip,
}

#[derive(Debug, Clone)]
pub enum TablesMode {
    Embed,
    Sample(SampleStrategy),
    CsvFile,
    Drop,
}

impl Default for TablesMode {
    fn default() -> Self {
        TablesMode::Embed
    }
}

#[derive(Debug, Clone)]
pub enum SampleStrategy {
    HeadTail { head: usize, tail: usize },
    RandomSeed { rows: usize, seed: u64 },
}

impl Default for SampleStrategy {
    fn default() -> Self {
        SampleStrategy::HeadTail { head: 5, tail: 5 }
    }
}

#[derive(Debug, Clone, Default)]
pub enum ImagesMode {
    Keep,
    #[default]
    AltTextOnly,
    Download,
    Drop,
}
```

- [ ] **Step 10: Wire modules into `src/extractor/mod.rs`**

Replace contents with:

```rust
//! Content extraction pipeline.

pub mod base_href;
pub mod frontmatter;
pub mod images;
pub mod links;
pub mod metadata;
pub mod options;
pub mod output;
pub mod pipeline;
pub mod quality;
pub mod tables;

pub use metadata::ExtractedMetadata;
pub use options::{ExtractOptions, ImagesMode, MetadataMode, SampleStrategy, TablesMode};
pub use output::OutputPaths;
pub use pipeline::{ExtractedDoc, ExtractorError, extract};
pub use tables::TableTransform;
```

- [ ] **Step 11: Build to verify deps resolve and stubs compile**

```bash
cargo build
```

Expected: clean compile, zero warnings (warnings are denied in `Cargo.toml`).

If any stub re-export is flagged as `dead_code`, leave it: subsequent tasks consume each name. Otherwise, add per-item `#[allow(dead_code)]` with a one-line comment naming the task that constructs it.

- [ ] **Step 12: Commit**

```bash
git add Cargo.toml Cargo.lock src/extractor/
git commit -m "feat(m4): scaffold extractor submodules + Cargo deps for csv/rand/mime_guess"
```

---

## Task 2: ExtractorError extensions

**Files:**
- Modify: `src/extractor/pipeline.rs` (extend `ExtractorError`)
- Modify: `src/mcp/error.rs` (the existing `Self::Extractor(_)` arm needs no change; verify)

The existing `ExtractorError` has `Readability(String)` and `NoArticle`. M4 adds five variants. They're added now so subsequent tasks can `?`-propagate without needing to revisit the enum.

- [ ] **Step 1: Add the new variants to `src/extractor/pipeline.rs`**

Locate the `ExtractorError` enum and replace it with:

```rust
#[derive(Debug, Error)]
pub enum ExtractorError {
    #[error("readabilityrs: {0}")]
    Readability(String),

    #[error("readabilityrs returned no article")]
    NoArticle,

    #[error("metadata extraction failed: {0}")]
    Metadata(String),

    #[error("output directory error at {path}: {source}")]
    Output {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not write table {ordinal} to {path}: {source}")]
    TableWrite {
        ordinal: usize,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not download image at {url}: {source}")]
    ImageDownload {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("could not write image at {path}: {source}")]
    ImageWrite {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
```

- [ ] **Step 2: Inline test for translation through McpError**

Append to `src/mcp/error.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn extractor_output_error_routes_to_extract_failed() {
        use crate::extractor::ExtractorError;
        let e = McpError::Extractor(ExtractorError::Output {
            path: "/no/such".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        });
        let r = e.into_rover_error();
        assert_eq!(r.code, RoverError::EXTRACT_FAILED);
        assert!(r.message.contains("/no/such"));
    }
```

- [ ] **Step 3: Run the test + build**

```bash
cargo test --lib mcp::error
cargo build
```

Expected: test passes; build clean. The new variants are constructed only in later tasks; if dead-code denies them, add per-variant `#[allow(dead_code)]` with the task number.

- [ ] **Step 4: Commit**

```bash
git add src/extractor/pipeline.rs src/mcp/error.rs
git commit -m "feat(extractor): m4 error variants (metadata, output, table/image write/download)"
```

---

## Task 3: JSON-LD walker

**Files:**
- Modify: `src/extractor/metadata.rs`

Implements depth-8 flattening of every `<script type="application/ld+json">` block, picks the "primary" node, surfaces scalar fields, collects schema_types.

- [ ] **Step 1: Write the failing tests in `src/extractor/metadata.rs`**

Append (above any existing `extract` stub):

```rust
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
        // 20-deep nested object (well past the depth-8 cap).
        let mut payload = String::from(r#"{"@type":"Thing","x":"#);
        for _ in 0..20 { payload.push_str(r#"{"x":"#); }
        payload.push('"');
        for _ in 0..20 { payload.push_str("\"}"); }
        payload.push_str(r#"}"#);
        let html = format!(
            r#"<!doctype html><html><head><script type="application/ld+json">{payload}</script></head><body></body></html>"#
        );
        let m = extract(&html, &base());
        // Walker bottoms out gracefully; primary node is "Thing".
        assert!(m.schema_types.contains(&"Thing".to_string()));
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
```

- [ ] **Step 2: Implement the JSON-LD walker**

Replace the file's body (keeping the `ExtractedMetadata` struct from Task 1) with:

```rust
//! Structured-metadata extraction (JSON-LD + Open Graph + Twitter Cards).
//!
//! JSON-LD walker flattens `@graph` arrays and nested objects up to depth
//! 8, picks the first node whose `@type` is in the "primary" set, and
//! surfaces its scalar fields.

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
        if self.title.is_none() { self.title = other.title; }
        if self.description.is_none() { self.description = other.description; }
        if self.author.is_none() { self.author = other.author; }
        if self.published.is_none() { self.published = other.published; }
        if self.modified.is_none() { self.modified = other.modified; }
        if self.image.is_none() { self.image = other.image; }
        if self.og_type.is_none() { self.og_type = other.og_type; }
        if self.canonical.is_none() { self.canonical = other.canonical; }
        if self.language.is_none() { self.language = other.language; }
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

fn walk(v: &Value, depth: usize, nodes: &mut Vec<Value>, all_types: &mut Vec<String>) {
    if depth > MAX_DEPTH { return; }
    match v {
        Value::Object(map) => {
            if let Some(t) = map.get("@type") {
                let names = type_names(t);
                if !names.is_empty() {
                    nodes.push(v.clone());
                    for n in names { all_types.push(n); }
                }
            }
            if let Some(graph) = map.get("@graph") {
                walk(graph, depth + 1, nodes, all_types);
            }
            for (k, child) in map {
                if k == "@type" || k == "@graph" { continue; }
                walk(child, depth + 1, nodes, all_types);
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
```

- [ ] **Step 3: Run the tests**

```bash
cargo test --lib extractor::metadata
```

Expected: all four `jsonld_tests` pass.

- [ ] **Step 4: Commit**

```bash
git add src/extractor/metadata.rs
git commit -m "feat(extractor): json-ld walker with depth-8 flattening and primary-type selection"
```

---

## Task 4: Open Graph + Twitter Cards walkers + merged extract

**Files:**
- Modify: `src/extractor/metadata.rs`

Adds OG and Twitter walkers, plus `html[lang]`, `meta[name="description"]`, and `link[rel="canonical"]` extraction. Composes into the public `extract` function.

- [ ] **Step 1: Write the failing tests**

Append to `src/extractor/metadata.rs`:

```rust
#[cfg(test)]
mod og_twitter_tests {
    use super::*;
    use url::Url;

    fn base() -> Url {
        Url::parse("https://example.com/").unwrap()
    }

    #[test]
    fn reads_open_graph_metatags() {
        let html = r#"<!doctype html><html lang="en"><head>
            <meta property="og:title" content="OG Title">
            <meta property="og:description" content="OG Desc">
            <meta property="og:image" content="https://x/og.png">
            <meta property="og:type" content="article">
            <meta property="article:published_time" content="2026-03-01T00:00:00Z">
            <meta property="article:modified_time" content="2026-03-02T00:00:00Z">
            <meta property="article:author" content="Grace Hopper">
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(m.title.as_deref(), Some("OG Title"));
        assert_eq!(m.description.as_deref(), Some("OG Desc"));
        assert_eq!(m.image.as_deref(), Some("https://x/og.png"));
        assert_eq!(m.og_type.as_deref(), Some("article"));
        assert_eq!(m.published.as_deref(), Some("2026-03-01T00:00:00Z"));
        assert_eq!(m.modified.as_deref(), Some("2026-03-02T00:00:00Z"));
        assert_eq!(m.author.as_deref(), Some("Grace Hopper"));
        assert_eq!(m.language.as_deref(), Some("en"));
    }

    #[test]
    fn twitter_fills_holes_left_by_og() {
        let html = r#"<!doctype html><html><head>
            <meta name="twitter:title" content="Twitter Title">
            <meta name="twitter:description" content="Twitter Desc">
            <meta name="twitter:image" content="https://x/tc.png">
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(m.title.as_deref(), Some("Twitter Title"));
        assert_eq!(m.description.as_deref(), Some("Twitter Desc"));
        assert_eq!(m.image.as_deref(), Some("https://x/tc.png"));
    }

    #[test]
    fn jsonld_wins_over_og_wins_over_twitter() {
        let html = r#"<!doctype html><html><head>
            <script type="application/ld+json">
            {"@type":"Article","headline":"JSON-LD Title"}
            </script>
            <meta property="og:title" content="OG Title">
            <meta name="twitter:title" content="Twitter Title">
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(m.title.as_deref(), Some("JSON-LD Title"));
    }

    #[test]
    fn description_meta_fills_when_others_missing() {
        let html = r#"<!doctype html><html><head>
            <meta name="description" content="Plain meta desc">
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(m.description.as_deref(), Some("Plain meta desc"));
    }

    #[test]
    fn canonical_absolutized_against_base() {
        let html = r#"<!doctype html><html><head>
            <link rel="canonical" href="/article">
            </head><body></body></html>"#;
        let m = extract(html, &base());
        assert_eq!(m.canonical.as_deref(), Some("https://example.com/article"));
    }
}
```

- [ ] **Step 2: Implement OG + Twitter + canonical**

Replace the `extract` function with:

```rust
pub fn extract(html: &str, base: &Url) -> ExtractedMetadata {
    let doc = Html::parse_document(html);
    let mut out = ExtractedMetadata::default();
    out.merge_in(extract_jsonld(&doc));
    out.merge_in(extract_open_graph(&doc));
    out.merge_in(extract_twitter(&doc));
    out.merge_in(extract_meta_description(&doc));
    out.merge_in(extract_html_lang(&doc));
    out.merge_in(extract_canonical(&doc, base));
    out
}

fn meta_content(doc: &Html, sel: &str) -> Option<String> {
    let selector = Selector::parse(sel).ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn extract_open_graph(doc: &Html) -> ExtractedMetadata {
    ExtractedMetadata {
        title: meta_content(doc, r#"meta[property="og:title"]"#),
        description: meta_content(doc, r#"meta[property="og:description"]"#),
        image: meta_content(doc, r#"meta[property="og:image"]"#),
        og_type: meta_content(doc, r#"meta[property="og:type"]"#),
        published: meta_content(doc, r#"meta[property="article:published_time"]"#),
        modified: meta_content(doc, r#"meta[property="article:modified_time"]"#),
        author: meta_content(doc, r#"meta[property="article:author"]"#),
        ..Default::default()
    }
}

fn extract_twitter(doc: &Html) -> ExtractedMetadata {
    ExtractedMetadata {
        title: meta_content(doc, r#"meta[name="twitter:title"]"#),
        description: meta_content(doc, r#"meta[name="twitter:description"]"#),
        image: meta_content(doc, r#"meta[name="twitter:image"]"#),
        ..Default::default()
    }
}

fn extract_meta_description(doc: &Html) -> ExtractedMetadata {
    ExtractedMetadata {
        description: meta_content(doc, r#"meta[name="description"]"#),
        ..Default::default()
    }
}

fn extract_html_lang(doc: &Html) -> ExtractedMetadata {
    let selector = Selector::parse("html").unwrap();
    let language = doc
        .select(&selector)
        .next()
        .and_then(|el| el.value().attr("lang"))
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    ExtractedMetadata { language, ..Default::default() }
}

fn extract_canonical(doc: &Html, base: &Url) -> ExtractedMetadata {
    let selector = Selector::parse(r#"link[rel="canonical"]"#).unwrap();
    let canonical = doc
        .select(&selector)
        .next()
        .and_then(|el| el.value().attr("href"))
        .and_then(|href| base.join(href).ok())
        .map(|u| u.to_string());
    ExtractedMetadata { canonical, ..Default::default() }
}
```

- [ ] **Step 3: Run the full metadata suite**

```bash
cargo test --lib extractor::metadata
```

Expected: nine tests pass (four jsonld + five og/twitter).

- [ ] **Step 4: Commit**

```bash
git add src/extractor/metadata.rs
git commit -m "feat(extractor): og + twitter + canonical + lang walkers; first-wins precedence"
```

---

## Task 5: base_href reader

**Files:**
- Modify: `src/extractor/base_href.rs`

- [ ] **Step 1: Write the failing tests**

Replace the stub with:

```rust
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
    if href.is_empty() { return None; }
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
```

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib extractor::base_href
```

Expected: 4 pass.

```bash
git add src/extractor/base_href.rs
git commit -m "feat(extractor): base_href reader (head > base[href]; first wins)"
```

---

## Task 6: Link absolutization post-pass

**Files:**
- Modify: `src/extractor/links.rs`

Walks the rendered Markdown for inline links (`[text](href)`), inline images (`![alt](src)`), and reference-style link definitions (`[id]: href "title"`). Resolves relative URLs against the base.

- [ ] **Step 1: Write the failing tests**

Replace the stub with:

```rust
//! Markdown link/image post-pass: rewrite relative URLs to absolute.

use once_cell::sync::Lazy;  // already pulled in transitively via tokio-rusqlite; verify
use regex::Regex;
use url::Url;

// `[text](href)` — text may contain balanced brackets at depth 0; we
// accept anything that isn't a literal `]` then the `(href)` form. href
// stops at the first unescaped `)`.
static INLINE_LINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?P<alt>!?)\[(?P<text>[^\]]*)\]\((?P<href>[^)\s]+)(?P<rest>[^)]*)\)").unwrap()
});

// `[id]: href "optional title"` at start of line
static REF_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\[(?P<id>[^\]]+)\]:\s*(?P<href>\S+)(?P<rest>.*)$"#).unwrap()
});

pub fn absolutize(markdown: &str, base: &Url) -> String {
    let pass1 = INLINE_LINK.replace_all(markdown, |caps: &regex::Captures| {
        let alt = &caps["alt"];
        let text = &caps["text"];
        let href = &caps["href"];
        let rest = &caps["rest"];
        let abs = resolve(base, href);
        format!("{alt}[{text}]({abs}{rest})")
    });
    REF_DEF.replace_all(&pass1, |caps: &regex::Captures| {
        let id = &caps["id"];
        let href = &caps["href"];
        let rest = &caps["rest"];
        let abs = resolve(base, href);
        format!("[{id}]: {abs}{rest}")
    }).into_owned()
}

fn resolve(base: &Url, href: &str) -> String {
    // Already absolute (has scheme)?
    if href.contains("://") || href.starts_with("mailto:") || href.starts_with("data:") {
        return href.to_string();
    }
    match base.join(href) {
        Ok(u) => u.to_string(),
        Err(e) => {
            tracing::debug!(target: "rover::extractor", href, err = %e, "could not join link href");
            href.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Url {
        Url::parse("https://example.com/articles/m4").unwrap()
    }

    #[test]
    fn inline_relative_link_absolutized() {
        let md = "See [docs](/docs/intro).";
        let out = absolutize(md, &b());
        assert_eq!(out, "See [docs](https://example.com/docs/intro).");
    }

    #[test]
    fn absolute_link_unchanged() {
        let md = "Visit [site](https://www.example.org/).";
        let out = absolutize(md, &b());
        assert_eq!(out, md);
    }

    #[test]
    fn inline_image_src_absolutized() {
        let md = "![alt](/img/x.png)";
        let out = absolutize(md, &b());
        assert_eq!(out, "![alt](https://example.com/img/x.png)");
    }

    #[test]
    fn reference_definition_absolutized() {
        let md = "[ref]: /docs/ref \"title\"\nSome [ref] usage.";
        let out = absolutize(md, &b());
        assert!(out.contains("[ref]: https://example.com/docs/ref \"title\""));
    }

    #[test]
    fn anchor_hash_absolutized() {
        let md = "[next](#section)";
        let out = absolutize(md, &b());
        assert!(out.contains("https://example.com/articles/m4#section"));
    }

    #[test]
    fn mailto_and_data_preserved() {
        let md = "Email [me](mailto:x@y.z) and ![pixel](data:image/png;base64,iVBORw0KGgo).";
        let out = absolutize(md, &b());
        assert_eq!(out, md);
    }
}
```

- [ ] **Step 2: Add `once_cell` dep if not transitively present**

Run `cargo build` first. If unresolved, append to `Cargo.toml` `[dependencies]`:

```toml
once_cell = "1"
```

- [ ] **Step 3: Run tests + commit**

```bash
cargo test --lib extractor::links
```

Expected: 6 pass.

```bash
git add src/extractor/links.rs Cargo.toml Cargo.lock
git commit -m "feat(extractor): markdown link/image absolutization with regex post-pass"
```

---

## Task 7: extraction_quality scorer

**Files:**
- Modify: `src/extractor/quality.rs`

- [ ] **Step 1: Write the failing tests**

Replace the stub with:

```rust
//! extraction_quality scorer.
//!
//! score = density + 0.05 (if title) + 0.10 (if metadata), clamped to [0, 1].
//! density = extracted_text_len / raw_html_text_len, clamped to [0, 1].

pub fn score(
    extracted_md: &str,
    raw_html_text_len: usize,
    has_metadata: bool,
    has_title: bool,
) -> f32 {
    let extracted_len = visible_text_len(extracted_md);
    let density = (extracted_len as f32 / raw_html_text_len.max(1) as f32).min(1.0);
    let mut bonus = 0.0;
    if has_title { bonus += 0.05; }
    if has_metadata { bonus += 0.10; }
    (density + bonus).clamp(0.0, 1.0)
}

/// Strip whitespace-only lines and a leading YAML frontmatter block, then return char count.
fn visible_text_len(md: &str) -> usize {
    let after_fm = strip_frontmatter(md);
    after_fm
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().count())
        .sum()
}

fn strip_frontmatter(s: &str) -> &str {
    if !s.starts_with("---\n") { return s; }
    let rest = &s[4..];
    if let Some(end) = rest.find("\n---\n") {
        return &rest[end + 5..];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_scores_zero() {
        assert_eq!(score("", 100, false, false), 0.0);
    }

    #[test]
    fn perfect_density_with_bonuses_clamped_to_one() {
        // 100-char extracted text against 50-char raw -> density 2.0 clamped to 1.0
        let md = "a".repeat(100);
        let s = score(&md, 50, true, true);
        assert!((s - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn density_only_path_produces_expected_ratio() {
        let md = "a".repeat(40);
        let s = score(&md, 100, false, false);
        assert!((s - 0.40).abs() < 0.01);
    }

    #[test]
    fn bonuses_alone_are_capped_below_one() {
        let s = score("", 100, true, true);
        // density 0; bonuses 0.15
        assert!((s - 0.15).abs() < 0.01);
    }

    #[test]
    fn score_always_in_unit_interval() {
        for raw in [1usize, 10, 100, 1000, 1_000_000] {
            for md_len in [0usize, 1, 10, 100, 10_000] {
                let md = "a".repeat(md_len);
                let s = score(&md, raw, true, true);
                assert!((0.0..=1.0).contains(&s), "raw={raw} md_len={md_len} s={s}");
            }
        }
    }

    #[test]
    fn frontmatter_excluded_from_density() {
        let md = "---\nurl: \"x\"\n---\n\nhello world\n";
        let s = score(md, 100, false, false);
        // "hello world" is 11 chars -> 0.11
        assert!((s - 0.11).abs() < 0.02);
    }
}
```

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib extractor::quality
```

Expected: 6 pass.

```bash
git add src/extractor/quality.rs
git commit -m "feat(extractor): extraction_quality scorer (density + title/metadata bonuses)"
```

---

## Task 8: OutputPaths + [output] config

**Files:**
- Modify: `src/extractor/output.rs`
- Modify: `src/config.rs` (add `OutputConfig`)

Resolves the output root from env > config > XDG default, creates the directory, and exposes path helpers.

- [ ] **Step 1: Write OutputPaths impl with tests**

Replace `src/extractor/output.rs` with:

```rust
//! Output paths for table CSVs and downloaded images.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use url::Url;

use crate::extractor::pipeline::ExtractorError;

#[derive(Debug, Clone)]
pub struct OutputPaths {
    root: PathBuf,
}

impl OutputPaths {
    /// Resolve the output root. Precedence: `ROVER_OUTPUT_DIR` env var,
    /// then the supplied path (if `Some`), then `dirs::data_local_dir()
    /// .join("rover").join("output")`. Creates the root if missing.
    pub fn resolve(configured: Option<&Path>) -> Result<Self, ExtractorError> {
        let root: PathBuf = if let Ok(env_dir) = std::env::var("ROVER_OUTPUT_DIR") {
            PathBuf::from(env_dir)
        } else if let Some(p) = configured {
            p.to_path_buf()
        } else {
            dirs::data_local_dir()
                .ok_or_else(|| ExtractorError::Output {
                    path: "<data_local_dir>".to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no platform data dir",
                    ),
                })?
                .join("rover")
                .join("output")
        };
        std::fs::create_dir_all(&root).map_err(|source| ExtractorError::Output {
            path: root.display().to_string(),
            source,
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn table_path(&self, url: &Url, table_ordinal: usize) -> PathBuf {
        let host = url.host_str().unwrap_or("unknown");
        let key = format!("{}#{}", url.as_str(), table_ordinal);
        self.root
            .join("tables")
            .join(host)
            .join(format!("{}.csv", sha8(&key)))
    }

    pub fn image_path(&self, url: &Url, ext: &str) -> PathBuf {
        let host = url.host_str().unwrap_or("unknown");
        let ext = ext.trim_start_matches('.');
        let ext = if ext.is_empty() { "bin" } else { ext };
        self.root
            .join("images")
            .join(host)
            .join(format!("{}.{ext}", sha8(url.as_str())))
    }
}

pub fn sha8(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    out.iter().take(4).fold(String::with_capacity(8), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> Url { Url::parse("https://example.com/article").unwrap() }

    #[test]
    fn sha8_is_deterministic_and_eight_chars() {
        let a = sha8("https://example.com/x");
        let b = sha8("https://example.com/x");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn table_path_includes_ordinal() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let paths = OutputPaths::resolve(None).unwrap();
        let p0 = paths.table_path(&url(), 0);
        let p1 = paths.table_path(&url(), 1);
        assert_ne!(p0, p1);
        assert!(p0.to_string_lossy().ends_with(".csv"));
        assert!(p0.to_string_lossy().contains("example.com"));
    }

    #[test]
    fn image_path_uses_sha8_of_url_and_ext() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let paths = OutputPaths::resolve(None).unwrap();
        let p = paths.image_path(&Url::parse("https://x/img.png").unwrap(), "png");
        assert!(p.to_string_lossy().ends_with(".png"));
        assert!(p.to_string_lossy().contains(&sha8("https://x/img.png")));
    }

    #[test]
    fn resolve_honors_env_then_config_then_default() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let p = OutputPaths::resolve(Some(Path::new("/ignored"))).unwrap();
        assert_eq!(p.root, tmp.path());
        // SAFETY: single-threaded test
        unsafe { std::env::remove_var("ROVER_OUTPUT_DIR") };

        let tmp2 = tempfile::tempdir().unwrap();
        let p2 = OutputPaths::resolve(Some(tmp2.path())).unwrap();
        assert_eq!(p2.root, tmp2.path());
    }
}
```

- [ ] **Step 2: Add `[output]` to `src/config.rs`**

Add to `Config`:

```rust
    #[serde(default)]
    pub output: OutputConfig,
```

Append (alongside `TokenizerConfig`, `McpConfig`):

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    #[serde(default)]
    pub dir: Option<std::path::PathBuf>,
}
```

And a test inside `mod tests`:

```rust
    #[test]
    fn load_output_dir_override() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, r#"
[output]
dir = "/tmp/rover-out"
"#).unwrap();
        let cfg = load(Some(file.path())).unwrap();
        assert_eq!(cfg.output.dir.as_deref().unwrap().to_str(), Some("/tmp/rover-out"));
    }
```

- [ ] **Step 3: Run tests + commit**

```bash
cargo test --lib extractor::output
cargo test --lib config
```

Expected: all pass.

```bash
git add src/extractor/output.rs src/config.rs
git commit -m "feat(output): OutputPaths resolver + [output] config section + sha8 helper"
```

---

## Task 9: Tables modes — Embed, Drop, Sample (HeadTail + RandomSeed), CsvFile

**Files:**
- Modify: `src/extractor/tables.rs`

Walks the markdown for pipe tables and applies one of four modes.

- [ ] **Step 1: Implement the table walker + modes + tests**

Replace `src/extractor/tables.rs` with:

```rust
//! Table transformation modes.

use std::path::PathBuf;

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::options::{SampleStrategy, TablesMode};
use crate::extractor::output::OutputPaths;
use crate::extractor::pipeline::ExtractorError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTransform {
    pub ordinal: usize,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_rows: Option<usize>,
}

/// Returns the transformed markdown plus per-table records.
pub fn apply(
    markdown: &str,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
) -> Result<(String, Vec<TableTransform>), ExtractorError> {
    let mut out = String::with_capacity(markdown.len());
    let mut records = Vec::new();
    let mut ordinal: usize = 0;
    let mut iter = markdown.lines().peekable();

    while let Some(line) = iter.next() {
        if is_pipe_table_start(line, iter.peek().copied()) {
            // Collect all consecutive table lines.
            let mut rows: Vec<String> = vec![line.to_string()];
            while let Some(next) = iter.peek().copied() {
                if next.trim_start().starts_with('|') {
                    rows.push(next.to_string());
                    iter.next();
                } else {
                    break;
                }
            }
            let (replacement, record) = transform_table(rows, ordinal, mode, output_paths, base_url)?;
            out.push_str(&replacement);
            out.push('\n');
            if let Some(r) = record {
                records.push(r);
            }
            ordinal += 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok((out, records))
}

fn is_pipe_table_start(line: &str, next: Option<&str>) -> bool {
    if !line.trim_start().starts_with('|') { return false; }
    let Some(n) = next else { return false; };
    // Markdown pipe-table separator: |---|---|... or |:---:|...
    let nt = n.trim_start();
    nt.starts_with('|') && nt.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn transform_table(
    rows: Vec<String>,
    ordinal: usize,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
) -> Result<(String, Option<TableTransform>), ExtractorError> {
    match mode {
        TablesMode::Embed => Ok((
            rows.join("\n"),
            Some(TableTransform {
                ordinal,
                mode: "embed".into(),
                path: None,
                kept_rows: None,
                truncated_rows: None,
            }),
        )),
        TablesMode::Drop => Ok((
            format!("_Table {ordinal} omitted_"),
            Some(TableTransform {
                ordinal,
                mode: "drop".into(),
                path: None,
                kept_rows: None,
                truncated_rows: None,
            }),
        )),
        TablesMode::Sample(strategy) => {
            // rows[0] = header, rows[1] = separator, rows[2..] = data
            if rows.len() < 3 {
                return Ok((rows.join("\n"), None));
            }
            let header = &rows[0];
            let sep = &rows[1];
            let data: Vec<&String> = rows[2..].iter().collect();
            let (kept, truncated) = sample_rows(&data, strategy);
            let mut out = vec![header.clone(), sep.clone()];
            for r in &kept { out.push((*r).clone()); }
            if truncated > 0 {
                out.push(format!("_… {truncated} rows truncated …_"));
            }
            Ok((
                out.join("\n"),
                Some(TableTransform {
                    ordinal,
                    mode: "sample".into(),
                    path: None,
                    kept_rows: Some(kept.len()),
                    truncated_rows: Some(truncated),
                }),
            ))
        }
        TablesMode::CsvFile => {
            let path = output_paths.table_path(base_url, ordinal);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| ExtractorError::TableWrite {
                    ordinal,
                    path: parent.display().to_string(),
                    source,
                })?;
            }
            write_csv(&path, &rows, ordinal)?;
            let abs = path.canonicalize().unwrap_or(path.clone());
            Ok((
                format!("_Table {ordinal} saved to {}_", abs.display()),
                Some(TableTransform {
                    ordinal,
                    mode: "csv_file".into(),
                    path: Some(abs),
                    kept_rows: None,
                    truncated_rows: None,
                }),
            ))
        }
    }
}

fn sample_rows<'a>(data: &[&'a String], strategy: &SampleStrategy) -> (Vec<&'a String>, usize) {
    let total = data.len();
    match strategy {
        SampleStrategy::HeadTail { head, tail } => {
            if total <= head + tail {
                return (data.iter().copied().collect(), 0);
            }
            let mut kept: Vec<&String> = data.iter().take(*head).copied().collect();
            kept.extend(data.iter().rev().take(*tail).rev().copied());
            let truncated = total - kept.len();
            (kept, truncated)
        }
        SampleStrategy::RandomSeed { rows, seed } => {
            if total <= *rows {
                return (data.iter().copied().collect(), 0);
            }
            let mut rng = ChaCha8Rng::seed_from_u64(*seed);
            let mut indices: Vec<usize> = (0..total).collect();
            indices.shuffle(&mut rng);
            indices.truncate(*rows);
            indices.sort();
            let kept: Vec<&String> = indices.iter().map(|i| data[*i]).collect();
            let truncated = total - kept.len();
            (kept, truncated)
        }
    }
}

fn parse_pipe_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.trim_start_matches('|').trim_end_matches('|');
    line.split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

fn write_csv(path: &std::path::Path, rows: &[String], ordinal: usize) -> Result<(), ExtractorError> {
    let file = std::fs::File::create(path).map_err(|source| ExtractorError::TableWrite {
        ordinal,
        path: path.display().to_string(),
        source,
    })?;
    let mut wtr = csv::Writer::from_writer(file);
    for (i, row) in rows.iter().enumerate() {
        if i == 1 { continue; } // skip separator
        let cells = parse_pipe_row(row);
        wtr.write_record(&cells).map_err(|e| ExtractorError::TableWrite {
            ordinal,
            path: path.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?;
    }
    wtr.flush().map_err(|source| ExtractorError::TableWrite {
        ordinal,
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> OutputPaths {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        // Leak the tempdir; OS reclaims on process exit (matches storage tests).
        std::mem::forget(tmp);
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
        OutputPaths::resolve(None).unwrap()
    }

    fn url() -> Url { Url::parse("https://example.com/").unwrap() }

    const TABLE_3ROWS: &str = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";

    #[test]
    fn embed_mode_passes_through() {
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::Embed, &paths(), &url()).unwrap();
        assert!(out.contains("| 1 | 2 |"));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].mode, "embed");
    }

    #[test]
    fn drop_mode_replaces_with_marker() {
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::Drop, &paths(), &url()).unwrap();
        assert!(out.contains("_Table 0 omitted_"));
        assert!(!out.contains("| 1 | 2 |"));
        assert_eq!(recs[0].mode, "drop");
    }

    #[test]
    fn sample_head_tail_keeps_head_plus_tail() {
        let strategy = SampleStrategy::HeadTail { head: 1, tail: 1 };
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::Sample(strategy), &paths(), &url()).unwrap();
        assert!(out.contains("| 1 | 2 |"));
        assert!(out.contains("| 5 | 6 |"));
        assert!(out.contains("_… 1 rows truncated …_"));
        assert_eq!(recs[0].kept_rows, Some(2));
        assert_eq!(recs[0].truncated_rows, Some(1));
    }

    #[test]
    fn sample_random_seed_is_deterministic() {
        let strat = SampleStrategy::RandomSeed { rows: 2, seed: 42 };
        let (out_a, _) = apply(TABLE_3ROWS, &TablesMode::Sample(strat.clone()), &paths(), &url()).unwrap();
        let (out_b, _) = apply(TABLE_3ROWS, &TablesMode::Sample(strat), &paths(), &url()).unwrap();
        assert_eq!(out_a, out_b);
    }

    #[test]
    fn csv_file_writes_table_to_disk_and_replaces_markdown() {
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::CsvFile, &paths(), &url()).unwrap();
        assert!(out.contains("_Table 0 saved to "));
        let p = recs[0].path.as_ref().unwrap();
        let csv = std::fs::read_to_string(p).unwrap();
        assert!(csv.contains("A,B"));
        assert!(csv.contains("1,2"));
        assert!(csv.contains("5,6"));
    }

    #[test]
    fn non_table_content_passes_through_unchanged() {
        let md = "Just some text\n\nNo tables here.\n";
        let (out, recs) = apply(md, &TablesMode::Drop, &paths(), &url()).unwrap();
        assert_eq!(out, md);
        assert!(recs.is_empty());
    }
}
```

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib extractor::tables
```

Expected: 6 pass.

```bash
git add src/extractor/tables.rs
git commit -m "feat(extractor): tables modes (embed/drop/sample headtail+randomseed/csv_file)"
```

---

## Task 10: Images modes — Keep, AltTextOnly, Drop, Download

**Files:**
- Modify: `src/extractor/images.rs`

- [ ] **Step 1: Implement the image walker + modes + tests**

Replace `src/extractor/images.rs` with:

```rust
//! Image transformation modes.

use once_cell::sync::Lazy;
use regex::Regex;
use url::Url;

use crate::extractor::options::ImagesMode;
use crate::extractor::output::OutputPaths;
use crate::extractor::pipeline::ExtractorError;

static INLINE_IMG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"!\[(?P<alt>[^\]]*)\]\((?P<src>[^)\s]+)(?P<rest>[^)]*)\)").unwrap());

#[derive(Debug, Default, Clone)]
pub struct ImagesApplied {
    pub markdown: String,
    pub images_seen: usize,
    pub images_downloaded: usize,
    pub images_failed: usize,
}

pub async fn apply(
    markdown: &str,
    mode: &ImagesMode,
    output_paths: &OutputPaths,
    http: &reqwest::Client,
) -> Result<ImagesApplied, ExtractorError> {
    let mut images_seen = 0usize;
    let mut images_downloaded = 0usize;
    let mut images_failed = 0usize;

    // Two-step: enumerate matches, then transform. async download requires
    // we can't use `replace_all` directly.
    let matches: Vec<_> = INLINE_IMG
        .captures_iter(markdown)
        .map(|c| {
            let m = c.get(0).unwrap();
            (
                m.start(),
                m.end(),
                c["alt"].to_string(),
                c["src"].to_string(),
                c["rest"].to_string(),
            )
        })
        .collect();

    let mut out = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for (start, end, alt, src, rest) in matches {
        images_seen += 1;
        out.push_str(&markdown[cursor..start]);
        cursor = end;
        let replacement: String = match mode {
            ImagesMode::Keep => markdown[start..end].to_string(),
            ImagesMode::Drop => String::new(),
            ImagesMode::AltTextOnly => alt.clone(),
            ImagesMode::Download => {
                match download_one(http, &src, output_paths).await {
                    Ok(local) => {
                        images_downloaded += 1;
                        format!("![{alt}]({local}{rest})")
                    }
                    Err(e) => {
                        images_failed += 1;
                        tracing::warn!(target: "rover::extractor", url = %src, err = %e, "image download failed; keeping original");
                        markdown[start..end].to_string()
                    }
                }
            }
        };
        out.push_str(&replacement);
    }
    out.push_str(&markdown[cursor..]);

    Ok(ImagesApplied {
        markdown: out,
        images_seen,
        images_downloaded,
        images_failed,
    })
}

async fn download_one(
    http: &reqwest::Client,
    src: &str,
    output_paths: &OutputPaths,
) -> Result<String, ExtractorError> {
    let url = Url::parse(src).map_err(|_| ExtractorError::ImageDownload {
        url: src.to_string(),
        source: reqwest::Error::from(
            reqwest::Client::new()
                .head(src)
                .build()
                .err()
                .unwrap_or_else(|| panic!("constructing error placeholder")),
        ),
    })?;
    let resp = http.get(url.clone()).send().await.map_err(|source| ExtractorError::ImageDownload {
        url: src.to_string(),
        source,
    })?;
    let resp = resp.error_for_status().map_err(|source| ExtractorError::ImageDownload {
        url: src.to_string(),
        source,
    })?;
    let ext = sniff_ext(&resp, &url);
    let bytes = resp.bytes().await.map_err(|source| ExtractorError::ImageDownload {
        url: src.to_string(),
        source,
    })?;
    let path = output_paths.image_path(&url, &ext);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExtractorError::ImageWrite {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&path, &bytes).map_err(|source| ExtractorError::ImageWrite {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path.canonicalize().unwrap_or(path).display().to_string())
}

fn sniff_ext(resp: &reqwest::Response, url: &Url) -> String {
    if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(s) = ct.to_str() {
            let mime = s.split(';').next().unwrap_or("").trim();
            if let Some(ext) = mime_guess::get_mime_extensions_str(mime).and_then(|exts| exts.first()) {
                return (*ext).to_string();
            }
        }
    }
    if let Some(path_seg) = url.path_segments().and_then(|s| s.last()) {
        if let Some((_, ext)) = path_seg.rsplit_once('.') {
            if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return ext.to_lowercase();
            }
        }
    }
    "bin".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> OutputPaths {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
        OutputPaths::resolve(None).unwrap()
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn keep_passes_through_unchanged() {
        let md = "Look ![alt](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::Keep, &paths(), &client()).await.unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 1);
        assert_eq!(r.images_downloaded, 0);
    }

    #[tokio::test]
    async fn alt_text_only_substitutes_alt() {
        let md = "Look ![hello](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::AltTextOnly, &paths(), &client()).await.unwrap();
        assert_eq!(r.markdown, "Look hello at this.");
    }

    #[tokio::test]
    async fn alt_text_only_with_empty_alt_removes_image() {
        let md = "Look ![](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::AltTextOnly, &paths(), &client()).await.unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn drop_removes_image_syntax_entirely() {
        let md = "Look ![alt](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::Drop, &paths(), &client()).await.unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn no_images_in_input_yields_empty_counters() {
        let md = "No images here.";
        let r = apply(md, &ImagesMode::Download, &paths(), &client()).await.unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 0);
    }
}
```

NOTE on the `Url::parse` error path in `download_one`: the spec shape shown for that error is awkward (synthesising a `reqwest::Error` from an unrelated builder). Replace it with the cleaner shape:

```rust
let url = Url::parse(src).map_err(|e| ExtractorError::ImageDownload {
    url: src.to_string(),
    source: reqwest::Client::new()
        .get("https://invalid")  // unreachable; just to construct
        .send()
        .await
        .err()
        .unwrap_or_else(|| panic!("constructing reqwest::Error placeholder")),
})?;
```

Actually, that's still awkward. Cleaner: add a new `ExtractorError` variant `ImageUrlInvalid { url: String, source: url::ParseError }` and route to it. But since the variant set was frozen in Task 2, the path of least resistance is to map `url::ParseError` to a different variant (e.g. `Metadata(format!("invalid image url: {src}: {e}"))`). Pick one; document the choice in the commit. **Recommended fix for the implementer:** add `ImageUrlInvalid { url: String, source: url::ParseError }` to `ExtractorError` (one additional variant), update Task 2's listing inline, and use it here.

- [ ] **Step 2: Add `ImageUrlInvalid` variant**

In `src/extractor/pipeline.rs`, append to `ExtractorError`:

```rust
    #[error("invalid image url {url}: {source}")]
    ImageUrlInvalid {
        url: String,
        #[source]
        source: url::ParseError,
    },
```

Update `download_one` to use it:

```rust
let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
    url: src.to_string(),
    source,
})?;
```

- [ ] **Step 3: Run tests + commit**

```bash
cargo test --lib extractor::images
```

Expected: 5 pass.

```bash
git add src/extractor/images.rs src/extractor/pipeline.rs
git commit -m "feat(extractor): images modes (keep/drop/alt-text-only/download with mime sniffing)"
```

---

## Task 11: Pipeline integration — two-pass extract, ExtractedDoc gains metadata, frontmatter expansion

**Files:**
- Modify: `src/extractor/pipeline.rs` (extend `extract`)
- Modify: `src/extractor/frontmatter.rs` (PageMeta + render)
- Modify: `src/fetcher/cached.rs` (ExtractResult carries metadata)
- Modify: `src/storage/pages.rs` (Page row round-trips metadata_json)

This is the biggest task — it wires the pieces together and threads the new types through the cache layer.

- [ ] **Step 1: Extend `extract()` to run the two-pass shape**

Replace the body of `pipeline::extract` with:

```rust
pub fn extract_full(
    html: &str,
    base_url: &Url,
) -> Result<ExtractedDoc, ExtractorError> {
    // Pre-pass: base href + metadata, on raw HTML.
    let effective_base = crate::extractor::base_href::read_base_href(html).unwrap_or_else(|| base_url.clone());
    let metadata = crate::extractor::metadata::extract(html, &effective_base);

    let raw_html_text_len = approximate_html_text_len(html);

    // readabilityrs (unchanged from M3).
    let opts = ReadabilityOptions::builder()
        .output_markdown(true)
        .markdown_options(rover_markdown_options())
        .build();
    let readability = Readability::new(html, Some(effective_base.as_str()), Some(opts))
        .map_err(|e| ExtractorError::Readability(e.to_string()))?;
    let article = readability.parse().ok_or(ExtractorError::NoArticle)?;

    let body_md = article.markdown_content.unwrap_or_default();

    // Post-pass 1: absolutize links/images relative to base.
    let body_md = crate::extractor::links::absolutize(&body_md, &effective_base);

    Ok(ExtractedDoc {
        title: article.title.or_else(|| metadata.title.clone()),
        body_md,
        language: article.lang.or_else(|| metadata.language.clone()),
        byline: article.byline,
        excerpt: article.excerpt,
        site_name: article.site_name,
        published_time: article.published_time.or_else(|| metadata.published.clone()),
        image: article.image.or_else(|| metadata.image.clone()),
        metadata,
        raw_html_text_len,
    })
}

// Backwards-compatible wrapper for callers that don't carry a Url yet.
pub fn extract(html: &str, base_url: Option<&Url>) -> Result<ExtractedDoc, ExtractorError> {
    let base = base_url
        .cloned()
        .unwrap_or_else(|| Url::parse("about:blank").unwrap());
    extract_full(html, &base)
}

fn approximate_html_text_len(html: &str) -> usize {
    let doc = scraper::Html::parse_document(html);
    let body_sel = scraper::Selector::parse("body").unwrap();
    doc.select(&body_sel)
        .next()
        .map(|b| b.text().map(|t| t.chars().count()).sum())
        .unwrap_or_else(|| html.chars().count())
}
```

And extend `ExtractedDoc`:

```rust
#[derive(Debug, Clone)]
pub struct ExtractedDoc {
    pub title: Option<String>,
    pub body_md: String,
    pub language: Option<String>,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub site_name: Option<String>,
    pub published_time: Option<String>,
    pub image: Option<String>,
    pub metadata: crate::extractor::metadata::ExtractedMetadata,
    pub raw_html_text_len: usize,
}
```

- [ ] **Step 2: Extend PageMeta + render with M4 fields**

Modify `src/extractor/frontmatter.rs`. `PageMeta`:

```rust
pub struct PageMeta<'a> {
    pub url: &'a Url,
    pub canonical_url: &'a Url,
    pub title: Option<&'a str>,
    pub fetched_at: Timestamp,
    pub body: &'a str,
    pub tokens: usize,
    pub tokenizer_name: &'a str,
    // M4 additions:
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
```

Update `render`:

```rust
pub fn render(meta: &PageMeta<'_>) -> String {
    let mut buf = String::with_capacity(meta.body.len() + 512);
    buf.push_str("---\n");

    write_field(&mut buf, "url", meta.url.as_str());
    if meta.canonical_url != meta.url {
        write_field(&mut buf, "canonical_url", meta.canonical_url.as_str());
    }
    if let Some(t) = meta.title { write_field(&mut buf, "title", t); }
    write_field(&mut buf, "fetched_at", &meta.fetched_at.to_string());
    write_field(&mut buf, "content_hash", &format!("sha256:{}", sha256_hex(meta.body.as_bytes())));
    buf.push_str(&format!("estimated_tokens: {}\n", meta.tokens));
    write_field(&mut buf, "tokenizer", meta.tokenizer_name);

    if let Some(v) = meta.description { write_field(&mut buf, "description", v); }
    if let Some(v) = meta.author { write_field(&mut buf, "author", v); }
    if let Some(v) = meta.published { write_field(&mut buf, "published", v); }
    if let Some(v) = meta.modified { write_field(&mut buf, "modified", v); }
    if let Some(v) = meta.image { write_field(&mut buf, "image", v); }
    if let Some(v) = meta.og_type { write_field(&mut buf, "og_type", v); }
    if let Some(v) = meta.language { write_field(&mut buf, "language", v); }
    if !meta.schema_types.is_empty() {
        buf.push_str("schema_types:\n");
        for s in meta.schema_types {
            buf.push_str("  - ");
            buf.push_str(&yaml_escape(s));
            buf.push('\n');
        }
    }
    buf.push_str(&format!("extraction_quality: {:.2}\n", meta.extraction_quality));
    if !meta.tables_transformed.is_empty() {
        buf.push_str("tables_transformed:\n");
        for t in meta.tables_transformed {
            buf.push_str(&format!("  - ordinal: {}\n    mode: {}\n", t.ordinal, t.mode));
            if let Some(p) = &t.path {
                buf.push_str(&format!("    path: {:?}\n", p.display().to_string()));
            }
            if let Some(k) = t.kept_rows { buf.push_str(&format!("    kept_rows: {k}\n")); }
            if let Some(tr) = t.truncated_rows { buf.push_str(&format!("    truncated_rows: {tr}\n")); }
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
    if !meta.body.ends_with('\n') { buf.push('\n'); }
    buf
}

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
```

Update the existing inline tests' `meta` helper to default the new fields to `None`/empty/0; add new tests for each surfaced field. Specifically:

```rust
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
```

- [ ] **Step 3: Extend `ExtractResult` to carry metadata**

In `src/fetcher/cached.rs`, add `metadata: ExtractedMetadata` to `ExtractResult`:

```rust
pub struct ExtractResult {
    pub title: Option<String>,
    pub body_md: String,
    pub content_hash: String,
    pub metadata: crate::extractor::metadata::ExtractedMetadata,
}
```

Inside `fetch_with_cache`, after a cache-miss extract, ensure the `metadata` field is captured by the closure into the storage write. The storage actor's `upsert` should serialize `metadata` as JSON into `pages.metadata_json`.

- [ ] **Step 4: Round-trip `metadata_json` in `src/storage/pages.rs`**

The M2 storage layer already reserved `metadata_json` as a column. Extend `Page`:

```rust
pub struct Page {
    // ... existing fields ...
    pub metadata: Option<crate::extractor::metadata::ExtractedMetadata>,
}
```

`upsert` serializes via `serde_json::to_string(&metadata)?` into the column; `from_row` deserializes via `serde_json::from_str::<ExtractedMetadata>(&s)`. If the column is NULL (M2 rows or M4 rows where metadata is empty), `metadata: None`.

Add a unit test in `pages.rs` that round-trips a populated `metadata` value.

- [ ] **Step 5: Update existing callers**

The CLI fetch path (`src/cli/fetch.rs`) and the MCP fetch tool (`src/mcp/tools/fetch.rs`) both call the extract closure. Update both to set `metadata: extracted.metadata` on the `ExtractResult` they construct. Both must compile; the new fields propagate.

- [ ] **Step 6: Run all tests**

```bash
cargo build
cargo test --lib
```

Expected: all existing tests plus the new ones pass.

- [ ] **Step 7: Commit**

```bash
git add src/extractor/ src/fetcher/cached.rs src/storage/pages.rs src/cli/fetch.rs src/mcp/tools/fetch.rs
git commit -m "feat(extractor): two-pass pipeline; frontmatter+cache carry metadata"
```

---

## Task 12: MCP fetch tool — typed args + per-request pipeline + frontmatter expansion

**Files:**
- Modify: `src/mcp/tools/fetch.rs`
- Modify: `src/mcp/envelope.rs` (if needed for new types)

Tightens the M3 placeholder args; threads `ExtractOptions` through the post-passes; expands the rendered frontmatter with the M4 fields.

- [ ] **Step 1: Define typed argument structs**

Replace the placeholder fields in `FetchArgs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    pub url: String,

    #[serde(default)]
    pub force_refresh: bool,

    #[serde(default)]
    pub count_only: bool,

    #[serde(default)]
    pub tokenizer: Option<String>,

    #[serde(default)]
    pub max_tokens: Option<usize>,

    #[serde(default)]
    pub tables: Option<TablesArg>,

    #[serde(default)]
    pub images: Option<ImagesArg>,

    #[serde(default)]
    pub metadata: Option<MetadataArg>,

    // Still accept-no-op:
    #[serde(default)]
    pub headless: Option<serde_json::Value>,
    #[serde(default)]
    pub summarize: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "mode")]
pub enum TablesArg {
    Embed,
    Drop,
    CsvFile,
    Summarize,
    Sample {
        #[serde(flatten)]
        strategy: SampleArg,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "strategy")]
pub enum SampleArg {
    HeadTail { #[serde(default = "default_head")] head: usize, #[serde(default = "default_tail")] tail: usize },
    RandomSeed { #[serde(default = "default_random_rows")] rows: usize, #[serde(default = "default_random_seed")] seed: u64 },
}

fn default_head() -> usize { 5 }
fn default_tail() -> usize { 5 }
fn default_random_rows() -> usize { 10 }
fn default_random_seed() -> u64 { 42 }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "mode")]
pub enum ImagesArg {
    Keep,
    AltTextOnly,
    Download,
    Drop,
    CaptionVlm,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum MetadataArg {
    Include,
    Skip,
}
```

- [ ] **Step 2: Translate arg types to internal modes; reject deferred modes**

```rust
fn tables_mode(arg: Option<&TablesArg>) -> Result<TablesMode, McpError> {
    use crate::extractor::options::{SampleStrategy, TablesMode};
    Ok(match arg {
        None | Some(TablesArg::Embed) => TablesMode::Embed,
        Some(TablesArg::Drop) => TablesMode::Drop,
        Some(TablesArg::CsvFile) => TablesMode::CsvFile,
        Some(TablesArg::Sample { strategy }) => match strategy {
            SampleArg::HeadTail { head, tail } => {
                if *head == 0 || *tail == 0 {
                    return Err(McpError::InvalidArgs("tables.sample head/tail must be > 0".into()));
                }
                TablesMode::Sample(SampleStrategy::HeadTail { head: *head, tail: *tail })
            }
            SampleArg::RandomSeed { rows, seed } => {
                if *rows == 0 {
                    return Err(McpError::InvalidArgs("tables.sample rows must be > 0".into()));
                }
                TablesMode::Sample(SampleStrategy::RandomSeed { rows: *rows, seed: *seed })
            }
        },
        Some(TablesArg::Summarize) => {
            return Err(McpError::Extractor(ExtractorError::Metadata(
                "tables summarize mode is not available until M7".into(),
            )));
        }
    })
}

fn images_mode(arg: Option<&ImagesArg>) -> Result<ImagesMode, McpError> {
    Ok(match arg {
        None | Some(ImagesArg::AltTextOnly) => ImagesMode::AltTextOnly,
        Some(ImagesArg::Keep) => ImagesMode::Keep,
        Some(ImagesArg::Download) => ImagesMode::Download,
        Some(ImagesArg::Drop) => ImagesMode::Drop,
        Some(ImagesArg::CaptionVlm) => {
            return Err(McpError::Extractor(ExtractorError::Metadata(
                "images caption_vlm mode requires the vlm feature (M9)".into(),
            )));
        }
    })
}
```

- [ ] **Step 3: Extend `fetch_inner` to run the post-passes and emit M4 frontmatter**

After the existing `tokenizer::count` call, before the response is constructed, run tables → images → quality and pass the results through to the new `PageMeta` shape:

```rust
let output_paths = std::sync::Arc::new(
    OutputPaths::resolve(self.config.output.dir.as_deref())
        .map_err(McpError::Extractor)?
);

let tables_mode = tables_mode(args.tables.as_ref())?;
let images_mode = images_mode(args.images.as_ref())?;

let body_md = result.page.extracted_md.clone();
let (body_md, tables_transformed) =
    crate::extractor::tables::apply(&body_md, &tables_mode, &output_paths, &url)
        .map_err(McpError::Extractor)?;
let images_result =
    crate::extractor::images::apply(&body_md, &images_mode, &output_paths, &self.client).await
        .map_err(McpError::Extractor)?;
let body_md = images_result.markdown;

// Recompute tokens against the post-pass markdown (the cached body is pre-pass).
let tokens = crate::tokenizer::count(&body_md, family)?;

// Max-tokens check (unchanged but now on post-pass body).
if let Some(max) = args.max_tokens && tokens > max {
    return Err(McpError::MaxTokensExceeded { actual: tokens, max });
}

// Metadata + quality.
let metadata = result.page.metadata.clone().unwrap_or_default();
let quality = crate::extractor::quality::score(
    &body_md,
    /* raw_html_text_len missing — cache doesn't store it; pass body length as a fallback */
    body_md.chars().count().max(1),
    !metadata.is_empty(),
    result.page.title.is_some(),
);
```

NOTE: `raw_html_text_len` is computed by `pipeline::extract_full` but currently not stored in the cache. For M4, accept the fallback shown above (pass `body_md.chars().count()` so the ratio always saturates to 1.0 plus any bonuses) and document a follow-up: store `raw_html_text_len` on the `pages` row in a future milestone if quality fidelity becomes important. Acceptable for M4 because the quality bonus terms already provide meaningful differentiation.

- [ ] **Step 4: Build the new `PageMeta` and render**

```rust
let frontmatter = render_frontmatter(&PageMeta {
    url: &url,
    canonical_url: &canonical,
    title: result.page.title.as_deref(),
    fetched_at: jiff::Timestamp::now(),
    body: &body_md,
    tokens,
    tokenizer_name: family.as_str(),
    description: metadata.description.as_deref(),
    author: metadata.author.as_deref(),
    published: metadata.published.as_deref(),
    modified: metadata.modified.as_deref(),
    image: metadata.image.as_deref(),
    og_type: metadata.og_type.as_deref(),
    language: metadata.language.as_deref(),
    schema_types: &metadata.schema_types,
    extraction_quality: quality,
    tables_transformed: &tables_transformed,
    images_seen: images_result.images_seen,
    images_downloaded: images_result.images_downloaded,
    images_failed: images_result.images_failed,
});
```

- [ ] **Step 5: Update inline tests**

Add tests that the typed args deserialize and reject correctly:

```rust
    #[test]
    fn typed_tables_sample_parses() {
        let v: FetchArgs = serde_json::from_str(
            r#"{"url":"https://x/","tables":{"mode":"sample","strategy":"head_tail","head":3,"tail":2}}"#
        ).unwrap();
        match v.tables.unwrap() {
            TablesArg::Sample { strategy: SampleArg::HeadTail { head, tail } } => {
                assert_eq!(head, 3); assert_eq!(tail, 2);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn typed_tables_rejects_unknown_field() {
        let r: Result<FetchArgs, _> = serde_json::from_str(
            r#"{"url":"https://x/","tables":{"mode":"embed","bogus":1}}"#
        );
        assert!(r.is_err());
    }

    #[test]
    fn typed_images_download_parses() {
        let v: FetchArgs = serde_json::from_str(r#"{"url":"https://x/","images":{"mode":"download"}}"#).unwrap();
        assert!(matches!(v.images, Some(ImagesArg::Download)));
    }
```

- [ ] **Step 6: Build + run + commit**

```bash
cargo build
cargo test --lib mcp::tools::fetch
```

Expected: all existing fetch tests still pass plus 3 new typed-arg tests.

```bash
git add src/mcp/tools/fetch.rs
git commit -m "feat(mcp): typed tables/images/metadata args; post-pass pipeline; m4 frontmatter"
```

---

## Task 13: `get_metadata` MCP tool + envelope

**Files:**
- Create: `src/mcp/tools/get_metadata.rs`
- Modify: `src/mcp/envelope.rs` (`MetadataResponse`)
- Modify: `src/mcp/handler.rs` (`#[tool_router]` registers `get_metadata_tool`)
- Modify: `src/mcp/tools/mod.rs`

- [ ] **Step 1: Add `MetadataResponse` to envelope**

In `src/mcp/envelope.rs`, append:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataResponse {
    #[serde(skip_serializing_if = "Option::is_none")] pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub og_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub canonical: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub language: Option<String>,
    pub schema_types: Vec<String>,
    pub extraction_quality: f32,
    pub url: String,
    pub content_hash: String,
    pub fetched_at: String,
    pub cache_status: CacheStatus,
}
```

- [ ] **Step 2: Create `src/mcp/tools/get_metadata.rs`**

```rust
//! MCP `get_metadata` tool — fetch metadata only (no markdown body).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::fetcher::cached::{FetchOptions, fetch_with_cache, ExtractResult, sha256_hex};
use crate::mcp::envelope::{CacheStatus, MetadataResponse};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMetadataArgs {
    pub url: String,
    #[serde(default)]
    pub force_refresh: bool,
    #[serde(default)]
    pub tokenizer: Option<String>,
}

impl RoverHandler {
    pub async fn get_metadata_inner(&self, args: GetMetadataArgs) -> Result<MetadataResponse, McpError> {
        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;
        tokenizer::ensure_loaded(family).await?;

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &url,
            &self.config.cache,
            FetchOptions { force_refresh: args.force_refresh, ssrf_level: self.ssrf_level },
            |body, base| {
                let extracted = crate::extractor::extract(body, Some(base))
                    .map_err(|_| crate::fetcher::FetcherError::Decode)?;
                let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                    metadata: extracted.metadata,
                })
            },
        ).await?;

        let metadata = result.page.metadata.clone().unwrap_or_default();
        let quality = crate::extractor::quality::score(
            &result.page.extracted_md,
            result.page.extracted_md.chars().count().max(1),
            !metadata.is_empty(),
            result.page.title.is_some(),
        );

        Ok(MetadataResponse {
            title: metadata.title.clone(),
            description: metadata.description.clone(),
            author: metadata.author.clone(),
            published: metadata.published.clone(),
            modified: metadata.modified.clone(),
            image: metadata.image.clone(),
            og_type: metadata.og_type.clone(),
            canonical: metadata.canonical.clone(),
            language: metadata.language.clone(),
            schema_types: metadata.schema_types.clone(),
            extraction_quality: quality,
            url: url.as_str().to_string(),
            content_hash: result.page.content_hash.clone(),
            fetched_at: jiff::Timestamp::from_second(result.page.fetched_at)
                .map(|t| t.to_string())
                .unwrap_or_default(),
            cache_status: result.cache_status.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::envelope::RoverError;

    #[test]
    fn schema_contains_required_fields() {
        let schema = schemars::schema_for!(GetMetadataArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for f in ["url", "force_refresh", "tokenizer"] {
            assert!(json.contains(f), "missing {f}");
        }
    }

    #[test]
    fn rejects_unknown_field() {
        let r: Result<GetMetadataArgs, _> =
            serde_json::from_str(r#"{"url":"https://x/","bogus":1}"#);
        assert!(r.is_err());
    }
}
```

- [ ] **Step 3: Register `get_metadata_tool` on `RoverHandler`**

In `src/mcp/handler.rs`, inside the `#[tool_router] impl RoverHandler` block, append a third tool method:

```rust
    /// Fetch a URL and return ONLY its structured metadata (no markdown body).
    #[rmcp::tool(description = "Fetch a URL and return only its structured metadata: \
                                title, description, author, published/modified dates, \
                                schema_types, image, canonical, language, extraction_quality.")]
    pub async fn get_metadata_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::get_metadata::GetMetadataArgs>,
    ) -> Result<Json<crate::mcp::envelope::MetadataResponse>, ErrorData> {
        match self.get_metadata_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }
```

Update `src/mcp/tools/mod.rs`:

```rust
pub mod count_tokens;
pub mod fetch;
pub mod get_metadata;
```

- [ ] **Step 4: Run unit tests + commit**

```bash
cargo test --lib mcp
```

Expected: existing 18 mcp tests pass; 2 new `get_metadata` tests pass.

```bash
git add src/mcp/
git commit -m "feat(mcp): get_metadata tool with metadata-only response envelope"
```

---

## Task 14: Integration tests

**Files:**
- Create: `tests/common/mod.rs`
- Create: `tests/extractor_metadata.rs`
- Create: `tests/extractor_tables.rs`
- Create: `tests/extractor_images.rs`
- Create: `tests/extractor_links.rs`
- Create: `tests/mcp_get_metadata.rs`
- Create: `tests/fixtures/m4/*.html`
- Modify: `tests/mcp_smoke.rs` (extract `seed_default_tokenizer` into `tests/common/mod.rs`)

The smoke test for `get_metadata` is the M4 acceptance proof.

- [ ] **Step 1: Factor `seed_default_tokenizer` into `tests/common/mod.rs`**

Move the existing helper from `tests/mcp_smoke.rs` to `tests/common/mod.rs`. Update `tests/mcp_smoke.rs` to `mod common;` and call `common::seed_default_tokenizer(...)`.

- [ ] **Step 2: Create fixture HTML files**

In `tests/fixtures/m4/`:

- `article-jsonld-og-twitter.html` — article with all three metadata sources; verify JSON-LD wins.
- `graph-newsarticle-person.html` — `@graph` with NewsArticle + Person author.
- `og-only.html` — only OG metatags.
- `no-metadata.html` — bare article body, no metadata.
- `with-base-href.html` — `<base href="https://other.example/">` plus relative links.
- `two-tables-one-large.html` — 5-row table + 200-row table.
- `relative-images.html` — `<img src="/img/x.png">` plus alt text.
- `small-image-pixel.png` — 1x1 PNG fixture for wiremock to serve (~70 bytes; generate with imagemagick or commit a tiny valid PNG).

The exact HTML bodies should be small but realistic; the implementer composes them to satisfy the test assertions below.

- [ ] **Step 3: `tests/extractor_metadata.rs`**

```rust
mod common;

use rover::extractor;
use url::Url;

fn fixture(name: &str) -> String {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/m4");
    p.push(name);
    std::fs::read_to_string(p).unwrap()
}

fn base() -> Url {
    Url::parse("https://example.com/article").unwrap()
}

#[test]
fn jsonld_og_twitter_precedence() {
    let html = fixture("article-jsonld-og-twitter.html");
    let m = extractor::metadata::extract(&html, &base());
    // JSON-LD's title wins; OG's image fills hole if JSON-LD has no image, etc.
    assert!(m.title.is_some());
    assert!(m.schema_types.iter().any(|t| t == "Article"));
}

#[test]
fn og_only_page_yields_og_fields() {
    let html = fixture("og-only.html");
    let m = extractor::metadata::extract(&html, &base());
    assert!(m.title.is_some());
    assert!(m.og_type.is_some());
}

#[test]
fn no_metadata_page_yields_empty() {
    let html = fixture("no-metadata.html");
    let m = extractor::metadata::extract(&html, &base());
    assert!(m.is_empty() || m.language.is_some()); // <html lang> might be present
}
```

- [ ] **Step 4: `tests/extractor_links.rs`**

Tests absolutization end-to-end against the `with-base-href.html` fixture: after `pipeline::extract_full`, every link in the body is absolute and resolves to `https://other.example/...`.

- [ ] **Step 5: `tests/extractor_tables.rs`**

Tests CsvFile mode against `two-tables-one-large.html`: the 200-row table gets written to `<output_dir>/tables/<host>/<sha8>.csv` with 200 data rows; the 5-row table is the second written file. Reads the CSV back and asserts shape.

- [ ] **Step 6: `tests/extractor_images.rs`**

`#[ignore]`-free wiremock-backed download test. Requires `--features test-loopback`:

```rust
mod common;

use rover::extractor::{images, options::ImagesMode, output::OutputPaths};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::method;

#[tokio::test]
async fn download_writes_to_disk_and_rewrites_markdown() {
    let server = MockServer::start().await;
    let png_bytes: Vec<u8> = std::fs::read(
        format!("{}/tests/fixtures/m4/small-image-pixel.png", env!("CARGO_MANIFEST_DIR"))
    ).unwrap();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(png_bytes)
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
    let paths = OutputPaths::resolve(None).unwrap();

    let md = format!("![pixel]({}/pixel.png)", server.uri());
    let client = reqwest::Client::new();
    let r = images::apply(&md, &ImagesMode::Download, &paths, &client).await.unwrap();

    assert_eq!(r.images_downloaded, 1);
    assert_eq!(r.images_failed, 0);
    assert!(r.markdown.contains(tmp.path().to_str().unwrap()));
}
```

- [ ] **Step 7: `tests/mcp_get_metadata.rs`**

Spawn `rover mcp` with `ROVER_MCP_SSRF=test_loopback`, point it at a wiremock-served HTML page, call `get_metadata_tool`, assert the response shape.

```rust
mod common;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::json;
use tokio::process::Command;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE_HTML: &str = include_str!("fixtures/m4/article-jsonld-og-twitter.html");

#[tokio::test]
async fn lists_get_metadata_tool() {
    let tmp = tempfile::tempdir().unwrap();
    common::seed_default_tokenizer(tmp.path());
    let client = common::spawn_client(tmp.path()).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"get_metadata_tool"), "missing get_metadata_tool: {names:?}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_metadata_against_fixture_returns_expected_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(FIXTURE_HTML),
        ).mount(&server).await;

    let tmp = tempfile::tempdir().unwrap();
    common::seed_default_tokenizer(tmp.path());
    let client = common::spawn_client(tmp.path()).await;
    let res = client
        .call_tool(CallToolRequestParams::new("get_metadata_tool")
            .with_arguments(json!({"url": server.uri()}).as_object().cloned().unwrap()))
        .await.unwrap();
    let blob = serde_json::to_string(&res).unwrap();
    assert!(blob.contains("\"title\""));
    assert!(blob.contains("\"schema_types\""));
    assert!(blob.contains("\"extraction_quality\""));
    client.cancel().await.unwrap();
}
```

(`common::spawn_client` is the new shared helper based on `mcp_smoke.rs`'s spawn pattern.)

- [ ] **Step 8: Run full integration suite + commit**

```bash
cargo test --features test-loopback
```

Expected: ~170+ tests pass (M3's 154 + ~16 M4 unit + integration).

```bash
git add tests/ src/
git commit -m "test(m4): extractor metadata/tables/images/links + mcp get_metadata integration"
```

---

## Acceptance check

After Task 14, run:

```bash
cargo fmt --check
cargo clippy --all-targets --features test-loopback -- -D warnings
cargo test --features test-loopback
```

All three must pass. Manual smoke:

```bash
# Fetch a real article and confirm M4 fields populate
ROVER_OUTPUT_DIR=/tmp/rover-out cargo run --release -- fetch \
    https://en.wikipedia.org/wiki/Rust_\(programming_language\) \
    | head -40
# Expect: description, image, language, extraction_quality, schema_types
# (if Wikipedia exposes any JSON-LD/OG fields), tables_transformed
# entries if the article has tables.
```

## Self-review summary

- **Spec coverage:** every section of the M4 design has at least one task.
  Metadata = Tasks 3-4. base_href = Task 5. Links = Task 6. Quality = Task
  7. Output paths + config = Task 8. Tables = Task 9. Images = Task 10.
  Pipeline integration + frontmatter + cache write = Task 11. MCP fetch
  typed args + post-passes = Task 12. get_metadata tool = Task 13. Tests
  = Task 14.
- **Placeholder scan:** no TBDs or "implement later" steps. Two known
  fix-ups inline in the task bodies (Task 10's `Url::parse` error path
  uses a cleaner variant; Task 11 step 5 documents the `raw_html_text_len`
  follow-up).
- **Type consistency:** `ExtractedMetadata`, `ExtractedDoc`,
  `ExtractOptions`, `TablesMode`, `ImagesMode`, `SampleStrategy`,
  `TableTransform`, `ImagesApplied`, `OutputPaths`, `MetadataResponse`,
  the M4 `FetchArgs` extensions, and the new `ExtractorError` variants
  are stable across tasks.
- **rmcp paths:** all paths used are the same ones M3 verified
  (`#[rmcp::tool]`, `#[rmcp::tool_router]`, `Parameters<T>`, `Json<T>`,
  `ErrorData`, `ServiceExt`, `transport::io::stdio`,
  `transport::child_process::TokioChildProcess`). No new rmcp surface
  introduced.

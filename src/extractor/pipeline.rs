//! Content extraction pipeline (PRD §6.1).
//!
//! `bytes → charset_detect → utf8 → readabilityrs → markdown_postprocess`.
//!
//! Charset detection is the fetcher's job (see `fetcher::charset`); this
//! module receives a UTF-8 string and returns markdown.

use readabilityrs::{
    MarkdownOptions, Readability, ReadabilityOptions,
    markdown::options::{HeadingStyle, LinkStyle},
};
use thiserror::Error;
use url::Url;

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

    #[error("invalid image url {url}: {source}")]
    ImageUrlInvalid {
        url: String,
        #[source]
        source: url::ParseError,
    },
}

/// Successfully extracted article.
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

/// Build the markdown options Rover prefers (PRD §6.1: ATX headings, backtick
/// fences, dash bullets, inline links).
fn rover_markdown_options() -> MarkdownOptions {
    MarkdownOptions {
        heading_style: HeadingStyle::Atx,
        bullet_char: '-',
        code_fence: '`',
        emphasis_delimiter: '*',
        strong_delimiter: "**".to_string(),
        link_style: LinkStyle::Inline,
        preserve_complex_tables: true,
    }
}

/// Extract the article from `html`, resolving relative links against `base_url`.
///
/// Runs the two-pass M4 shape:
///   1. Pre-pass on raw HTML — read `<base href>` and extract structured
///      metadata (JSON-LD / OG / Twitter / `<html lang>` / canonical).
///   2. readabilityrs main pass against the effective base.
///   3. Post-pass — absolutize relative links/images in the markdown body.
pub fn extract_full(html: &str, base_url: &Url) -> Result<ExtractedDoc, ExtractorError> {
    // Pre-pass: base href + metadata, on raw HTML.
    let effective_base =
        crate::extractor::base_href::read_base_href(html).unwrap_or_else(|| base_url.clone());
    let metadata = crate::extractor::metadata::extract(html, &effective_base);
    let raw_html_text_len = approximate_html_text_len(html);

    // readabilityrs main pass.
    let opts = ReadabilityOptions::builder()
        .output_markdown(true)
        .markdown_options(rover_markdown_options())
        .build();
    let readability = Readability::new(html, Some(effective_base.as_str()), Some(opts))
        .map_err(|e| ExtractorError::Readability(e.to_string()))?;
    let article = readability.parse().ok_or(ExtractorError::NoArticle)?;

    let body_md = article.markdown_content.unwrap_or_default();

    // Post-pass: absolutize links/images against the effective base.
    let body_md = crate::extractor::links::absolutize(&body_md, &effective_base);

    Ok(ExtractedDoc {
        title: article.title.or_else(|| metadata.title.clone()),
        body_md,
        language: article.lang.or_else(|| metadata.language.clone()),
        byline: article.byline,
        excerpt: article.excerpt,
        site_name: article.site_name,
        published_time: article
            .published_time
            .or_else(|| metadata.published.clone()),
        image: article.image.or_else(|| metadata.image.clone()),
        metadata,
        raw_html_text_len,
    })
}

/// Backwards-compatible wrapper for callers that don't have a base `Url`.
pub fn extract(html: &str, base_url: Option<&Url>) -> Result<ExtractedDoc, ExtractorError> {
    let base = base_url
        .cloned()
        .unwrap_or_else(|| Url::parse("about:blank").unwrap());
    extract_full(html, &base)
}

/// Approximate the visible-text length of `html` by counting characters
/// in the `<body>`'s text descendants. Falls back to the full input length
/// when no `<body>` is present (defends against fragment HTML).
fn approximate_html_text_len(html: &str) -> usize {
    let doc = scraper::Html::parse_document(html);
    let body_sel = scraper::Selector::parse("body").unwrap();
    doc.select(&body_sel)
        .next()
        .map(|b| b.text().map(|t| t.chars().count()).sum())
        .unwrap_or_else(|| html.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <title>Sample Article About How To Do The Thing</title>
  <meta http-equiv="Content-Language" content="en" />
</head>
<body>
  <article>
    <h1>Sample Article About How To Do The Thing</h1>
    <h2>How to do the thing</h2>
    <p>This is a long paragraph of body content. It needs to be substantial enough that
       readabilityrs identifies it as the article. Otherwise the extractor will fall back
       to no-article, which is what we want to avoid in this test. The content has to
       cross the default character threshold of 500 characters, so we need a few sentences
       of filler. Here is more filler. Lorem ipsum dolor sit amet, consectetur adipiscing
       elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>
    <p>Second paragraph with a <a href="/relative">relative link</a> and a <a href="https://example.com/abs">absolute link</a>.</p>
  </article>
</body>
</html>
"#;

    #[test]
    fn extracts_title_and_body() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        assert!(doc.title.unwrap().contains("Sample Article"));
        assert!(doc.body_md.contains("How to do the thing"));
        assert!(doc.body_md.contains("filler"));
    }

    #[test]
    fn produces_atx_headings() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        // ATX heading is `## Heading`, not the Setext underline form.
        assert!(doc.body_md.contains("## How to do the thing"));
    }

    #[test]
    fn captures_language() {
        let url = Url::parse("https://example.com/page").unwrap();
        let doc = extract(SAMPLE_HTML, Some(&url)).expect("extract ok");
        assert_eq!(doc.language.as_deref(), Some("en"));
    }
}

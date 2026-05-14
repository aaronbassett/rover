//! Markdown link/image post-pass: rewrite relative URLs to absolute.

use url::Url;

pub fn absolutize(markdown: &str, _base: &Url) -> String {
    markdown.to_string()
}

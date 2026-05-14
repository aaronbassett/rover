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

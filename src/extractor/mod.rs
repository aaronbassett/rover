//! Content extraction pipeline.

pub mod frontmatter;
pub mod pipeline;

pub use pipeline::{ExtractedDoc, ExtractorError, extract};

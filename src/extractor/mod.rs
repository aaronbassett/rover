//! Content extraction pipeline.

pub mod base_href;
pub mod frontmatter;
pub mod image_dims;
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

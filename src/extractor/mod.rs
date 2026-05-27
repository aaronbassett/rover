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

/// Process-wide mutex shared by every extractor test module that mutates
/// the `ROVER_OUTPUT_DIR` env var consumed by [`OutputPaths::resolve`].
///
/// Previously `output`, `tables`, and `images` each defined their own
/// `static TEST_MUTEX`, but std `Mutex` identity is per-static — three
/// instances meant tests in different files raced against each other on
/// the same global env var, producing a flaky failure on
/// `output::tests::resolve_honors_env_then_config_then_default`. The
/// single shared static gives all three test modules the same lock.
#[cfg(test)]
pub(crate) static OUTPUT_DIR_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

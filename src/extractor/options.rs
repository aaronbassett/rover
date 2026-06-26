//! Per-fetch extraction options carried through the pipeline.

use std::sync::Arc;

use crate::extractor::output::OutputPaths;
use crate::storage::Db;
use crate::vlm::CaptionerRegistry;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub tables: TablesMode,
    pub images: ImagesMode,
    pub metadata: MetadataMode,
    pub output_paths: Arc<OutputPaths>,

    /// M9: captioner registry (always present in default builds since cloud
    /// captioners ship in every binary). `None` only during very early tests
    /// or when no `[captioners.*]` are configured.
    pub captioners: Option<Arc<CaptionerRegistry>>,
    pub caption_filters: ImageCaptionFilters,
    pub db: Option<Db>,
}

#[derive(Debug, Clone, Default)]
pub enum MetadataMode {
    #[default]
    Include,
    Skip,
}

#[derive(Debug, Clone, Default)]
pub enum TablesMode {
    #[default]
    Embed,
    Sample(SampleStrategy),
    CsvFile,
    Drop,
    Summarize,
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
    /// Caption each `<img>` via a configured `[captioners.<name>]` (M9).
    /// When no captioner is configured at fetch time, the apply() call
    /// returns ExtractorError::CaptionerNotConfigured.
    Caption,
}

/// Per-fetch caption-mode budget knobs. Resolved from `[image_captions]`
/// at server startup; cloned per-fetch with any per-call overrides applied.
#[derive(Debug, Clone)]
pub struct ImageCaptionFilters {
    pub max_per_page: usize,
    pub min_width: u32,
    pub min_height: u32,
    pub max_bytes: u64,
    pub max_tokens: usize,
    /// When Some, overrides the registry's default captioner for this fetch.
    pub captioner_override: Option<String>,
    /// Maximum concurrent image HTTP requests to any single hostname.
    /// Applied via [`crate::extractor::image_limiter::DomainLimiter`].
    /// A value of 0 is treated as 1 (always issues at least one request).
    pub per_domain_concurrency: u32,
    /// Maximum number of captioner provider calls allowed to run concurrently
    /// across the whole page. Bounds the per-batch caption fan-out via a
    /// [`tokio::sync::Semaphore`]. A value of 0 is treated as 1.
    pub max_concurrent: usize,
    /// Hard cap on the number of *real* captioner provider calls per page.
    /// The selection loop stops once this many `captioner.caption(...)`
    /// invocations have been made, independent of how many succeed. Cache
    /// hits (Task 10) do not consume this budget. Resolved from
    /// `[image_captions]` at Task 11 (default `3 * max_per_page` when unset).
    pub max_attempts: usize,
}

impl Default for ImageCaptionFilters {
    fn default() -> Self {
        Self {
            max_per_page: 10,
            min_width: 200,
            min_height: 200,
            max_bytes: 10 * 1024 * 1024,
            max_tokens: 50,
            captioner_override: None,
            per_domain_concurrency: 2,
            max_concurrent: 2,
            max_attempts: 30,
        }
    }
}

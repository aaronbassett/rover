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

#[derive(Debug, Clone, Default)]
pub enum TablesMode {
    #[default]
    Embed,
    Sample(SampleStrategy),
    CsvFile,
    Drop,
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

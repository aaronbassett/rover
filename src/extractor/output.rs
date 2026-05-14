//! Output paths for table CSVs and downloaded images.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OutputPaths {
    // constructed by Task 9 (images download) / Task 8 (table CSVs)
    #[allow(dead_code)]
    pub(crate) root: PathBuf,
}

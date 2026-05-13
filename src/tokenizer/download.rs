//! HuggingFace tokenizer file downloader.
//!
//! Uses `hf-hub`'s sync API behind `spawn_blocking`. The on-disk layout is
//! `$XDG_DATA_HOME/rover/tokenizers/<family>/tokenizer.json`. If the file is
//! already present, the function is a no-op; otherwise it pulls from HF and
//! copies into place.

use std::fs;
use std::path::{Path, PathBuf};

use hf_hub::api::sync::{Api, ApiBuilder};

use crate::tokenizer::{Tokenizer, TokenizerError};

/// Ensure the tokenizer.json for `family` exists under `root`, downloading
/// from HuggingFace if missing. Blocks; call inside `spawn_blocking`.
pub fn ensure_on_disk(root: &Path, family: Tokenizer) -> Result<PathBuf, TokenizerError> {
    let dest_dir = root.join(family.as_str());
    let dest_file = dest_dir.join("tokenizer.json");
    if dest_file.exists() {
        return Ok(dest_file);
    }

    fs::create_dir_all(&dest_dir).map_err(|source| TokenizerError::Io {
        path: dest_dir.display().to_string(),
        source,
    })?;

    let (repo_id, filename) = family.hf_source();
    tracing::info!(
        target: "rover::tokenizer",
        family = %family,
        repo = repo_id,
        "downloading tokenizer from HuggingFace"
    );

    let api: Api = ApiBuilder::new()
        .with_progress(false)
        .build()
        .map_err(|e| TokenizerError::Download { family, source: e })?;
    let staged: PathBuf = api
        .model(repo_id.to_string())
        .get(filename)
        .map_err(|e| TokenizerError::Download { family, source: e })?;

    // hf-hub places the file in its own cache. Copy (not symlink — Windows
    // would need elevated perms) into XDG so removal of the hf-hub cache
    // doesn't leave us with a dangling tokenizer.
    fs::copy(&staged, &dest_file).map_err(|source| TokenizerError::Io {
        path: dest_file.display().to_string(),
        source,
    })?;

    let size = fs::metadata(&dest_file).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        target: "rover::tokenizer",
        family = %family,
        bytes = size,
        path = %dest_file.display(),
        "downloaded tokenizer"
    );

    Ok(dest_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_present_file_is_returned_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("cl100k");
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("tokenizer.json");
        fs::write(&f, "{}").unwrap();

        let result = ensure_on_disk(root, Tokenizer::Cl100k).unwrap();
        assert_eq!(result, f);
    }
}

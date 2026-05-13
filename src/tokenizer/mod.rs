//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. The public surface is two
//! functions:
//!
//!   - [`ensure_loaded`] is async; it downloads (if needed) and parses the
//!     tokenizer into a process-wide cache.
//!   - [`count`] is synchronous; it returns a token count from the cached
//!     tokenizer. Returns [`TokenizerError::NotLoaded`] if `ensure_loaded`
//!     hasn't been called for the family.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

pub mod download;
pub mod error;
pub mod hf;
pub mod registry;

pub use error::TokenizerError;
pub use hf::HfTokenizer;
pub use registry::Tokenizer;

/// Process-wide registry. Initialised on first access.
fn registry() -> &'static RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>> {
    static REG: OnceLock<RwLock<HashMap<Tokenizer, Arc<HfTokenizer>>>> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Compute the XDG tokenizer root (`$XDG_DATA_HOME/rover/tokenizers` or the
/// platform-appropriate fallback). The `ROVER_DATA_DIR` env var, when set,
/// overrides the parent. Mirrors `cli::fetch::data_dir`.
pub fn xdg_root() -> Result<PathBuf, TokenizerError> {
    if let Ok(env_dir) = std::env::var("ROVER_DATA_DIR") {
        let p = PathBuf::from(env_dir).join("tokenizers");
        return Ok(p);
    }
    let base = dirs::data_local_dir().ok_or_else(|| TokenizerError::Io {
        path: "<data_local_dir>".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no data dir"),
    })?;
    Ok(base.join("rover").join("tokenizers"))
}

/// Download (if needed) and parse the tokenizer for `family` into the
/// process-wide cache. Subsequent calls for the same family are no-ops.
pub async fn ensure_loaded(family: Tokenizer) -> Result<(), TokenizerError> {
    if registry()
        .read()
        .expect("tokenizer registry rwlock poisoned")
        .contains_key(&family)
    {
        return Ok(());
    }

    let root = xdg_root()?;
    let path: PathBuf =
        tokio::task::spawn_blocking(move || download::ensure_on_disk(&root, family))
            .await
            .map_err(|e| TokenizerError::Io {
                path: "<spawn_blocking>".to_string(),
                source: std::io::Error::other(format!("spawn_blocking join failed: {e}")),
            })??;

    let parsed = tokio::task::spawn_blocking(move || HfTokenizer::from_path(&path, family))
        .await
        .map_err(|e| TokenizerError::Io {
            path: "<spawn_blocking>".to_string(),
            source: std::io::Error::other(format!("spawn_blocking join failed: {e}")),
        })??;

    registry()
        .write()
        .expect("tokenizer registry rwlock poisoned")
        .insert(family, Arc::new(parsed));
    Ok(())
}

/// Synchronously count tokens in `text` using the cached tokenizer for
/// `family`. Returns [`TokenizerError::NotLoaded`] if [`ensure_loaded`] has
/// not been called.
pub fn count(text: &str, family: Tokenizer) -> Result<usize, TokenizerError> {
    let map = registry()
        .read()
        .expect("tokenizer registry rwlock poisoned");
    let tk = map.get(&family).ok_or(TokenizerError::NotLoaded(family))?;
    tk.count(text)
}

/// Test-only: clear the global registry. Used by unit tests to keep state
/// independent.
#[cfg(test)]
pub(crate) fn _clear_registry_for_tests() {
    registry()
        .write()
        .expect("tokenizer registry rwlock poisoned")
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/tokenizer/tiny.json");
        p
    }

    #[test]
    fn count_without_load_errors() {
        _clear_registry_for_tests();
        let err = count("abab", Tokenizer::Llama3).unwrap_err();
        assert!(matches!(err, TokenizerError::NotLoaded(Tokenizer::Llama3)));
    }

    #[tokio::test]
    async fn manual_insert_then_count_works() {
        _clear_registry_for_tests();
        let tk = HfTokenizer::from_path(&fixture(), Tokenizer::Cl100k).unwrap();
        registry()
            .write()
            .unwrap()
            .insert(Tokenizer::Cl100k, Arc::new(tk));
        assert_eq!(count("abab", Tokenizer::Cl100k).unwrap(), 2);
    }
}

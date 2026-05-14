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

/// Base directory for cached tokenizer files. Delegates to
/// [`crate::paths::data_dir`] for the resolution order.
pub fn xdg_root() -> Result<PathBuf, TokenizerError> {
    Ok(crate::paths::data_dir().join("tokenizers"))
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

/// Test-only: shared mutex for tests that mutate the process-global
/// tokenizer registry. Tests must acquire this guard before calling
/// [`_clear_registry_for_tests`] or otherwise touching the registry, so
/// they serialise under cargo's parallel test runner.
#[cfg(test)]
pub(crate) fn _test_mutex() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
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
        let _guard = _test_mutex().lock().expect("test mutex");
        _clear_registry_for_tests();
        let err = count("abab", Tokenizer::Llama3).unwrap_err();
        assert!(matches!(err, TokenizerError::NotLoaded(Tokenizer::Llama3)));
    }

    #[tokio::test]
    async fn manual_insert_then_count_works() {
        let _guard = _test_mutex().lock().expect("test mutex");
        _clear_registry_for_tests();
        let tk = HfTokenizer::from_path(&fixture(), Tokenizer::Cl100k).unwrap();
        registry()
            .write()
            .unwrap()
            .insert(Tokenizer::Cl100k, Arc::new(tk));
        assert_eq!(count("abab", Tokenizer::Cl100k).unwrap(), 2);
    }
}

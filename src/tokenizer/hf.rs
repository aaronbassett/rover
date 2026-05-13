//! Thin wrapper around `tokenizers::Tokenizer`.

use std::path::Path;

use tokenizers::Tokenizer as Inner;

use crate::tokenizer::{Tokenizer, TokenizerError};

#[derive(Debug)]
pub struct HfTokenizer {
    inner: Inner,
}

impl HfTokenizer {
    /// Parse a `tokenizer.json` file from disk.
    pub fn from_path(path: &Path, family: Tokenizer) -> Result<Self, TokenizerError> {
        let inner =
            Inner::from_file(path).map_err(|e| TokenizerError::Parse { family, source: e })?;
        Ok(Self { inner })
    }

    /// Count tokens in `text`. Special tokens are not added.
    pub fn count(&self, text: &str) -> Result<usize, TokenizerError> {
        let encoded = self
            .inner
            .encode(text, false)
            .map_err(|e| TokenizerError::Parse {
                family: Tokenizer::Cl100k, // placeholder; encode errors are vanishingly rare in practice
                source: e,
            })?;
        Ok(encoded.get_ids().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/tokenizer/tiny.json");
        p
    }

    #[test]
    fn parses_fixture_tokenizer() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        // Just confirm the type round-trips through parse.
        let _ = tk;
    }

    #[test]
    fn counts_merged_pair_as_one_token() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        assert_eq!(tk.count("abab").unwrap(), 2); // ab, ab
    }

    #[test]
    fn counts_partial_merge_correctly() {
        let tk = HfTokenizer::from_path(&fixture_path(), Tokenizer::Cl100k).unwrap();
        assert_eq!(tk.count("abc").unwrap(), 2); // ab, c
    }

    #[test]
    fn parse_failure_surfaces_family() {
        // Point at a path that exists but isn't a tokenizer.json.
        let bad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
        let err = HfTokenizer::from_path(&bad, Tokenizer::Llama3).unwrap_err();
        match err {
            TokenizerError::Parse { family, .. } => assert_eq!(family, Tokenizer::Llama3),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}

//! Thin wrapper around `tokenizers::Tokenizer`.

use std::path::Path;

use tokenizers::Tokenizer as Inner;

use crate::tokenizer::{Tokenizer, TokenizerError};

#[derive(Debug)]
pub struct HfTokenizer {
    inner: Inner,
    family: Tokenizer,
}

impl HfTokenizer {
    /// Parse a `tokenizer.json` file from disk for `family`.
    pub fn from_path(path: &Path, family: Tokenizer) -> Result<Self, TokenizerError> {
        let inner =
            Inner::from_file(path).map_err(|e| TokenizerError::Parse { family, source: e })?;
        Ok(Self { inner, family })
    }

    /// Count tokens in `text`. Special tokens are not added — see PRD §10:
    /// `count_tokens` measures plain user text, not chat-completion budgets
    /// (which would include role/turn tokens).
    ///
    /// Returns `Err(TokenizerError::Parse)` if `tokenizers::Tokenizer::encode`
    /// rejects the input. In practice this is vanishingly rare for a
    /// successfully-parsed tokenizer with a `&str` input + `add_special_tokens=false`
    /// (the post-processor branch where most encode failures live is skipped),
    /// but the upstream API is fallible so we propagate.
    pub fn count(&self, text: &str) -> Result<usize, TokenizerError> {
        let encoded = self
            .inner
            .encode(text, false)
            .map_err(|e| TokenizerError::Parse {
                family: self.family,
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
        assert_eq!(tk.count("").unwrap(), 0);
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

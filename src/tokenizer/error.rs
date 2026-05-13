//! Tokenizer module errors. Real variants land in Task 2.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("tokenizer module not yet initialised")]
    Stub,
}

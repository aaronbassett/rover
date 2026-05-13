//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. Tasks 2-4 fill this module in.

pub mod error;
pub mod registry;

pub use error::TokenizerError;
pub use registry::Tokenizer;

//! Token counting for the MCP layer and the frontmatter writer.
//!
//! Lazy-loads HuggingFace tokenizers from `$XDG_DATA_HOME/rover/tokenizers/`,
//! downloading on first use via `hf-hub`. Task 4 fills in the load+download
//! orchestration; this module currently exposes the registry, error type, and
//! parser-wrapper.

pub mod error;
pub mod hf;
pub mod registry;

pub use error::TokenizerError;
pub use registry::Tokenizer;

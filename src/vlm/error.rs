//! VLM (captioner) error types. Per-module thiserror enum.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VlmError {
    #[error("no such captioner: {name}")]
    NoSuchCaptioner { name: String },

    #[error(
        "local (mistralrs) captioning has been removed; configure a cloud captioner \
         (kind = \"cloud\") instead — for local inference, point provider = \"openai_compat\" \
         + base_url at an OpenAI-compatible server such as ollama or LM Studio"
    )]
    LocalFeatureNotCompiled,

    #[error("no captioners configured for image captioning")]
    NoCaptionersConfigured,

    #[error("captioner {name} unavailable: {reason}")]
    Unavailable { name: String, reason: String },

    #[error("captioner {name} rate limited")]
    RateLimited { name: String },

    #[error("captioner {name} auth failed")]
    AuthFailed { name: String },

    #[error("captioner {name} model error: {reason}")]
    ModelError { name: String, reason: String },

    #[error(
        "captioner {name}: model file {file} has been modified (expected {expected}, got {actual})"
    )]
    ModelIntegrityFailure {
        name: String,
        file: String,
        expected: String,
        actual: String,
    },

    #[error("captioner semaphore closed")]
    SemaphoreClosed,

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
}

//! VLM (captioner) error types. Per-module thiserror enum.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VlmError {
    #[error("no such captioner: {name}")]
    NoSuchCaptioner { name: String },

    #[error("local captioner requires the `local-vision` cargo feature")]
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

    #[error("image decode failed: {0}")]
    ImageDecode(#[from] image::ImageError),

    #[error("captioner semaphore closed")]
    SemaphoreClosed,

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
}

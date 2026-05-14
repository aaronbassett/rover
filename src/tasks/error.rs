//! Errors raised by the task subsystem.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TasksError {
    #[error("storage: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("worker {kind} failed: {message}")]
    WorkerFailed { kind: &'static str, message: String },

    #[error("task {0} not found")]
    NotFound(String),

    #[error("task {id} is not of kind {expected}")]
    KindMismatch { id: String, expected: &'static str },

    #[error("invalid task params: {0}")]
    InvalidParams(#[from] serde_json::Error),

    #[error("internal: worker panicked")]
    WorkerPanic,

    #[error("cancelled")]
    Cancelled,
}

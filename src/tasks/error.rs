//! Errors raised by the task subsystem.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TasksError {
    #[error("storage: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("task {0} not found")]
    NotFound(String),

    #[error("invalid task params: {0}")]
    InvalidParams(#[from] serde_json::Error),
}

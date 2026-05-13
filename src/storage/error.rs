//! Storage-layer error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to open database at {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: tokio_rusqlite::Error,
    },

    #[error("failed to apply migration {name}: {source}")]
    Migration {
        name: String,
        #[source]
        source: tokio_rusqlite::Error,
    },

    #[error("database error: {0}")]
    Backend(#[from] tokio_rusqlite::Error),
}

impl From<rusqlite::Error> for StorageError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Backend(tokio_rusqlite::Error::Error(err))
    }
}

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
    Db(#[from] tokio_rusqlite::Error),

    #[error("rusqlite error: {0}")]
    Sqlite(#[from] tokio_rusqlite::rusqlite::Error),
}

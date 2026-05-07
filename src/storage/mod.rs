//! SQLite-backed cache and task storage.
//!
//! The storage layer is a thin async API over a single `tokio-rusqlite`
//! connection actor. All access is async; sync rusqlite is reachable only via
//! the actor's `call` closure.
//!
//! Per design §2.1 and §4.2: a single connection writer per process; multi-
//! process safety via WAL mode + `busy_timeout`. Migrations applied on open.

pub mod error;
pub mod pages;
pub mod system;

pub use error::StorageError;

use std::path::Path;
use std::time::Duration;

use tokio_rusqlite::Connection;

/// Async wrapper around a single SQLite connection.
#[derive(Debug, Clone)]
pub struct Db {
    pub(crate) conn: Connection,
}

/// Embedded migrations, applied in array order on every `open` whose
/// `schema_version` is below the index.
///
/// To add a migration: increment its filename (e.g. `002_servers.sql`),
/// append the `(name, sql)` pair here, never edit a previously-released
/// migration in place.
const MIGRATIONS: &[(&str, &str)] = &[(
    "001_initial.sql",
    include_str!("migrations/001_initial.sql"),
)];

impl Db {
    /// Open the database at `path`, applying any pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let conn = Connection::open(path)
            .await
            .map_err(|source| StorageError::Open {
                path: path_str.clone(),
                source: tokio_rusqlite::Error::Error(source),
            })?;

        // Set WAL + busy_timeout per-connection. WAL is persistent at the file
        // level, so this only matters on first open, but it's idempotent.
        conn.call(|c| {
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.busy_timeout(Duration::from_secs(5))?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        let db = Self { conn };
        db.run_migrations().await?;
        Ok(db)
    }

    async fn run_migrations(&self) -> Result<(), StorageError> {
        self.conn
            .call(|c| {
                let current = system::read_schema_version(c)
                    .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
                    let target = (idx + 1) as u32;
                    if current >= target {
                        continue;
                    }
                    let tx = c.unchecked_transaction()?;
                    tx.execute_batch(sql)?;
                    system::write_schema_version(&tx, target)
                        .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                    tx.commit()?;
                    tracing::info!(target: "rover::storage", migration = name, "applied migration");
                }
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Current schema version (for `rover doctor` and tests).
    pub async fn schema_version(&self) -> Result<u32, StorageError> {
        let v = self
            .conn
            .call(|c| {
                system::read_schema_version(c).map_err(|_| rusqlite::Error::ExecuteReturnedResults)
            })
            .await?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_creates_db_and_applies_migrations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        assert_eq!(db.schema_version().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let _db1 = Db::open(&path).await.unwrap();
        let db2 = Db::open(&path).await.unwrap();
        assert_eq!(db2.schema_version().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn open_creates_pages_table() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        let count: i64 = db
            .conn
            .call(|c| {
                let n: i64 =
                    c.query_row("SELECT COUNT(*) FROM pages", [], |r| r.get::<_, i64>(0))?;
                Ok::<_, rusqlite::Error>(n)
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

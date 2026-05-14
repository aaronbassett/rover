//! SQLite-backed cache and task storage.
//!
//! The storage layer is a thin async API over a single `tokio-rusqlite`
//! connection actor. All access is async; sync rusqlite is reachable only via
//! the actor's `call` closure.
//!
//! Per design §2.1 and §4.2: a single connection writer per process; multi-
//! process safety via WAL mode + `busy_timeout`. Migrations applied on open.

pub mod error;
pub mod events;
pub mod pages;
pub mod robots;
pub mod servers;
pub mod system;
pub mod tasks;

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
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial.sql",
        include_str!("migrations/001_initial.sql"),
    ),
    (
        "002_servers.sql",
        include_str!("migrations/002_servers.sql"),
    ),
    (
        "003_robots_state.sql",
        include_str!("migrations/003_robots_state.sql"),
    ),
    ("004_tasks.sql", include_str!("migrations/004_tasks.sql")),
];

/// Per-migration outcome shuttled out of the actor closure so the failed
/// migration's filename survives back to the awaiter.
enum MigrationOutcome {
    Ok,
    FailedAt { name: String, err: rusqlite::Error },
}

impl Db {
    /// Open the database at `path`, applying any pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::open_with_migrations(path, MIGRATIONS).await
    }

    /// Test-aware variant of [`Self::open`] taking an explicit migration slice.
    /// Production callers use [`Self::open`]; this seam lets tests inject a
    /// deliberately-broken migration without compromising the canonical list.
    pub(crate) async fn open_with_migrations(
        path: impl AsRef<Path>,
        migrations: &'static [(&'static str, &'static str)],
    ) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let conn = Connection::open(path)
            .await
            .map_err(|source| StorageError::Open {
                path: path_str.clone(),
                source: tokio_rusqlite::Error::Error(source),
            })?;

        conn.call(|c| {
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.busy_timeout(Duration::from_secs(5))?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        let db = Self { conn };
        db.run_migrations(migrations).await?;
        Ok(db)
    }

    async fn run_migrations(
        &self,
        migrations: &'static [(&'static str, &'static str)],
    ) -> Result<(), StorageError> {
        let outcome = self
            .conn
            .call(move |c| {
                let current = system::read_schema_version(c).map_err(unwrap_storage_err)?;
                for (idx, (name, sql)) in migrations.iter().enumerate() {
                    let target = (idx + 1) as u32;
                    if current >= target {
                        continue;
                    }
                    // unchecked = no compile-time &mut Connection proof; safe at
                    // runtime because the actor closure is the only borrow.
                    let tx = c.unchecked_transaction()?;
                    if let Err(err) = tx
                        .execute_batch(sql)
                        .and_then(|()| {
                            system::write_schema_version(&tx, target).map_err(unwrap_storage_err)
                        })
                        .and_then(|()| tx.commit())
                    {
                        return Ok(MigrationOutcome::FailedAt {
                            name: (*name).to_string(),
                            err,
                        });
                    }
                    tracing::info!(target: "rover::storage", migration = name, "applied migration");
                }
                Ok::<_, rusqlite::Error>(MigrationOutcome::Ok)
            })
            .await?;

        match outcome {
            MigrationOutcome::Ok => Ok(()),
            MigrationOutcome::FailedAt { name, err } => Err(StorageError::Migration {
                name,
                source: tokio_rusqlite::Error::Error(err),
            }),
        }
    }

    /// Current schema version (for `rover doctor` and tests).
    pub async fn schema_version(&self) -> Result<u32, StorageError> {
        self.conn
            .call(|c| Ok::<_, rusqlite::Error>(system::read_schema_version(c)))
            .await?
    }
}

/// Collapse a `StorageError` raised inside an actor closure back to a
/// `rusqlite::Error` so it can flow through `Connection::call`'s typed result.
/// `system::{read,write}_schema_version` only ever produce `Backend(Error(rusqlite))`
/// internally, so unwrapping is total in practice; the fallback preserves the
/// message rather than panicking.
fn unwrap_storage_err(e: StorageError) -> rusqlite::Error {
    match e {
        StorageError::Backend(tokio_rusqlite::Error::Error(inner)) => inner,
        other => rusqlite::Error::ToSqlConversionFailure(Box::new(StringErr(other.to_string()))),
    }
}

#[derive(Debug)]
pub(crate) struct StringErr(pub(crate) String);
impl std::fmt::Display for StringErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StringErr {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_creates_db_and_applies_migrations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        assert_eq!(db.schema_version().await.unwrap(), MIGRATIONS.len() as u32);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let _db1 = Db::open(&path).await.unwrap();
        let db2 = Db::open(&path).await.unwrap();
        assert_eq!(db2.schema_version().await.unwrap(), MIGRATIONS.len() as u32);
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

    const BROKEN_MIGRATIONS: &[(&str, &str)] =
        &[("001_broken.sql", "CREATE TABLE oops(SYNTAX ERROR);")];

    #[tokio::test]
    async fn broken_migration_surfaces_named_migration_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let err = Db::open_with_migrations(&path, BROKEN_MIGRATIONS)
            .await
            .expect_err("broken migration must fail");
        match err {
            StorageError::Migration { name, .. } => {
                assert_eq!(name, "001_broken.sql");
            }
            other => panic!("expected StorageError::Migration, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn migration_003_adds_state_column_to_robots_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();

        let cols: Vec<String> = db
            .conn
            .call(|c| {
                let mut stmt = c.prepare("PRAGMA table_info(robots_cache)")?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(r) = rows.next()? {
                    out.push(r.get::<_, String>(1)?);
                }
                Ok::<_, rusqlite::Error>(out)
            })
            .await
            .unwrap();
        assert!(cols.contains(&"state".to_string()), "cols = {cols:?}");
        assert_eq!(db.schema_version().await.unwrap(), MIGRATIONS.len() as u32);
    }
}

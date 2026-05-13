//! `servers` table: tracks live `rover mcp` instances.
//!
//! Each running server inserts its own PID row on startup and refreshes
//! `last_heartbeat` on a tokio interval. Stale rows (`last_heartbeat`
//! older than the configured threshold) are reaped at startup. Clean
//! shutdown deletes the own row.

use std::time::Duration;

use rusqlite::params;

use super::{Db, StorageError};

/// Row shape returned by query helpers + used by tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub pid: i64,
    pub version: String,
    pub started_at: i64,
    pub last_heartbeat: i64,
}

impl Db {
    /// Upsert the current process's row. If a row already exists for `pid`
    /// (from a crashed prior run of this PID), its `started_at` is reset.
    pub async fn upsert_server_self(&self, pid: i64, version: String) -> Result<(), StorageError> {
        let now = now_epoch();
        let version_for_sql = version.clone();
        self.conn
            .call(move |c| {
                c.execute(
                    "INSERT INTO servers(pid, version, started_at, last_heartbeat)
                     VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(pid) DO UPDATE SET
                       version = excluded.version,
                       started_at = excluded.started_at,
                       last_heartbeat = excluded.last_heartbeat",
                    params![pid, version_for_sql, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Refresh `last_heartbeat` for `pid`. If no row exists, this is a no-op
    /// (logged at the caller).
    pub async fn heartbeat_server(&self, pid: i64) -> Result<(), StorageError> {
        let now = now_epoch();
        self.conn
            .call(move |c| {
                c.execute(
                    "UPDATE servers SET last_heartbeat = ?1 WHERE pid = ?2",
                    params![now, pid],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Delete rows whose `last_heartbeat` is older than `threshold` ago.
    /// Returns the number of rows removed.
    pub async fn reap_stale_servers(&self, threshold: Duration) -> Result<usize, StorageError> {
        let cutoff = now_epoch() - threshold.as_secs() as i64;
        let removed = self
            .conn
            .call(move |c| {
                let n = c.execute(
                    "DELETE FROM servers WHERE last_heartbeat < ?1",
                    params![cutoff],
                )?;
                Ok::<_, rusqlite::Error>(n)
            })
            .await?;
        Ok(removed)
    }

    /// Remove the row for `pid` (idempotent).
    pub async fn delete_server_self(&self, pid: i64) -> Result<(), StorageError> {
        self.conn
            .call(move |c| {
                c.execute("DELETE FROM servers WHERE pid = ?1", params![pid])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    /// Read all rows. Used by tests and by future M3 integration scenarios.
    pub async fn list_servers(&self) -> Result<Vec<ServerRow>, StorageError> {
        let rows = self
            .conn
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT pid, version, started_at, last_heartbeat FROM servers ORDER BY pid",
                )?;
                let mut out = Vec::new();
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    out.push(ServerRow {
                        pid: r.get(0)?,
                        version: r.get(1)?,
                        started_at: r.get(2)?,
                        last_heartbeat: r.get(3)?,
                    });
                }
                Ok::<_, rusqlite::Error>(out)
            })
            .await?;
        Ok(rows)
    }
}

fn now_epoch() -> i64 {
    jiff::Timestamp::now().as_second()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_db() -> Db {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        // Keep tempdir alive by leaking it; this is a unit test, the OS will
        // reclaim on exit.
        std::mem::forget(tmp);
        db
    }

    #[tokio::test]
    async fn upsert_is_idempotent_and_updates_version() {
        let db = fresh_db().await;
        db.upsert_server_self(123, "0.1.0".into()).await.unwrap();
        db.upsert_server_self(123, "0.1.1".into()).await.unwrap();
        let rows = db.list_servers().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 123);
        assert_eq!(rows[0].version, "0.1.1");
    }

    #[tokio::test]
    async fn heartbeat_updates_last_heartbeat() {
        let db = fresh_db().await;
        db.upsert_server_self(7, "v".into()).await.unwrap();
        let initial = db.list_servers().await.unwrap()[0].last_heartbeat;
        // Push the timestamp backwards to simulate elapsed time.
        db.conn
            .call(|c| {
                c.execute("UPDATE servers SET last_heartbeat = 100 WHERE pid = 7", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        db.heartbeat_server(7).await.unwrap();
        let updated = db.list_servers().await.unwrap()[0].last_heartbeat;
        assert!(updated > 100);
        assert!(updated >= initial);
    }

    #[tokio::test]
    async fn reap_stale_removes_old_rows_only() {
        let db = fresh_db().await;
        db.upsert_server_self(1, "v".into()).await.unwrap();
        db.upsert_server_self(2, "v".into()).await.unwrap();
        db.upsert_server_self(3, "v".into()).await.unwrap();
        // Mark PID 1 and 2 as ancient.
        db.conn
            .call(|c| {
                c.execute(
                    "UPDATE servers SET last_heartbeat = 0 WHERE pid IN (1,2)",
                    [],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        let removed = db
            .reap_stale_servers(Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(removed, 2);
        let rows = db.list_servers().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 3);
    }

    #[tokio::test]
    async fn delete_self_is_idempotent() {
        let db = fresh_db().await;
        db.upsert_server_self(42, "v".into()).await.unwrap();
        db.delete_server_self(42).await.unwrap();
        db.delete_server_self(42).await.unwrap();
        assert!(db.list_servers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn heartbeat_on_missing_pid_is_noop() {
        let db = fresh_db().await;
        db.heartbeat_server(999).await.unwrap();
        assert!(db.list_servers().await.unwrap().is_empty());
    }
}

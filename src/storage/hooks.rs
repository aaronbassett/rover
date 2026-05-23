//! SQLite `update_hook` plumbing for fast in-process task-insert wakeups.
//!
//! The scheduler's polling tick (default 10s) is the cross-process safety
//! net. Inside a single process, we want sub-second pickup whenever any
//! code path inserts into `tasks` without going through the scheduler's
//! mpsc channel (e.g. the MCP `fetch` revalidate path, or the CLI batch
//! enqueue). SQLite's `update_hook` gives us exactly that — **but only**
//! for writes via the same `Connection` the hook is installed on.
//!
//! Because the storage layer funnels every write through the single
//! `tokio-rusqlite` actor that `Db` owns, installing the hook on that
//! actor's connection captures every write performed through this `Db`
//! handle (and any clone of it — `Db: Clone` shares the same actor).
//! Code that opens a *separate* `Db` to the same file uses a *different*
//! actor connection; those writes are not observed and fall back to the
//! polling tick. Cross-OS-process writes always rely on the polling tick.
//!
//! The hook captures a `Weak<Notify>`; once the scheduler drops its
//! `Arc<Notify>`, subsequent fires become cheap no-ops. The returned
//! [`UpdateHookGuard`] schedules a best-effort detach on drop.
//!
//! Lives inside `src/storage/` per the workspace rule that raw
//! `rusqlite::Connection` access stays storage-internal.

use std::sync::{Arc, Weak};

use rusqlite::hooks::Action;
use tokio::sync::Notify;
use tokio_rusqlite::Connection;

use super::{Db, StorageError};

/// RAII guard for the installed `update_hook`. Dropping the guard sends a
/// fire-and-forget detach (`update_hook(None)`) to the storage actor when
/// a Tokio runtime is available; otherwise the hook is left in place and
/// the captured `Weak<Notify>` simply lapses.
pub struct UpdateHookGuard {
    conn: Connection,
}

impl std::fmt::Debug for UpdateHookGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateHookGuard").finish_non_exhaustive()
    }
}

impl Drop for UpdateHookGuard {
    fn drop(&mut self) {
        let conn = self.conn.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                let _ = conn
                    .call(|c| {
                        c.update_hook(None::<fn(Action, &str, &str, i64)>);
                        Ok::<_, rusqlite::Error>(())
                    })
                    .await;
            });
        }
    }
}

/// Register a SQLite `update_hook` on `db`'s actor connection that fires
/// `notify` on every insert/update of the `tasks` table.
///
/// The hook holds a `Weak` reference to `notify`; when the caller drops
/// its `Arc<Notify>`, subsequent fires become no-ops. Drop the returned
/// guard to eagerly detach.
pub async fn register_tasks_update_hook(
    db: &Db,
    notify: Arc<Notify>,
) -> Result<UpdateHookGuard, StorageError> {
    let weak: Weak<Notify> = Arc::downgrade(&notify);
    let conn = db.conn.clone();
    let conn_for_guard = conn.clone();
    conn.call(move |c| {
        c.update_hook(Some(
            move |action: Action, _db: &str, table: &str, _rowid: i64| {
                if table == "tasks"
                    && matches!(action, Action::SQLITE_INSERT | Action::SQLITE_UPDATE)
                    && let Some(n) = weak.upgrade()
                {
                    n.notify_one();
                }
            },
        ));
        Ok::<_, rusqlite::Error>(())
    })
    .await?;
    Ok(UpdateHookGuard {
        conn: conn_for_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use std::time::Duration;

    #[tokio::test]
    async fn insert_via_same_db_fires_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();
        let notify = Arc::new(Notify::new());
        let _guard = register_tasks_update_hook(&db, notify.clone())
            .await
            .unwrap();

        db.conn
            .call(|c| {
                c.execute(
                    "INSERT INTO tasks (id, kind, status, created_at, updated_at, params_json) \
                     VALUES (?1, 'batch_fetch', 'pending', 0, 0, '{}')",
                    rusqlite::params!["hook-test-id"],
                )?;
                Ok::<(), rusqlite::Error>(())
            })
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_millis(200), notify.notified())
            .await
            .expect("notify did not fire within 200ms");
    }
}

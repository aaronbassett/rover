//! `tasks` table async API.
//!
//! Sibling of `storage::pages`/`storage::robots`: a thin async wrapper that
//! hops into the `tokio-rusqlite` actor for SQLite work. Timestamps are
//! epoch milliseconds (sub-second ordering matters for the event stream).
//!
//! Helpers covering `task_events` live in `storage::events`.

use rusqlite::{OptionalExtension, params};

use super::{Db, StorageError, StringErr};

/// Build a `StorageError` for an unknown enum text decoded from SQLite.
///
/// `tokio_rusqlite` 0.7's `Error` enum has no `Other` variant (only
/// `ConnectionClosed`, `Close`, `Error(rusqlite::Error)`), so we wrap a
/// synthetic `rusqlite::Error::FromSqlConversionFailure` — semantically
/// correct here since we're failing to map a SQL text value back to a Rust
/// enum. Matches the pattern used by `storage::robots::RobotsState`.
fn unknown_enum_err(column_index: usize, message: String) -> StorageError {
    StorageError::Backend(tokio_rusqlite::Error::Error(
        rusqlite::Error::FromSqlConversionFailure(
            column_index,
            rusqlite::types::Type::Text,
            Box::new(StringErr(message)),
        ),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    BatchFetch,
    Retry,
    Revalidate,
    Summarize,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BatchFetch => "batch_fetch",
            Self::Retry => "retry",
            Self::Revalidate => "revalidate",
            Self::Summarize => "summarize",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "batch_fetch" => Self::BatchFetch,
            "retry" => Self::Retry,
            "revalidate" => Self::Revalidate,
            "summarize" => Self::Summarize,
            other => {
                // Column index 1 matches the `kind` position in the SELECT
                // projections used by `get` / `list_orphans`.
                return Err(unknown_enum_err(1, format!("unknown tasks.kind = {other}")));
            }
        })
    }

    /// Whether the worker can resume from persisted progress after an
    /// owner-PID handoff. `summarize` is not resumable per design §2.3.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::BatchFetch | Self::Retry | Self::Revalidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => {
                // Column index 2 matches the `status` position in the SELECT
                // projections used by `get` / `list_orphans`.
                return Err(unknown_enum_err(
                    2,
                    format!("unknown tasks.status = {other}"),
                ));
            }
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Row shape returned by query helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub params_json: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub cancellation_requested: bool,
    pub owner_pid: Option<i64>,
}

/// Input for inserting a new task (status & timestamps set internally).
#[derive(Debug, Clone)]
pub struct TaskInsert {
    pub id: String,
    pub kind: TaskKind,
    pub params_json: String,
    pub owner_pid: Option<i64>,
}

pub async fn insert(db: &Db, input: TaskInsert) -> Result<(), StorageError> {
    let TaskInsert {
        id,
        kind,
        params_json,
        owner_pid,
    } = input;
    let kind_s = kind.as_str().to_string();
    let now = now_epoch_ms();
    let id_for_notify = id.clone();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO tasks
                   (id, kind, status, created_at, updated_at, params_json,
                    result_json, error, cancellation_requested, owner_pid)
                 VALUES (?1, ?2, 'running', ?3, ?3, ?4, NULL, NULL, 0, ?5)",
                params![id, kind_s, now, params_json, owner_pid],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    // Notify the scheduler if a listener has been installed. This is the
    // single source of truth for new-task dispatch — every insert site
    // (MCP tool, fetcher SWR, deferred retry, retry chain) benefits.
    // A poisoned mutex or a closed channel is non-fatal: the orphan scan
    // will pick the row up eventually if the inserting process dies.
    if let Ok(guard) = db.new_task_tx.lock() {
        if let Some(tx) = guard.as_ref() {
            if let Err(e) = tx.send(id_for_notify) {
                tracing::debug!(
                    target: "rover::storage",
                    error = ?e,
                    "new-task notify channel closed",
                );
            }
        }
    }
    Ok(())
}

pub async fn get(db: &Db, id: &str) -> Result<Option<TaskRow>, StorageError> {
    let id = id.to_string();
    let row = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT id, kind, status, created_at, updated_at, params_json,
                        result_json, error, cancellation_requested, owner_pid
                 FROM tasks WHERE id = ?1",
                [&id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                    ))
                },
            )
            .optional()
        })
        .await?;
    let Some((
        id,
        kind_s,
        status_s,
        created_at,
        updated_at,
        params_json,
        result_json,
        error,
        canc,
        owner_pid,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(TaskRow {
        id,
        kind: TaskKind::from_db(&kind_s)?,
        status: TaskStatus::from_db(&status_s)?,
        created_at,
        updated_at,
        params_json,
        result_json,
        error,
        cancellation_requested: canc != 0,
        owner_pid,
    }))
}

pub async fn set_status(
    db: &Db,
    id: &str,
    status: TaskStatus,
    result_json: Option<String>,
    error: Option<String>,
) -> Result<(), StorageError> {
    let id = id.to_string();
    let status_s = status.as_str().to_string();
    let now = now_epoch_ms();
    db.conn
        .call(move |c| {
            c.execute(
                "UPDATE tasks
                    SET status = ?1, updated_at = ?2,
                        result_json = COALESCE(?3, result_json),
                        error = COALESCE(?4, error)
                  WHERE id = ?5",
                params![status_s, now, result_json, error, id],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    Ok(())
}

pub async fn set_cancellation_requested(db: &Db, id: &str) -> Result<bool, StorageError> {
    let id = id.to_string();
    let now = now_epoch_ms();
    let changed = db
        .conn
        .call(move |c| {
            let n = c.execute(
                "UPDATE tasks
                    SET cancellation_requested = 1, updated_at = ?1
                  WHERE id = ?2 AND cancellation_requested = 0",
                params![now, id],
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(changed == 1)
}

pub async fn is_cancelled(db: &Db, id: &str) -> Result<bool, StorageError> {
    let id = id.to_string();
    let flag = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT cancellation_requested FROM tasks WHERE id = ?1",
                [&id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
        .await?;
    Ok(flag.unwrap_or(0) != 0)
}

pub async fn list_orphans(db: &Db) -> Result<Vec<TaskRow>, StorageError> {
    let rows = db
        .conn
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, kind, status, created_at, updated_at, params_json,
                        result_json, error, cancellation_requested, owner_pid
                 FROM tasks
                 WHERE status = 'running'
                   AND owner_pid IS NOT NULL
                   AND owner_pid NOT IN (SELECT pid FROM servers)",
            )?;
            let iter = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                ))
            })?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    let mut tasks = Vec::with_capacity(rows.len());
    for (
        id,
        kind_s,
        status_s,
        created_at,
        updated_at,
        params_json,
        result_json,
        error,
        canc,
        owner_pid,
    ) in rows
    {
        tasks.push(TaskRow {
            id,
            kind: TaskKind::from_db(&kind_s)?,
            status: TaskStatus::from_db(&status_s)?,
            created_at,
            updated_at,
            params_json,
            result_json,
            error,
            cancellation_requested: canc != 0,
            owner_pid,
        });
    }
    Ok(tasks)
}

pub async fn claim_orphan(
    db: &Db,
    id: &str,
    orphan_pid: i64,
    own_pid: i64,
) -> Result<bool, StorageError> {
    let id = id.to_string();
    let now = now_epoch_ms();
    let changed = db
        .conn
        .call(move |c| {
            let n = c.execute(
                "UPDATE tasks
                    SET owner_pid = ?1, updated_at = ?2
                  WHERE id = ?3 AND owner_pid = ?4 AND status = 'running'",
                params![own_pid, now, id, orphan_pid],
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(changed == 1)
}

fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        db
    }

    fn sample_insert(id: &str, pid: Option<i64>) -> TaskInsert {
        TaskInsert {
            id: id.into(),
            kind: TaskKind::BatchFetch,
            params_json: r#"{"urls":["https://a.example/"]}"#.into(),
            owner_pid: pid,
        }
    }

    #[tokio::test]
    async fn insert_and_get_round_trip() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        let got = get(&db, "t1").await.unwrap().expect("row missing");
        assert_eq!(got.id, "t1");
        assert_eq!(got.kind, TaskKind::BatchFetch);
        assert_eq!(got.status, TaskStatus::Running);
        assert_eq!(got.owner_pid, Some(7));
        assert!(!got.cancellation_requested);
    }

    #[tokio::test]
    async fn get_unknown_returns_none() {
        let db = fresh_db().await;
        assert!(get(&db, "nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_status_terminal_writes_result_and_error() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        set_status(
            &db,
            "t1",
            TaskStatus::Failed,
            None,
            Some("owner_died".into()),
        )
        .await
        .unwrap();
        let got = get(&db, "t1").await.unwrap().unwrap();
        assert_eq!(got.status, TaskStatus::Failed);
        assert_eq!(got.error.as_deref(), Some("owner_died"));
    }

    #[tokio::test]
    async fn set_cancellation_requested_is_idempotent() {
        let db = fresh_db().await;
        insert(&db, sample_insert("t1", Some(7))).await.unwrap();
        let first = set_cancellation_requested(&db, "t1").await.unwrap();
        let second = set_cancellation_requested(&db, "t1").await.unwrap();
        assert!(first);
        assert!(!second, "second call should be a no-op");
        assert!(is_cancelled(&db, "t1").await.unwrap());
    }

    #[tokio::test]
    async fn set_cancellation_requested_on_missing_id_returns_false() {
        let db = fresh_db().await;
        assert!(!set_cancellation_requested(&db, "ghost").await.unwrap());
    }

    #[tokio::test]
    async fn list_orphans_excludes_live_pids() {
        let db = fresh_db().await;
        db.upsert_server_self(100, "v".into()).await.unwrap();
        insert(&db, sample_insert("live", Some(100))).await.unwrap();
        insert(&db, sample_insert("dead", Some(999))).await.unwrap();
        let orphans = list_orphans(&db).await.unwrap();
        let ids: Vec<&str> = orphans.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["dead"]);
    }

    #[tokio::test]
    async fn list_orphans_excludes_terminal_tasks() {
        let db = fresh_db().await;
        insert(&db, sample_insert("dead_done", Some(999)))
            .await
            .unwrap();
        set_status(&db, "dead_done", TaskStatus::Completed, None, None)
            .await
            .unwrap();
        let orphans = list_orphans(&db).await.unwrap();
        assert!(
            orphans.is_empty(),
            "completed orphan should not appear: {orphans:?}",
        );
    }

    #[tokio::test]
    async fn claim_orphan_cas_wins_then_loses() {
        let db = fresh_db().await;
        insert(&db, sample_insert("orphan", Some(999)))
            .await
            .unwrap();
        let first = claim_orphan(&db, "orphan", 999, 1).await.unwrap();
        let second = claim_orphan(&db, "orphan", 999, 2).await.unwrap();
        assert!(first, "first claimer should win");
        assert!(!second, "second claimer should lose");
        let got = get(&db, "orphan").await.unwrap().unwrap();
        assert_eq!(got.owner_pid, Some(1));
    }
}

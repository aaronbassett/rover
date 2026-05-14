//! `task_events` append-only log.

use rusqlite::{OptionalExtension, params};

use super::{Db, StorageError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub task_id: String,
    pub ts: i64,
    pub kind: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub struct EventInsert {
    pub task_id: String,
    pub kind: String,
    pub payload_json: String,
}

pub async fn append(db: &Db, input: EventInsert) -> Result<i64, StorageError> {
    let EventInsert {
        task_id,
        kind,
        payload_json,
    } = input;
    let now = now_epoch_ms();
    let id = db
        .conn
        .call(move |c| {
            c.execute(
                "INSERT INTO task_events (task_id, ts, kind, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![task_id, now, kind, payload_json],
            )?;
            Ok::<_, rusqlite::Error>(c.last_insert_rowid())
        })
        .await?;
    Ok(id)
}

pub async fn range_since(
    db: &Db,
    task_id: &str,
    after_id: i64,
    limit: i64,
) -> Result<Vec<EventRow>, StorageError> {
    let task_id = task_id.to_string();
    let rows = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, task_id, ts, kind, payload_json
                 FROM task_events
                 WHERE task_id = ?1 AND id > ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )?;
            let iter = stmt.query_map(params![task_id, after_id, limit], |r| {
                Ok(EventRow {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    ts: r.get(2)?,
                    kind: r.get(3)?,
                    payload_json: r.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    Ok(rows)
}

pub async fn count_by_kind(db: &Db, task_id: &str) -> Result<Vec<(String, i64)>, StorageError> {
    let task_id = task_id.to_string();
    let counts = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT kind, COUNT(*) FROM task_events WHERE task_id = ?1 GROUP BY kind",
            )?;
            let iter = stmt.query_map([&task_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            let mut out = Vec::new();
            for r in iter {
                out.push(r?);
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .await?;
    Ok(counts)
}

pub async fn last_for_task(db: &Db, task_id: &str) -> Result<Option<EventRow>, StorageError> {
    let task_id = task_id.to_string();
    let row = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT id, task_id, ts, kind, payload_json
                 FROM task_events WHERE task_id = ?1
                 ORDER BY id DESC LIMIT 1",
                [&task_id],
                |r| {
                    Ok(EventRow {
                        id: r.get(0)?,
                        task_id: r.get(1)?,
                        ts: r.get(2)?,
                        kind: r.get(3)?,
                        payload_json: r.get(4)?,
                    })
                },
            )
            .optional()
        })
        .await?;
    Ok(row)
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
    use crate::storage::tasks::{TaskInsert, TaskKind, insert as insert_task};
    use tempfile::tempdir;

    async fn fresh_db_with_task(id: &str) -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        insert_task(
            &db,
            TaskInsert {
                id: id.into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        db
    }

    fn ev(task_id: &str, kind: &str, payload: &str) -> EventInsert {
        EventInsert {
            task_id: task_id.into(),
            kind: kind.into(),
            payload_json: payload.into(),
        }
    }

    #[tokio::test]
    async fn append_returns_monotonic_ids() {
        let db = fresh_db_with_task("t1").await;
        let id1 = append(&db, ev("t1", "task_started", "{}")).await.unwrap();
        let id2 = append(&db, ev("t1", "item_done", r#"{"url":"a"}"#))
            .await
            .unwrap();
        assert!(id2 > id1);
    }

    #[tokio::test]
    async fn range_since_filters_by_cursor() {
        let db = fresh_db_with_task("t1").await;
        let id1 = append(&db, ev("t1", "a", "{}")).await.unwrap();
        let id2 = append(&db, ev("t1", "b", "{}")).await.unwrap();
        let id3 = append(&db, ev("t1", "c", "{}")).await.unwrap();
        let rows = range_since(&db, "t1", id1, 100).await.unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["b", "c"]);
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![id2, id3]);
    }

    #[tokio::test]
    async fn range_since_caps_at_limit() {
        let db = fresh_db_with_task("t1").await;
        for i in 0..10 {
            append(&db, ev("t1", "x", &format!(r#"{{"i":{i}}}"#)))
                .await
                .unwrap();
        }
        let rows = range_since(&db, "t1", 0, 3).await.unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn range_since_isolates_tasks() {
        let db = fresh_db_with_task("t1").await;
        insert_task(
            &db,
            TaskInsert {
                id: "t2".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        append(&db, ev("t2", "b", "{}")).await.unwrap();
        let rows = range_since(&db, "t1", 0, 100).await.unwrap();
        assert!(rows.iter().all(|r| r.task_id == "t1"));
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn count_by_kind_groups_correctly() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "item_done", "{}")).await.unwrap();
        append(&db, ev("t1", "item_done", "{}")).await.unwrap();
        append(&db, ev("t1", "item_failed", "{}")).await.unwrap();
        let mut counts = count_by_kind(&db, "t1").await.unwrap();
        counts.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            counts,
            vec![("item_done".into(), 2), ("item_failed".into(), 1)],
        );
    }

    #[tokio::test]
    async fn last_for_task_returns_highest_id() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        let mid_id = append(&db, ev("t1", "b", r#"{"k":1}"#)).await.unwrap();
        append(&db, ev("t1", "c", "{}")).await.unwrap();
        let last = last_for_task(&db, "t1").await.unwrap().unwrap();
        assert_eq!(last.kind, "c");
        assert!(last.id > mid_id);
    }

    #[tokio::test]
    async fn cascade_delete_drops_events() {
        let db = fresh_db_with_task("t1").await;
        append(&db, ev("t1", "a", "{}")).await.unwrap();
        db.conn
            .call(|c| {
                c.execute("DELETE FROM tasks WHERE id = 't1'", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        let rows = range_since(&db, "t1", 0, 100).await.unwrap();
        assert!(rows.is_empty());
    }
}

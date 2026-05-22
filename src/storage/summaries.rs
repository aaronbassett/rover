//! Async wrapper around the `summary_cache` table.
//!
//! `content_hash` is the page's extracted-markdown sha256 (with the
//! `sha256:` prefix matching `pages.content_hash`) for whole-page
//! summaries, or the raw sha256-of-table-text (no prefix) for per-table
//! summaries. The column accepts either shape — callers decide.

use crate::storage::Db;
use crate::storage::error::StorageError;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// One `summary_cache` row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryRow {
    pub content_hash: String,
    pub params_hash: String,
    pub summary_md: String,
    pub created_at: i64,
}

/// Look up a cached summary by `(content_hash, params_hash)`. Returns
/// `Ok(None)` on no-such-row; `Err(...)` only on storage errors.
pub async fn lookup(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
) -> Result<Option<SummaryRow>, StorageError> {
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    db.conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT content_hash, params_hash, summary_md, created_at \
                   FROM summary_cache \
                  WHERE content_hash = ?1 AND params_hash = ?2",
            )?;
            let mut rows = stmt.query(rusqlite::params![ch, ph])?;
            if let Some(r) = rows.next()? {
                Ok::<_, rusqlite::Error>(Some(SummaryRow {
                    content_hash: r.get(0)?,
                    params_hash: r.get(1)?,
                    summary_md: r.get(2)?,
                    created_at: r.get(3)?,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(Into::into)
}

/// Insert a new summary. On unique-conflict, the existing row wins and
/// the function returns `Ok(())` — concurrent writers can both attempt the
/// write and the cache stays consistent.
pub async fn insert(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
    summary_md: &str,
) -> Result<(), StorageError> {
    let now = Timestamp::now().as_second();
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    let md = summary_md.to_string();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO summary_cache (content_hash, params_hash, summary_md, created_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(content_hash, params_hash) DO NOTHING",
                rusqlite::params![ch, ph, md, now],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_db() -> (Db, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        (Db::open(&path).await.unwrap(), tmp)
    }

    #[tokio::test]
    async fn lookup_returns_none_for_missing_row() {
        let (db, _tmp) = make_db().await;
        let r = lookup(&db, "sha256:abc", "ph").await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn insert_then_lookup_round_trips() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "hello").await.unwrap();
        let r = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        assert_eq!(r.content_hash, "sha256:abc");
        assert_eq!(r.params_hash, "ph1");
        assert_eq!(r.summary_md, "hello");
        assert!(r.created_at > 0);
    }

    #[tokio::test]
    async fn insert_conflict_keeps_first_row() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "first").await.unwrap();
        insert(&db, "sha256:abc", "ph1", "second").await.unwrap();
        let r = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        assert_eq!(r.summary_md, "first");
    }

    #[tokio::test]
    async fn different_params_hash_creates_independent_rows() {
        let (db, _tmp) = make_db().await;
        insert(&db, "sha256:abc", "ph1", "one").await.unwrap();
        insert(&db, "sha256:abc", "ph2", "two").await.unwrap();
        let r1 = lookup(&db, "sha256:abc", "ph1").await.unwrap().unwrap();
        let r2 = lookup(&db, "sha256:abc", "ph2").await.unwrap().unwrap();
        assert_eq!(r1.summary_md, "one");
        assert_eq!(r2.summary_md, "two");
    }
}

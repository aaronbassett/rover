//! Pages table CRUD.
//!
//! All operations are async wrappers that hop into the `tokio-rusqlite`
//! actor (`Db.conn.call(...)`) for the actual SQLite work. Errors flow back
//! as [`StorageError`]; the `From<rusqlite::Error>` impl on `StorageError`
//! lets the closure use `?` directly on `rusqlite::Error`.

use std::fmt::Write as _;

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::{Db, StorageError};

/// A row in the `pages` table.
///
/// Mirrors the schema from `001_initial.sql` minus `raw_html_zstd`, which is
/// reserved for a later milestone and always written as NULL today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub url_hash: String,
    pub url: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub fetched_at: i64,
    pub expires_at: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_hash: String,
    pub extracted_md: String,
    pub metadata_json: Option<String>,
}

/// Aggregate stats for `rover cache stats`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStats {
    pub entry_count: u64,
    pub total_extracted_bytes: u64,
    pub expired_count: u64,
}

/// One row of `rover cache list` output.
#[derive(Debug, Clone)]
pub struct CacheListEntry {
    pub url: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub fetched_at: i64,
    pub expires_at: Option<i64>,
    pub size_bytes: i64,
}

/// Compute the cache key (sha256 hex) for a URL.
///
/// Callers should hash the canonical URL; the field is named `url_hash`
/// purely for column-name parity.
pub fn url_hash(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        // Writing to a `String` is infallible; the formatter cannot fail.
        write!(s, "{b:02x}").expect("write to String never fails");
    }
    s
}

/// Column list shared by `get_by_url_hash` / `get_by_url` so the row
/// decoder stays in lock-step with the SELECT projection.
const SELECT_COLUMNS: &str = "url_hash, url, canonical_url, title, fetched_at, expires_at, \
    etag, last_modified, content_hash, extracted_md, metadata_json";

fn row_to_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    Ok(Page {
        url_hash: row.get(0)?,
        url: row.get(1)?,
        canonical_url: row.get(2)?,
        title: row.get(3)?,
        fetched_at: row.get(4)?,
        expires_at: row.get(5)?,
        etag: row.get(6)?,
        last_modified: row.get(7)?,
        content_hash: row.get(8)?,
        extracted_md: row.get(9)?,
        metadata_json: row.get(10)?,
    })
}

/// Look up a page by its `url_hash` (PK).
pub async fn get_by_url_hash(db: &Db, hash: &str) -> Result<Option<Page>, StorageError> {
    let hash = hash.to_owned();
    let page = db
        .conn
        .call(move |c| {
            c.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM pages WHERE url_hash = ?1"),
                params![hash],
                row_to_page,
            )
            .optional()
        })
        .await?;
    Ok(page)
}

/// Look up a page by its `url` column (most-recently-requested URL).
///
/// This is a secondary lookup; multiple rows could in principle share the
/// same `url` if a redirect collapses two distinct canonical URLs onto it,
/// so this returns the first match.
pub async fn get_by_url(db: &Db, url: &str) -> Result<Option<Page>, StorageError> {
    let url = url.to_owned();
    let page = db
        .conn
        .call(move |c| {
            c.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM pages WHERE url = ?1 LIMIT 1"),
                params![url],
                row_to_page,
            )
            .optional()
        })
        .await?;
    Ok(page)
}

/// Insert or replace a page row.
pub async fn upsert(db: &Db, page: Page) -> Result<(), StorageError> {
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO pages (url_hash, url, canonical_url, title, fetched_at, \
                                    expires_at, etag, last_modified, content_hash, \
                                    extracted_md, metadata_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                 ON CONFLICT(url_hash) DO UPDATE SET \
                    url = excluded.url, \
                    canonical_url = excluded.canonical_url, \
                    title = excluded.title, \
                    fetched_at = excluded.fetched_at, \
                    expires_at = excluded.expires_at, \
                    etag = excluded.etag, \
                    last_modified = excluded.last_modified, \
                    content_hash = excluded.content_hash, \
                    extracted_md = excluded.extracted_md, \
                    metadata_json = excluded.metadata_json",
                params![
                    page.url_hash,
                    page.url,
                    page.canonical_url,
                    page.title,
                    page.fetched_at,
                    page.expires_at,
                    page.etag,
                    page.last_modified,
                    page.content_hash,
                    page.extracted_md,
                    page.metadata_json,
                ],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Bump `fetched_at` (and optionally `expires_at`) on an existing row.
///
/// Used when revalidation produces 304 Not Modified: the body is unchanged
/// but the freshness window has been extended.
pub async fn touch(
    db: &Db,
    url_hash: &str,
    fetched_at: i64,
    expires_at: Option<i64>,
) -> Result<(), StorageError> {
    let url_hash = url_hash.to_owned();
    db.conn
        .call(move |c| {
            c.execute(
                "UPDATE pages SET fetched_at = ?2, expires_at = ?3 WHERE url_hash = ?1",
                params![url_hash, fetched_at, expires_at],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Delete pages whose `url` matches the given SQL LIKE pattern.
///
/// `\` is the escape character. Returns the number of rows removed.
pub async fn delete_by_url_like(db: &Db, like: &str) -> Result<u64, StorageError> {
    let like = like.to_owned();
    let n = db
        .conn
        .call(move |c| {
            Ok(c.execute(
                "DELETE FROM pages WHERE url LIKE ?1 ESCAPE '\\'",
                params![like],
            )? as u64)
        })
        .await?;
    Ok(n)
}

/// Paginated listing of cached URLs ordered by `fetched_at DESC`.
pub async fn list_paginated(
    db: &Db,
    offset: u64,
    limit: u64,
) -> Result<Vec<CacheListEntry>, StorageError> {
    let entries = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT url, canonical_url, title, fetched_at, expires_at, length(extracted_md) \
                 FROM pages \
                 ORDER BY fetched_at DESC \
                 LIMIT ?1 OFFSET ?2",
            )?;
            let rows = stmt
                .query_map(params![limit as i64, offset as i64], |r| {
                    Ok(CacheListEntry {
                        url: r.get(0)?,
                        canonical_url: r.get(1)?,
                        title: r.get(2)?,
                        fetched_at: r.get(3)?,
                        expires_at: r.get(4)?,
                        size_bytes: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(entries)
}

/// Aggregate cache stats for `rover cache stats`.
///
/// `now` is unix epoch seconds; rows with `expires_at <= now` count as
/// expired. Rows with `expires_at IS NULL` (cache forever) never expire.
pub async fn stats(db: &Db, now: i64) -> Result<CacheStats, StorageError> {
    let stats = db
        .conn
        .call(move |c| {
            let entry_count: i64 = c.query_row("SELECT COUNT(*) FROM pages", [], |r| r.get(0))?;
            let total_bytes: i64 = c.query_row(
                "SELECT COALESCE(SUM(length(extracted_md)), 0) FROM pages",
                [],
                |r| r.get(0),
            )?;
            let expired_count: i64 = c.query_row(
                "SELECT COUNT(*) FROM pages WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![now],
                |r| r.get(0),
            )?;
            Ok(CacheStats {
                entry_count: entry_count as u64,
                total_extracted_bytes: total_bytes as u64,
                expired_count: expired_count as u64,
            })
        })
        .await?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(hash: &str, url: &str) -> Page {
        Page {
            url_hash: hash.to_owned(),
            url: url.to_owned(),
            canonical_url: url.to_owned(),
            title: Some("Sample".to_owned()),
            fetched_at: 1_700_000_000,
            expires_at: Some(1_700_003_600),
            etag: Some("\"abc\"".to_owned()),
            last_modified: None,
            content_hash: "sha256:deadbeef".to_owned(),
            extracted_md: "# Hello\n\nbody".to_owned(),
            metadata_json: None,
        }
    }

    async fn fresh_db() -> Db {
        let tmp = tempfile::tempdir().unwrap();
        Db::open(tmp.path().join("rover.db")).await.unwrap()
    }

    #[test]
    fn url_hash_is_hex_64() {
        let h = url_hash("https://example.com/");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn upsert_then_get() {
        let db = fresh_db().await;
        let page = sample("hash1", "https://example.com/page");
        upsert(&db, page.clone()).await.unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got, page);
    }

    #[tokio::test]
    async fn upsert_replaces_existing() {
        let db = fresh_db().await;
        let p1 = sample("hash1", "https://example.com/v1");
        let mut p2 = p1.clone();
        p2.url = "https://example.com/v2".to_owned();
        p2.fetched_at = 1_700_000_999;
        upsert(&db, p1).await.unwrap();
        upsert(&db, p2.clone()).await.unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got, p2);
    }

    #[tokio::test]
    async fn get_by_url_finds_secondary_lookup() {
        let db = fresh_db().await;
        upsert(&db, sample("hash1", "https://example.com/article"))
            .await
            .unwrap();
        let got = get_by_url(&db, "https://example.com/article")
            .await
            .unwrap();
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn touch_updates_timestamps() {
        let db = fresh_db().await;
        upsert(&db, sample("hash1", "https://example.com/"))
            .await
            .unwrap();
        touch(&db, "hash1", 1_700_999_999, Some(1_700_999_999 + 3600))
            .await
            .unwrap();
        let got = get_by_url_hash(&db, "hash1").await.unwrap().unwrap();
        assert_eq!(got.fetched_at, 1_700_999_999);
        assert_eq!(got.expires_at, Some(1_700_999_999 + 3600));
    }

    #[tokio::test]
    async fn delete_by_url_like() {
        let db = fresh_db().await;
        upsert(&db, sample("h1", "https://docs.example.com/a"))
            .await
            .unwrap();
        upsert(&db, sample("h2", "https://docs.example.com/b"))
            .await
            .unwrap();
        upsert(&db, sample("h3", "https://other.com/c"))
            .await
            .unwrap();
        let n = super::delete_by_url_like(&db, "https://docs.example.com/%")
            .await
            .unwrap();
        assert_eq!(n, 2);
        assert!(get_by_url_hash(&db, "h1").await.unwrap().is_none());
        assert!(get_by_url_hash(&db, "h3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn list_paginated_orders_by_recency() {
        let db = fresh_db().await;
        let mut a = sample("h_a", "https://a/");
        a.fetched_at = 100;
        let mut b = sample("h_b", "https://b/");
        b.fetched_at = 200;
        upsert(&db, a).await.unwrap();
        upsert(&db, b).await.unwrap();
        let rows = list_paginated(&db, 0, 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://b/");
        assert_eq!(rows[1].url, "https://a/");
    }

    #[tokio::test]
    async fn stats_counts_expired() {
        let db = fresh_db().await;
        let mut fresh = sample("h_fresh", "https://a/");
        fresh.expires_at = Some(2_000_000_000);
        let mut stale = sample("h_stale", "https://b/");
        stale.expires_at = Some(1);
        upsert(&db, fresh).await.unwrap();
        upsert(&db, stale).await.unwrap();
        let s = stats(&db, 1_700_000_000).await.unwrap();
        assert_eq!(s.entry_count, 2);
        assert!(s.total_extracted_bytes > 0);
        assert_eq!(s.expired_count, 1);
    }
}

//! Async wrapper around the `image_caption_cache` table.
//!
//! `content_hash` is the sha256 of the image bytes (or a URL-derived hash)
//! and `params_hash` encodes the captioning-model parameters so the same
//! image can be captioned differently under different settings. The pair
//! forms the primary key; concurrent writers safely race via
//! `ON CONFLICT DO NOTHING`.

use crate::storage::Db;
use crate::storage::error::StorageError;
use jiff::Timestamp;
use rusqlite::OptionalExtension;

/// One `image_caption_cache` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionRow {
    pub caption: String,
    pub created_at: i64,
    pub raw_image_zstd: Option<Vec<u8>>,
}

/// Look up a cached caption by `(content_hash, params_hash)`. Returns
/// `Ok(None)` on no-such-row; `Err(...)` only on storage errors.
pub async fn lookup(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
) -> Result<Option<CaptionRow>, StorageError> {
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    let row = db
        .conn
        .call(move |c| {
            c.query_row(
                "SELECT caption, created_at, raw_image_zstd \
                   FROM image_caption_cache \
                  WHERE content_hash = ?1 AND params_hash = ?2",
                rusqlite::params![ch, ph],
                |r| {
                    Ok(CaptionRow {
                        caption: r.get(0)?,
                        created_at: r.get(1)?,
                        raw_image_zstd: r.get(2)?,
                    })
                },
            )
            .optional()
        })
        .await?;
    Ok(row)
}

/// Insert a new caption. On unique-conflict, the existing row wins and
/// the function returns `Ok(())` — concurrent writers can both attempt the
/// write and the cache stays consistent.
pub async fn insert(
    db: &Db,
    content_hash: &str,
    params_hash: &str,
    caption: &str,
    raw_image_zstd: Option<&[u8]>,
) -> Result<(), StorageError> {
    let now = Timestamp::now().as_second();
    let ch = content_hash.to_string();
    let ph = params_hash.to_string();
    let cap = caption.to_string();
    let blob = raw_image_zstd.map(|b| b.to_vec());
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO image_caption_cache \
                     (content_hash, params_hash, caption, created_at, raw_image_zstd) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(content_hash, params_hash) DO NOTHING",
                rusqlite::params![ch, ph, cap, now, blob],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_with_and_without_raw_image() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        assert!(lookup(&db, "ch", "ph").await.unwrap().is_none());
        insert(&db, "ch", "ph", "a cat", None).await.unwrap();
        let row = lookup(&db, "ch", "ph").await.unwrap().unwrap();
        assert_eq!(row.caption, "a cat");
        assert!(row.raw_image_zstd.is_none());
        insert(&db, "ch2", "ph", "a dog", Some(b"\x28\xb5\x2f\xfd"))
            .await
            .unwrap();
        assert!(
            lookup(&db, "ch2", "ph")
                .await
                .unwrap()
                .unwrap()
                .raw_image_zstd
                .is_some()
        );
    }
}

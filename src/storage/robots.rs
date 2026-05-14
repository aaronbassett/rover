//! Async API over the `robots_cache` table.
//!
//! Mirrors `storage::pages` in shape: opaque row struct, lookup by primary key,
//! upsert, prune. The `state` column tracks one of `parsed`, `allow_all`, or
//! `disallow_all` per the M5 design spec.

use crate::storage::{Db, StorageError};

/// One row from `robots_cache`. The `state` discriminator is a string at the
/// storage edge so SQL migrations don't have to know about Rust enums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RobotsEntry {
    pub host: String,
    pub body: Option<String>, // None for allow_all / disallow_all sentinels
    pub fetched_at: i64,
    pub expires_at: i64,
    pub state: RobotsState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotsState {
    Parsed,
    AllowAll,
    DisallowAll,
}

impl RobotsState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::AllowAll => "allow_all",
            Self::DisallowAll => "disallow_all",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, StorageError> {
        Ok(match s {
            "parsed" => Self::Parsed,
            "allow_all" => Self::AllowAll,
            "disallow_all" => Self::DisallowAll,
            other => {
                // tokio_rusqlite 0.7's `Error` enum has no `Other` variant
                // (only `ConnectionClosed`, `Close`, `Error(rusqlite::Error)`),
                // so we mirror the `StringErr` fallback used in
                // `super::unwrap_storage_err` and wrap a synthetic
                // `rusqlite::Error::ToSqlConversionFailure`.
                return Err(StorageError::Backend(tokio_rusqlite::Error::Error(
                    rusqlite::Error::ToSqlConversionFailure(Box::new(StringErr(format!(
                        "unknown robots_cache.state = {other}"
                    )))),
                )));
            }
        })
    }
}

#[derive(Debug)]
struct StringErr(String);
impl std::fmt::Display for StringErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StringErr {}

pub async fn lookup(db: &Db, host: &str) -> Result<Option<RobotsEntry>, StorageError> {
    let host = host.to_string();
    let row = db
        .conn
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT host, body, fetched_at, expires_at, state \
                 FROM robots_cache WHERE host = ?1",
            )?;
            let mut rows = stmt.query([&host])?;
            if let Some(r) = rows.next()? {
                let host: String = r.get(0)?;
                let body: Option<String> = r.get(1)?;
                let fetched_at: i64 = r.get(2)?;
                let expires_at: i64 = r.get(3)?;
                let state_s: String = r.get(4)?;
                Ok::<_, rusqlite::Error>(Some((host, body, fetched_at, expires_at, state_s)))
            } else {
                Ok(None)
            }
        })
        .await?;

    let Some((host, body, fetched_at, expires_at, state_s)) = row else {
        return Ok(None);
    };
    let state = RobotsState::from_db(&state_s)?;
    Ok(Some(RobotsEntry {
        host,
        body,
        fetched_at,
        expires_at,
        state,
    }))
}

pub async fn upsert(db: &Db, entry: RobotsEntry) -> Result<(), StorageError> {
    let RobotsEntry {
        host,
        body,
        fetched_at,
        expires_at,
        state,
    } = entry;
    let state_s = state.as_str().to_string();
    db.conn
        .call(move |c| {
            c.execute(
                "INSERT INTO robots_cache (host, body, fetched_at, expires_at, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(host) DO UPDATE SET \
                    body=excluded.body, \
                    fetched_at=excluded.fetched_at, \
                    expires_at=excluded.expires_at, \
                    state=excluded.state",
                rusqlite::params![host, body, fetched_at, expires_at, state_s],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;
    Ok(())
}

pub async fn prune_expired(db: &Db, now: i64) -> Result<usize, StorageError> {
    let removed = db
        .conn
        .call(move |c| {
            let n = c.execute("DELETE FROM robots_cache WHERE expires_at < ?1", [now])?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        Db::open(tmp.path().join("rover.db")).await.unwrap()
    }

    #[tokio::test]
    async fn upsert_and_lookup_round_trip_parsed() {
        let db = fresh_db().await;
        let entry = RobotsEntry {
            host: "example.com".into(),
            body: Some("User-agent: *\nDisallow: /admin".into()),
            fetched_at: 1_000,
            expires_at: 1_000 + 86_400,
            state: RobotsState::Parsed,
        };
        upsert(&db, entry.clone()).await.unwrap();
        let got = lookup(&db, "example.com").await.unwrap();
        assert_eq!(got.as_ref(), Some(&entry));
    }

    #[tokio::test]
    async fn lookup_unknown_host_returns_none() {
        let db = fresh_db().await;
        assert_eq!(lookup(&db, "absent.example").await.unwrap(), None);
    }

    #[tokio::test]
    async fn upsert_overwrites_existing_row() {
        let db = fresh_db().await;
        let one = RobotsEntry {
            host: "example.com".into(),
            body: Some("v1".into()),
            fetched_at: 1_000,
            expires_at: 2_000,
            state: RobotsState::Parsed,
        };
        let two = RobotsEntry {
            body: Some("v2".into()),
            ..one.clone()
        };
        upsert(&db, one).await.unwrap();
        upsert(&db, two.clone()).await.unwrap();
        assert_eq!(lookup(&db, "example.com").await.unwrap(), Some(two));
    }

    #[tokio::test]
    async fn allow_all_sentinel_has_no_body() {
        let db = fresh_db().await;
        let entry = RobotsEntry {
            host: "404.example".into(),
            body: None,
            fetched_at: 1_000,
            expires_at: 1_000 + 86_400,
            state: RobotsState::AllowAll,
        };
        upsert(&db, entry.clone()).await.unwrap();
        let got = lookup(&db, "404.example").await.unwrap();
        assert_eq!(got, Some(entry));
    }

    #[tokio::test]
    async fn prune_expired_removes_old_rows_only() {
        let db = fresh_db().await;
        upsert(
            &db,
            RobotsEntry {
                host: "old.example".into(),
                body: Some("x".into()),
                fetched_at: 100,
                expires_at: 200,
                state: RobotsState::Parsed,
            },
        )
        .await
        .unwrap();
        upsert(
            &db,
            RobotsEntry {
                host: "new.example".into(),
                body: Some("y".into()),
                fetched_at: 100,
                expires_at: 10_000,
                state: RobotsState::Parsed,
            },
        )
        .await
        .unwrap();
        let pruned = prune_expired(&db, 500).await.unwrap();
        assert_eq!(pruned, 1);
        assert!(lookup(&db, "old.example").await.unwrap().is_none());
        assert!(lookup(&db, "new.example").await.unwrap().is_some());
    }
}

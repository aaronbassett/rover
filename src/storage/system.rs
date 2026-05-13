//! System table accessors (schema_version etc).

use rusqlite::params;

use super::StorageError;

pub fn read_schema_version(conn: &rusqlite::Connection) -> Result<u32, StorageError> {
    let row: Result<String, rusqlite::Error> = conn.query_row(
        "SELECT value FROM system WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    );
    let raw = match row {
        Ok(s) => s,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(0),
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg))) if msg.contains("no such table") => {
            return Ok(0);
        }
        Err(e) => return Err(e.into()),
    };
    raw.parse::<u32>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
            .into()
    })
}

pub fn write_schema_version(conn: &rusqlite::Connection, version: u32) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO system (key, value) VALUES ('schema_version', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    #[tokio::test]
    async fn schema_version_returns_zero_when_table_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();

        db.conn
            .call(|c| {
                c.execute_batch("DROP TABLE system")?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();

        let v = db
            .conn
            .call(|c| Ok::<_, rusqlite::Error>(read_schema_version(c)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v, 0);
    }

    #[tokio::test]
    async fn schema_version_returns_zero_when_row_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        let db = Db::open(&path).await.unwrap();

        db.conn
            .call(|c| {
                c.execute("DELETE FROM system WHERE key = 'schema_version'", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();

        let v = db
            .conn
            .call(|c| Ok::<_, rusqlite::Error>(read_schema_version(c)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v, 0);
    }
}

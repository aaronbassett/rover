//! System table accessors (schema_version etc).

use rusqlite::params;

use super::StorageError;

pub fn read_schema_version(conn: &rusqlite::Connection) -> Result<u32, StorageError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT value FROM system WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(row.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn write_schema_version(conn: &rusqlite::Connection, version: u32) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO system (key, value) VALUES ('schema_version', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    Ok(())
}

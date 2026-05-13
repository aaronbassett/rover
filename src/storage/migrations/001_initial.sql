-- M2: pages, robots_cache, system tables.
--
-- WAL is set per-connection at open time (see `Db::open`); recording it here
-- as the canonical journal mode for the project.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS system (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pages (
    url_hash      TEXT PRIMARY KEY,    -- sha256 hex of canonical_url
    url           TEXT NOT NULL,       -- most-recently-requested URL
    canonical_url TEXT NOT NULL,
    title         TEXT,
    fetched_at    INTEGER NOT NULL,    -- unix epoch seconds
    expires_at    INTEGER,             -- unix epoch seconds; NULL = never
    etag          TEXT,
    last_modified TEXT,
    content_hash  TEXT NOT NULL,       -- sha256 hex of extracted_md
    extracted_md  TEXT NOT NULL,
    metadata_json TEXT,                -- JSON blob (M4)
    raw_html_zstd BLOB                 -- optional, behind config flag (M2 leaves NULL)
);

CREATE INDEX IF NOT EXISTS pages_url ON pages(url);
CREATE INDEX IF NOT EXISTS pages_expires ON pages(expires_at);
CREATE INDEX IF NOT EXISTS pages_content_hash ON pages(content_hash);

CREATE TABLE IF NOT EXISTS robots_cache (
    host       TEXT PRIMARY KEY,
    body       TEXT,
    fetched_at INTEGER,
    expires_at INTEGER
);

-- M7: summary_cache.
--
-- One row per (content, params) pairing. `content_hash` is the existing
-- `pages.content_hash` (sha256 of extracted_md) for whole-page summaries;
-- for Tables Summarize it is sha256(table_text). No FK to pages because
-- table-text summaries don't have a page row.
--
-- `params_hash` includes the backend's config-key name (design §3.5), so
-- two backends pointing at the same model produce independent cache rows.

CREATE TABLE IF NOT EXISTS summary_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash  TEXT NOT NULL,
    params_hash   TEXT NOT NULL,
    summary_md    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE(content_hash, params_hash)
);

CREATE INDEX IF NOT EXISTS summary_cache_by_content ON summary_cache(content_hash);

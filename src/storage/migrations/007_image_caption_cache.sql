CREATE TABLE IF NOT EXISTS image_caption_cache (
    content_hash   TEXT NOT NULL,
    params_hash    TEXT NOT NULL,
    caption        TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    raw_image_zstd BLOB,
    PRIMARY KEY (content_hash, params_hash)
);

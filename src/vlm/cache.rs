//! Caption-cache wrapper over `storage::summaries`.
//!
//! Cache keys:
//! - `content_hash = sha256(image_bytes)` (hex)
//! - `params_hash  = sha256(captioner_name || RS || captioner_model_id || RS || max_tokens)` (hex)
//!
//! Cache rows live in the existing `summary_cache` table (M7 migration
//! `005_summary_cache.sql`) — no new migration needed. Captions and
//! summaries share the same table because their key derivation paths
//! cannot collide (caption `content_hash` is over image bytes, summary
//! `content_hash` is over extracted markdown; both are sha256 of disjoint
//! byte spaces and the probability of overlap is the same as any sha256
//! collision).

use sha2::{Digest, Sha256};

use crate::storage::Db;
use crate::storage::summaries;
use crate::vlm::error::VlmError;

const RS: char = '\u{1E}';

pub fn content_hash(image_bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(image_bytes);
    hex_lower(&h.finalize())
}

pub fn params_hash(captioner_name: &str, captioner_model_id: &str, max_tokens: usize) -> String {
    let serialized = format!(
        "{}{}{}{}{}",
        captioner_name, RS, captioner_model_id, RS, max_tokens
    );
    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        write!(s, "{b:02x}").expect("write to String never fails");
    }
    s
}

pub async fn lookup(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
) -> Result<Option<String>, VlmError> {
    let ch = content_hash(image_bytes);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    summaries::lookup(db, &ch, &ph)
        .await
        .map(|opt| opt.map(|row| row.summary_md))
        .map_err(VlmError::Storage)
}

pub async fn insert(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
    caption: &str,
) -> Result<(), VlmError> {
    let ch = content_hash(image_bytes);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    summaries::insert(db, &ch, &ph, caption)
        .await
        .map_err(VlmError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash(b"hello");
        let h2 = content_hash(b"hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn params_hash_distinguishes_max_tokens() {
        let a = params_hash("openai", "gpt-4o-mini", 50);
        let b = params_hash("openai", "gpt-4o-mini", 100);
        assert_ne!(a, b);
    }

    #[test]
    fn params_hash_distinguishes_model() {
        let a = params_hash("openai", "gpt-4o-mini", 50);
        let b = params_hash("openai", "gpt-4o", 50);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn round_trip_persists_caption() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let image = b"\x89PNG\r\n\x1a\n fake png bytes";
        let r1 = lookup(&db, image, "openai", "gpt-4o-mini", 50)
            .await
            .unwrap();
        assert!(r1.is_none());
        insert(&db, image, "openai", "gpt-4o-mini", 50, "A red dog.")
            .await
            .unwrap();
        let r2 = lookup(&db, image, "openai", "gpt-4o-mini", 50)
            .await
            .unwrap();
        assert_eq!(r2.as_deref(), Some("A red dog."));
    }

    #[tokio::test]
    async fn different_params_miss() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let image = b"image";
        insert(&db, image, "openai", "gpt-4o-mini", 50, "first")
            .await
            .unwrap();
        let r = lookup(&db, image, "openai", "gpt-4o", 50).await.unwrap();
        assert!(r.is_none());
    }
}

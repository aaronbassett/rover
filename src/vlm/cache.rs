//! Caption-cache wrapper over `storage::image_captions`.
//!
//! Cache keys:
//! - `content_hash` is derived from the image bytes and the configured
//!   [`CacheRestrict`] scope (see [`content_hash`]):
//!   - `None`: `sha256(bytes)`
//!   - `Host`: `sha256(host ‖ sha256(bytes))`
//!   - `Page`: `sha256(url  ‖ sha256(bytes))`
//! - `params_hash = sha256(captioner_name ‖ RS ‖ captioner_model_id ‖ RS ‖ max_tokens)`
//!
//! The pair `(content_hash, params_hash)` is the primary key of the
//! `image_caption_cache` table (migration `007_image_caption_cache.sql`).
//! Scoping the content hash means an identical image cached under a `Host`
//! scope is reused for every page on that host, while a `Page` scope keys per
//! image URL so cross-page reuse is suppressed.

use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config::CacheRestrict;
use crate::storage::Db;
use crate::storage::image_captions;
use crate::vlm::error::VlmError;

const RS: char = '\u{1E}';

/// Derive the cache content-hash for `image_bytes` under `restrict`.
///
/// The inner `sha256(image_bytes)` digest (32 raw bytes) is always the trailing
/// component, so the host/url prefix and the digest cannot be confused: any two
/// `(scope-prefix, digest)` byte strings of equal length that compare equal must
/// have equal prefixes and equal digests. Returned as lowercase hex.
pub fn content_hash(image_bytes: &[u8], restrict: CacheRestrict, host: &str, url: &str) -> String {
    // Inner digest over the raw image bytes (32 bytes), shared by all scopes.
    let inner = Sha256::digest(image_bytes);
    match restrict {
        // Global scope: the bare content digest. Identical bytes share one row
        // regardless of host or page.
        CacheRestrict::None => hex_lower(&inner),
        // Per-host / per-page scope: prefix the scope string, then the inner
        // digest. The digest is the fixed-length trailing component, so prefix
        // and digest are unambiguous without a delimiter.
        CacheRestrict::Host => scoped_hash(host.as_bytes(), &inner),
        CacheRestrict::Page => scoped_hash(url.as_bytes(), &inner),
    }
}

/// `sha256(scope ‖ inner_digest)` as lowercase hex.
fn scoped_hash(scope: &[u8], inner_digest: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(scope);
    h.update(inner_digest);
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

/// Look up a fresh cached caption.
///
/// Returns `Ok(Some(caption))` only when a row exists *and* is within `ttl`
/// (`now - created_at <= ttl`, with a zero `ttl` always missing). A row older
/// than `ttl` is treated as absent so the caller falls through to the provider.
#[allow(clippy::too_many_arguments)]
pub async fn lookup(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
    restrict: CacheRestrict,
    host: &str,
    url: &str,
    ttl: Duration,
) -> Result<Option<String>, VlmError> {
    let ch = content_hash(image_bytes, restrict, host, url);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    let row = match image_captions::lookup(db, &ch, &ph)
        .await
        .map_err(VlmError::Storage)?
    {
        Some(r) => r,
        None => return Ok(None),
    };
    // TTL expiry: a row strictly older than `ttl` is treated as absent. A zero
    // `ttl` disables reuse entirely (it always expires, even at age 0) so the
    // caller always falls through to the provider.
    let now = jiff::Timestamp::now().as_second();
    let age = now.saturating_sub(row.created_at);
    let ttl_secs = i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX);
    if ttl.is_zero() || age > ttl_secs {
        return Ok(None);
    }
    Ok(Some(row.caption))
}

/// Insert a freshly produced caption (already hardened by the caller).
///
/// `raw_image` is written straight to the `raw_image_zstd` BLOB column — the
/// caller is responsible for zstd-compressing it (only when `store_raw_image`
/// is set), and passes `None` otherwise.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
    restrict: CacheRestrict,
    host: &str,
    url: &str,
    caption: &str,
    raw_image: Option<&[u8]>,
) -> Result<(), VlmError> {
    let ch = content_hash(image_bytes, restrict, host, url);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    image_captions::insert(db, &ch, &ph, caption, raw_image)
        .await
        .map_err(VlmError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash(
            b"hello",
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
        );
        let h2 = content_hash(
            b"hello",
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn restrict_to_changes_key() {
        let none = content_hash(b"img", CacheRestrict::None, "h.com", "https://h.com/a.png");
        let host = content_hash(b"img", CacheRestrict::Host, "h.com", "https://h.com/a.png");
        let page = content_hash(b"img", CacheRestrict::Page, "h.com", "https://h.com/a.png");
        assert_ne!(none, host);
        assert_ne!(host, page);
        assert_ne!(none, page);
        // same bytes, same host, different page → host scope matches, page scope differs
        assert_eq!(
            host,
            content_hash(b"img", CacheRestrict::Host, "h.com", "https://h.com/b.png")
        );
        assert_ne!(
            page,
            content_hash(b"img", CacheRestrict::Page, "h.com", "https://h.com/b.png")
        );
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
        let ttl = Duration::from_secs(3600);
        let r1 = lookup(
            &db,
            image,
            "openai",
            "gpt-4o-mini",
            50,
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
            ttl,
        )
        .await
        .unwrap();
        assert!(r1.is_none());
        insert(
            &db,
            image,
            "openai",
            "gpt-4o-mini",
            50,
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
            "A red dog.",
            None,
        )
        .await
        .unwrap();
        let r2 = lookup(
            &db,
            image,
            "openai",
            "gpt-4o-mini",
            50,
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
            ttl,
        )
        .await
        .unwrap();
        assert_eq!(r2.as_deref(), Some("A red dog."));
    }

    #[tokio::test]
    async fn different_params_miss() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let image = b"image";
        let ttl = Duration::from_secs(3600);
        insert(
            &db,
            image,
            "openai",
            "gpt-4o-mini",
            50,
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
            "first",
            None,
        )
        .await
        .unwrap();
        let r = lookup(
            &db,
            image,
            "openai",
            "gpt-4o",
            50,
            CacheRestrict::None,
            "h.com",
            "https://h.com/a.png",
            ttl,
        )
        .await
        .unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn expired_rows_miss() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        insert(
            &db,
            b"img",
            "n",
            "m",
            50,
            CacheRestrict::None,
            "h",
            "u",
            "old",
            None,
        )
        .await
        .unwrap();
        // ttl = 0 → any positive age is expired
        let hit = lookup(
            &db,
            b"img",
            "n",
            "m",
            50,
            CacheRestrict::None,
            "h",
            "u",
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(hit.is_none());
    }
}

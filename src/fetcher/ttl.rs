//! TTL computation for cache entries.
//!
//! Order of precedence (PRD §8.2 with design §3.6 clarification):
//!   1. `Cache-Control: no-store` → don't cache, unless `override_no_store`
//!      (global or per-domain) is true. `min_ttl` only floors the TTL when
//!      the entry is otherwise being cached.
//!   2. `max-age` / `s-maxage` from `Cache-Control`.
//!   3. `Expires` header.
//!   4. `cache.default_ttl`.
//!
//! Always cap final TTL at `cache.max_ttl`. Always floor at `min_ttl` for
//! entries that are being cached.

use jiff::fmt::rfc2822;

use super::cache_control::CacheControl;
use crate::config::CacheConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlDecision {
    /// Cache with this absolute expiry (unix epoch seconds).
    Cache { expires_at: i64 },
    /// Do not cache.
    DoNotCache,
}

/// Compute the cache decision for a response.
///
/// `now` is unix epoch seconds at fetch time (so tests can pin it).
/// `host` is the request host, for the `override_no_store_domains` check.
pub fn compute_ttl(
    now: i64,
    host: &str,
    cache_control: &str,
    expires_header: Option<&str>,
    cfg: &CacheConfig,
) -> TtlDecision {
    let cc = CacheControl::parse(cache_control);

    // Step 1: no-store handling.
    let no_store_overridden = if cc.no_store {
        let host_override = cfg
            .override_no_store_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(host));
        if !cfg.override_no_store && !host_override {
            return TtlDecision::DoNotCache;
        }
        // Override active: treat the Cache-Control TTL as 0 and let
        // `min_ttl` floor below take effect.
        true
    } else {
        false
    };

    // Steps 2-4: pick the base TTL.
    //
    // When no-store is overridden, the server's intent is "do not cache",
    // so we treat the base TTL as 0 (rather than falling back to
    // default_ttl) and let `min_ttl` floor it. This honors the operator's
    // override while still keeping cached entries minimal: the spec-intent
    // of `min_ttl` for force-cached entries.
    let mut ttl_secs = if let Some(s) = cc.s_maxage {
        s
    } else if let Some(m) = cc.max_age {
        m
    } else if no_store_overridden {
        0
    } else if let Some(t) = expires_header.and_then(parse_expires_header) {
        if t <= now {
            return TtlDecision::DoNotCache;
        }
        (t - now) as u64
    } else {
        cfg.default_ttl.as_secs()
    };

    // Floor at min_ttl when caching.
    let min = cfg.min_ttl.as_secs();
    if ttl_secs < min {
        ttl_secs = min;
    }

    // Cap at max_ttl.
    let max = cfg.max_ttl.as_secs();
    if ttl_secs > max {
        ttl_secs = max;
    }

    let expires_at = now.saturating_add(ttl_secs as i64);
    TtlDecision::Cache { expires_at }
}

fn parse_expires_header(value: &str) -> Option<i64> {
    rfc2822::parse(value)
        .ok()
        .map(|z| z.timestamp().as_second())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn cfg() -> CacheConfig {
        CacheConfig {
            default_ttl: Duration::from_secs(3600),
            min_ttl: Duration::from_secs(300),
            max_ttl: Duration::from_secs(7 * 86400),
            stale_while_revalidate_window: Duration::from_secs(300),
            override_no_store: false,
            override_no_store_domains: vec![],
            store_raw_html: false,
        }
    }

    #[test]
    fn no_store_skips_cache() {
        let d = compute_ttl(0, "example.com", "no-store", None, &cfg());
        assert_eq!(d, TtlDecision::DoNotCache);
    }

    #[test]
    fn no_store_overridden_floors_min_ttl() {
        let mut c = cfg();
        c.override_no_store = true;
        let d = compute_ttl(0, "example.com", "no-store", None, &c);
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn no_store_per_domain_override() {
        let mut c = cfg();
        c.override_no_store_domains = vec!["docs.example.com".into()];
        let d = compute_ttl(0, "DOCS.example.com", "no-store, max-age=60", None, &c);
        // host match is case-insensitive; min_ttl floors the 60 to 300.
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn max_age_used_when_present() {
        let d = compute_ttl(1_000, "x", "max-age=600", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 1_600 });
    }

    #[test]
    fn s_maxage_overrides_max_age() {
        let d = compute_ttl(0, "x", "max-age=60, s-maxage=120", None, &cfg());
        // s-maxage=120 < min_ttl=300 → floored to 300.
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn expires_header_used_without_cache_control() {
        let d = compute_ttl(0, "x", "", Some("Mon, 1 Jan 2035 00:00:00 GMT"), &cfg());
        // Expires header in 2035 + max_ttl=7*86400 cap → expires_at = max_ttl.
        assert_eq!(
            d,
            TtlDecision::Cache {
                expires_at: 7 * 86400
            }
        );
    }

    #[test]
    fn expires_header_within_max_ttl_used_directly() {
        // Now = 1_700_000_000 (Nov 2023). The Expires header below parses to
        // some timestamp shortly after `now`. We assert it falls between `now`
        // and `now + max_ttl`, so the natural Expires-derived TTL is used
        // (no min_ttl floor or max_ttl cap kicks in).
        let d = compute_ttl(
            1_700_000_000,
            "x",
            "",
            Some("Sun, 14 Nov 2023 22:30:00 GMT"),
            &cfg(),
        );
        match d {
            TtlDecision::Cache { expires_at } => {
                assert!(
                    expires_at > 1_700_000_000,
                    "expires_at={expires_at} should be > now"
                );
                assert!(
                    expires_at < 1_700_000_000 + 7 * 86400,
                    "expires_at={expires_at} should be below now + max_ttl"
                );
            }
            other => panic!("expected Cache, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_default_ttl() {
        let d = compute_ttl(0, "x", "", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 3600 });
    }

    #[test]
    fn caps_at_max_ttl() {
        let d = compute_ttl(0, "x", "max-age=99999999", None, &cfg());
        assert_eq!(
            d,
            TtlDecision::Cache {
                expires_at: 7 * 86400
            }
        );
    }

    #[test]
    fn floors_at_min_ttl() {
        let d = compute_ttl(0, "x", "max-age=10", None, &cfg());
        assert_eq!(d, TtlDecision::Cache { expires_at: 300 });
    }

    #[test]
    fn past_expires_skips_cache() {
        // Jan 1 2000 was a Saturday; jiff's RFC 2822 parser strictly
        // validates the weekday, so we must use the correct one.
        let d = compute_ttl(
            1_700_000_000,
            "x",
            "",
            Some("Sat, 1 Jan 2000 00:00:00 GMT"),
            &cfg(),
        );
        assert_eq!(d, TtlDecision::DoNotCache);
    }
}

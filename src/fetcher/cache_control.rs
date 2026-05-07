//! RFC 9111 §5.2 `Cache-Control` directive parser (response side).
//!
//! Only the directives we use today: `max-age`, `s-maxage`, `no-store`,
//! `no-cache`, `must-revalidate`, `public`, `private`. Unknown directives
//! are tolerated and ignored — robust to non-compliant origins.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    pub no_store: bool,
    pub no_cache: bool,
    pub must_revalidate: bool,
    pub public: bool,
    pub private: bool,
}

impl CacheControl {
    /// Parse a `Cache-Control` header value. Multiple `Cache-Control` headers
    /// may be combined by the caller into a single comma-separated string
    /// before parsing.
    pub fn parse(header: &str) -> Self {
        let mut out = Self::default();
        for token in header.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let (name, value) = match token.split_once('=') {
                Some((n, v)) => (n.trim(), Some(strip_quotes(v.trim()))),
                None => (token, None),
            };
            match name.to_ascii_lowercase().as_str() {
                "max-age" => out.max_age = value.and_then(|v| v.parse().ok()),
                "s-maxage" => out.s_maxage = value.and_then(|v| v.parse().ok()),
                "no-store" => out.no_store = true,
                "no-cache" => out.no_cache = true,
                "must-revalidate" => out.must_revalidate = true,
                "public" => out.public = true,
                "private" => out.private = true,
                _ => {} // ignore unknowns
            }
        }
        out
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(CacheControl::parse(""), CacheControl::default());
    }

    #[test]
    fn parses_max_age() {
        let cc = CacheControl::parse("max-age=3600");
        assert_eq!(cc.max_age, Some(3600));
    }

    #[test]
    fn parses_quoted_values() {
        let cc = CacheControl::parse(r#"max-age="600""#);
        assert_eq!(cc.max_age, Some(600));
    }

    #[test]
    fn case_insensitive_directives() {
        let cc = CacheControl::parse("MAX-AGE=42, NO-STORE");
        assert_eq!(cc.max_age, Some(42));
        assert!(cc.no_store);
    }

    #[test]
    fn parses_combined_directives() {
        let cc = CacheControl::parse("public, max-age=300, must-revalidate");
        assert!(cc.public);
        assert_eq!(cc.max_age, Some(300));
        assert!(cc.must_revalidate);
        assert!(!cc.no_store);
    }

    #[test]
    fn s_maxage_separate_from_max_age() {
        let cc = CacheControl::parse("max-age=60, s-maxage=600");
        assert_eq!(cc.max_age, Some(60));
        assert_eq!(cc.s_maxage, Some(600));
    }

    #[test]
    fn ignores_unknown_directives() {
        let cc = CacheControl::parse("immutable, max-age=100, stale-while-revalidate=30");
        assert_eq!(cc.max_age, Some(100));
        assert!(!cc.no_store);
    }

    #[test]
    fn no_store_no_cache_are_independent() {
        let cc = CacheControl::parse("no-store");
        assert!(cc.no_store && !cc.no_cache);
        let cc = CacheControl::parse("no-cache");
        assert!(!cc.no_store && cc.no_cache);
    }

    #[test]
    fn malformed_max_age_yields_none() {
        let cc = CacheControl::parse("max-age=not-a-number");
        assert_eq!(cc.max_age, None);
    }
}

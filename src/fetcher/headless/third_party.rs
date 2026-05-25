//! Minimal EasyList-derived block list for headless asset filtering.
//!
//! Trades completeness for binary size and simplicity. We block the most
//! common third-party trackers/analytics/ads; everything else is allowed
//! through (subject to the per-resource-type config gates).

use url::Url;

const BLOCK_DOMAINS: &[&str] = &[
    // Analytics & tracking
    "google-analytics.com",
    "googletagmanager.com",
    "doubleclick.net",
    "scorecardresearch.com",
    "facebook.net",
    "connect.facebook.net",
    "platform.twitter.com",
    "segment.io",
    "mixpanel.com",
    "hotjar.com",
    "fullstory.com",
    "intercom.io",
    "drift.com",
    // Ads
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "adnxs.com",
    "criteo.com",
    "outbrain.com",
    "taboola.com",
    // CDN tag-managers / pixel beacons
    "segment.com",
    "snowplowanalytics.com",
    "amplitude.com",
];

/// Return true if `url` host matches a known third-party tracker/analytics
/// domain. Suffix match: `foo.bar.google-analytics.com` matches.
pub fn matches(url: &Url, _frame_first_party_host: &str) -> bool {
    let host = match url.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };
    BLOCK_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_domain() {
        let u = Url::parse("https://google-analytics.com/collect").unwrap();
        assert!(matches(&u, "example.com"));
    }

    #[test]
    fn matches_subdomain() {
        let u = Url::parse("https://www.google-analytics.com/collect").unwrap();
        assert!(matches(&u, "example.com"));
        let u = Url::parse("https://cdn.connect.facebook.net/foo.js").unwrap();
        assert!(matches(&u, "example.com"));
    }

    #[test]
    fn does_not_match_unrelated_host() {
        let u = Url::parse("https://example.com/foo.js").unwrap();
        assert!(!matches(&u, "example.com"));
    }

    #[test]
    fn url_without_host_does_not_match() {
        let u = Url::parse("data:text/plain,hi").unwrap();
        assert!(!matches(&u, "example.com"));
    }
}

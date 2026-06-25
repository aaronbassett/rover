//! Bot-protection challenge detection.
//!
//! Edge providers (Vercel, Cloudflare, …) front origins with managed
//! "challenge" responses: instead of the page, they return a small interstitial
//! plus a marker header, asking the client to execute a JavaScript proof-of-work
//! / browser-integrity check before being let through. A plain HTTP client
//! (reqwest) can never satisfy that — it has no JS engine — so retrying the same
//! request is pointless and just burns the backoff budget.
//!
//! We detect these by their definitive response headers (not status codes, which
//! vary: Vercel uses 429, Cloudflare 403/503). A detected challenge is surfaced
//! as [`crate::fetcher::FetcherError::BotChallenge`], which the retry layer
//! treats as fatal (no retries) and the Auto-mode fetcher can hand to the
//! headless renderer — a real browser *can* solve the challenge.

/// A recognized bot-protection provider presenting a managed challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    /// Vercel "Attack Challenge Mode" / managed challenge
    /// (`x-vercel-mitigated: challenge`).
    Vercel,
    /// Cloudflare managed/interactive challenge (`cf-mitigated: challenge`).
    Cloudflare,
}

impl ChallengeKind {
    /// Human-readable provider name for error messages.
    pub fn provider(self) -> &'static str {
        match self {
            ChallengeKind::Vercel => "Vercel",
            ChallengeKind::Cloudflare => "Cloudflare",
        }
    }
}

/// Inspect a response's status and headers for a managed bot-challenge marker.
///
/// `headers` is a list of `(name, value)` pairs as captured from the response;
/// names are matched case-insensitively (HTTP header names are case-insensitive
/// and reqwest lower-cases them, but we don't rely on that).
pub fn detect(status: u16, headers: &[(String, String)]) -> Option<ChallengeKind> {
    let header = |name: &str| -> Option<&str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };

    // Vercel: the `x-vercel-mitigated: challenge` header is definitive; the
    // accompanying `x-vercel-challenge-token` is a secondary tell.
    if header("x-vercel-mitigated").is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
        || header("x-vercel-challenge-token").is_some()
    {
        return Some(ChallengeKind::Vercel);
    }

    // Cloudflare: `cf-mitigated: challenge` is the modern definitive marker for
    // a managed/interactive challenge.
    if header("cf-mitigated").is_some_and(|v| v.eq_ignore_ascii_case("challenge")) {
        return Some(ChallengeKind::Cloudflare);
    }

    // Status alone is never sufficient — a bare 429/403 is ordinary
    // rate-limiting / forbidden, which the retry layer handles normally.
    let _ = status;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detects_vercel_mitigated_header() {
        let headers = h(&[("x-vercel-mitigated", "challenge"), ("server", "Vercel")]);
        assert_eq!(detect(429, &headers), Some(ChallengeKind::Vercel));
    }

    #[test]
    fn detects_vercel_via_challenge_token() {
        let headers = h(&[("x-vercel-challenge-token", "2.1782...")]);
        assert_eq!(detect(429, &headers), Some(ChallengeKind::Vercel));
    }

    #[test]
    fn header_name_match_is_case_insensitive() {
        let headers = h(&[("X-Vercel-Mitigated", "Challenge")]);
        assert_eq!(detect(429, &headers), Some(ChallengeKind::Vercel));
    }

    #[test]
    fn detects_cloudflare_mitigated_header() {
        let headers = h(&[("cf-mitigated", "challenge"), ("server", "cloudflare")]);
        assert_eq!(detect(403, &headers), Some(ChallengeKind::Cloudflare));
    }

    #[test]
    fn plain_429_without_markers_is_not_a_challenge() {
        let headers = h(&[("retry-after", "30"), ("server", "nginx")]);
        assert_eq!(detect(429, &headers), None);
    }

    #[test]
    fn cf_mitigated_non_challenge_value_is_ignored() {
        // Cloudflare also emits `cf-mitigated` for non-challenge mitigations.
        let headers = h(&[("cf-mitigated", "block")]);
        assert_eq!(detect(403, &headers), None);
    }

    #[test]
    fn provider_names() {
        assert_eq!(ChallengeKind::Vercel.provider(), "Vercel");
        assert_eq!(ChallengeKind::Cloudflare.provider(), "Cloudflare");
    }
}

//! HTTP client construction.

use reqwest::redirect::Policy;
use std::sync::OnceLock;
use std::time::Duration;

use crate::fetcher::dns::shared_resolver;

/// Install the ring-backed rustls crypto provider exactly once per process.
///
/// Must be called before any `reqwest::Client` (or other rustls consumer) is
/// built.  Subsequent calls are harmless — the `OnceLock` gate ensures the
/// `install_default` attempt is only made once, and the `Result` from
/// `install_default` is intentionally discarded via `.ok()` because a second
/// concurrent caller racing through the lock before the first has stored the
/// sentinel would receive `Err(AlreadyInstalled)`, which is not an error.
pub fn install_ring_provider() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
    });
}

/// Build a `reqwest::Client` configured for Rover's fetch defaults.
///
/// Per PRD §5.2: max 10 redirects.
///
/// Installs a custom DNS resolver ([`crate::fetcher::dns::SsrfValidatingResolver`])
/// that re-runs the SSRF address policy at dial time, closing the
/// resolve-then-dial TOCTOU window. Callers that should be policed must
/// wrap their `send().await` in
/// [`crate::fetcher::dns::SSRF_LEVEL`]`.scope(level, ...)`.
pub fn build_http_client(user_agent: &str, timeout: Duration) -> reqwest::Client {
    install_ring_provider();
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .redirect(Policy::limited(10))
        .dns_resolver(shared_resolver())
        .build()
        .expect("reqwest::Client::builder() should not fail with these defaults")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_with_defaults() {
        let _client = build_http_client("test/0.1", Duration::from_secs(15));
        // Mere fact of building without panicking is the assertion. reqwest's
        // builder can fail (e.g. when TLS backends are misconfigured), so we
        // exercise the path here.
    }
}

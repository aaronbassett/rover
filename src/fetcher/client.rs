//! HTTP client construction.

use std::time::Duration;
use reqwest::redirect::Policy;

/// Build a `reqwest::Client` configured for Rover's fetch defaults.
///
/// Per PRD §5.2: max 10 redirects.
pub fn build_http_client(user_agent: &str, timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .redirect(Policy::limited(10))
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

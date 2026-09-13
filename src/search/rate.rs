//! Client-side pacing for the search endpoint.
//!
//! Thin wrapper over the same `governor`-backed [`HostRateLimiter`] the
//! fetcher uses for per-domain pacing, keyed on a single endpoint. Reusing
//! that primitive keeps one token-bucket implementation in the tree; what
//! differs is the *policy* around it — search is a metered API where the
//! sensible default (60 rpm) mirrors the provider's own free-tier ceiling
//! rather than a politeness budget for someone else's origin server.

use crate::fetcher::rate_limit::HostRateLimiter;

/// A one-key token bucket in front of the search endpoint.
pub struct EndpointPacer {
    inner: HostRateLimiter,
}

impl EndpointPacer {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            inner: HostRateLimiter::new(requests_per_minute),
        }
    }

    /// Wait until the endpoint budget allows another request.
    pub async fn acquire(&self) {
        self.inner.until_ready("search").await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn first_request_is_immediate() {
        let p = EndpointPacer::new(60);
        let start = Instant::now();
        p.acquire().await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn the_bucket_actually_paces() {
        // Burst = 60 at 60rpm; the 61st has to wait for a refill.
        let p = EndpointPacer::new(60);
        for _ in 0..60 {
            p.acquire().await;
        }
        let start = Instant::now();
        let waited = tokio::time::timeout(Duration::from_secs(2), p.acquire()).await;
        assert!(waited.is_ok(), "the bucket should eventually refill");
        assert!(
            start.elapsed() >= Duration::from_millis(500),
            "elapsed = {:?}",
            start.elapsed()
        );
    }
}

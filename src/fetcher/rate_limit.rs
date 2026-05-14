//! Thin async wrapper around `governor` keyed by host.
//!
//! Used by `Pacer` (in `concurrency.rs`) to enforce the per-domain token
//! bucket from `[rate_limit] requests_per_minute_per_domain`. See M5 design
//! spec §3.2 / §3.6 for the rationale.

use std::num::NonZeroU32;

use governor::{Quota, RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};

/// Per-host token bucket. Internally a `governor::RateLimiter` keyed by
/// `String`, refilling at `rpm / 60` tokens per second per host.
pub struct HostRateLimiter {
    inner: RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>,
}

impl HostRateLimiter {
    pub fn new(rpm: u32) -> Self {
        // rpm = 0 is rejected by config validation; defensive clamp anyway.
        let rpm = rpm.max(1);
        // Quota::per_minute(N) emits up-to-N events per minute with burst = N
        // (one minute's worth). This matches PRD intent: smooth pacing with
        // a 1-minute burst budget per host.
        let per_minute = NonZeroU32::new(rpm).expect("rpm > 0");
        let quota = Quota::per_minute(per_minute);
        let inner = RateLimiter::keyed(quota);
        Self { inner }
    }

    /// Wait until a token is available for `host`, then consume one.
    pub async fn until_ready(&self, host: &str) {
        // governor's keyed limiter consumes a token on success; until_ready
        // loops internally on the clock until allowance.
        self.inner.until_key_ready(&host.to_string()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn first_token_is_immediate() {
        let r = HostRateLimiter::new(60);
        let start = Instant::now();
        r.until_ready("a.example").await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn second_token_after_burst_waits_at_least_one_period() {
        // With 60 rpm, burst = 60. Consume the entire burst, then the 61st
        // request must wait for the bucket to replenish at least one token.
        // At 60 rpm the replenishment interval is ~1000ms; allow scheduler
        // jitter with a 500ms floor.
        let r = HostRateLimiter::new(60);
        for _ in 0..60 {
            r.until_ready("example.com").await;
        }
        let start = Instant::now();
        let waited =
            tokio::time::timeout(Duration::from_secs(2), r.until_ready("example.com")).await;
        assert!(waited.is_ok(), "61st token should eventually be ready");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(500),
            "elapsed = {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn per_host_buckets_are_independent() {
        // rpm = 60 → burst = 60. Exhaust host A; host B should still be
        // immediately ready.
        let r = HostRateLimiter::new(60);
        for _ in 0..60 {
            r.until_ready("a.example").await;
        }
        let start = Instant::now();
        r.until_ready("b.example").await;
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "B was blocked by A"
        );
    }
}

//! Pacer: the single ownership point for all per-process pacing state.
//!
//! Owns four pieces:
//! - a global `tokio::sync::Semaphore`,
//! - a per-host `tokio::sync::Semaphore` registry,
//! - a governor keyed rate limiter (in `rate_limit.rs`),
//! - a per-host `last_request_at` map for Crawl-Delay floor enforcement.
//!
//! See M5 design spec §3.2 for the full picture.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::config::RateLimitConfig;

use super::rate_limit::HostRateLimiter;

/// Build-once-at-startup pacing state.
pub struct Pacer {
    rate_limit: HostRateLimiter,
    global: Arc<Semaphore>,
    per_host: Mutex<HashMap<String, Arc<Semaphore>>>,
    min_interval: Mutex<HashMap<String, Instant>>,
    pub(crate) per_host_limit: u32,
}

/// Permits + bookkeeping released when the guard is dropped.
///
/// On drop, when `updates_min_interval` is true, records `Instant::now()` in
/// `Pacer::min_interval[host]` so subsequent fetches to the same host respect
/// the Crawl-Delay floor measured from completion of the previous request.
pub struct PacerGuard<'a> {
    pacer: &'a Pacer,
    host: String,
    _per_host_permit: Option<OwnedSemaphorePermit>,
    _global_permit: OwnedSemaphorePermit,
    updates_min_interval: bool,
}

impl Drop for PacerGuard<'_> {
    fn drop(&mut self) {
        if !self.updates_min_interval {
            return;
        }
        // try_lock to avoid blocking; on contention, skip — the worst case is
        // one extra request without Crawl-Delay floor on the very next call,
        // which is acceptable.
        if let Ok(mut map) = self.pacer.min_interval.try_lock() {
            map.insert(self.host.clone(), Instant::now());
        }
    }
}

impl Pacer {
    pub fn new(cfg: &RateLimitConfig) -> Self {
        Self {
            rate_limit: HostRateLimiter::new(cfg.requests_per_minute_per_domain),
            global: Arc::new(Semaphore::new(cfg.global_concurrency as usize)),
            per_host: Mutex::new(HashMap::new()),
            min_interval: Mutex::new(HashMap::new()),
            per_host_limit: cfg.per_domain_concurrency,
        }
    }

    /// Acquire the full pacing stack: per-host slot → global slot → governor
    /// token → Crawl-Delay floor wait.
    ///
    /// `crawl_delay` is `Some(d)` when the robots.txt for this host advertises
    /// a `Crawl-Delay` directive; `None` otherwise.
    pub async fn acquire(&self, host: &str, crawl_delay: Option<Duration>) -> PacerGuard<'_> {
        // 1. Per-host semaphore.
        let per_host_sem = self.host_semaphore(host).await;
        let per_host_permit = per_host_sem
            .acquire_owned()
            .await
            .expect("per-host semaphore must not be closed");

        // 2. Global semaphore.
        let global_permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");

        // 3. Governor token.
        self.rate_limit.until_ready(host).await;

        // 4. Crawl-Delay floor.
        if let Some(delay) = crawl_delay {
            let last = self.min_interval.lock().await.get(host).copied();
            if let Some(last) = last {
                let elapsed = last.elapsed();
                if elapsed < delay {
                    tokio::time::sleep(delay - elapsed).await;
                }
            }
        }

        PacerGuard {
            pacer: self,
            host: host.to_string(),
            _per_host_permit: Some(per_host_permit),
            _global_permit: global_permit,
            updates_min_interval: true,
        }
    }

    /// Acquire global slot + governor token only. Used by robots fetches to
    /// avoid the chicken-and-egg with `crawl_delay`. Does NOT update
    /// `last_request_at` on drop.
    pub async fn acquire_global_only(&self, host: &str) -> PacerGuard<'_> {
        let global_permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        self.rate_limit.until_ready(host).await;
        PacerGuard {
            pacer: self,
            host: host.to_string(),
            _per_host_permit: None,
            _global_permit: global_permit,
            updates_min_interval: false,
        }
    }

    async fn host_semaphore(&self, host: &str) -> Arc<Semaphore> {
        let mut map = self.per_host.lock().await;
        map.entry(host.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_host_limit as usize)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    fn small_cfg(rpm: u32, global: u32, per_host: u32) -> RateLimitConfig {
        RateLimitConfig {
            requests_per_minute_per_domain: rpm,
            per_domain_concurrency: per_host,
            global_concurrency: global,
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            retry_after_ceiling: Duration::from_secs(300),
            jitter_seed: Some(1),
        }
    }

    #[tokio::test]
    async fn acquire_returns_a_guard() {
        let p = Pacer::new(&small_cfg(6000, 4, 2));
        let _g = p.acquire("example.com", None).await;
    }

    #[tokio::test]
    async fn global_cap_blocks_when_exhausted() {
        let p = Pacer::new(&small_cfg(6000, 1, 4));
        let _g1 = p.acquire("a.example", None).await;
        // Second acquire must block — timeout proves no progress in 50ms.
        let result = timeout(Duration::from_millis(50), p.acquire("b.example", None)).await;
        assert!(result.is_err(), "second acquire should block while g1 held");
    }

    #[tokio::test]
    async fn per_host_cap_blocks_within_same_host() {
        let p = Pacer::new(&small_cfg(6000, 8, 1));
        let _g1 = p.acquire("example.com", None).await;
        let result = timeout(Duration::from_millis(50), p.acquire("example.com", None)).await;
        assert!(result.is_err(), "second acquire on same host should block");
    }

    #[tokio::test]
    async fn per_host_isolation_other_host_proceeds() {
        let p = Pacer::new(&small_cfg(6000, 8, 1));
        let _g1 = p.acquire("a.example", None).await;
        timeout(Duration::from_millis(50), p.acquire("b.example", None))
            .await
            .expect("different host should not be blocked");
    }

    #[tokio::test]
    async fn acquire_global_only_skips_per_host() {
        let p = Pacer::new(&small_cfg(6000, 8, 1));
        let _g1 = p.acquire("example.com", None).await;
        timeout(
            Duration::from_millis(50),
            p.acquire_global_only("example.com"),
        )
        .await
        .expect("global-only should ignore per-host bucket");
    }

    #[tokio::test(start_paused = true)]
    async fn crawl_delay_blocks_second_acquire_for_same_host() {
        use tokio::sync::oneshot;
        let p = Arc::new(Pacer::new(&small_cfg(6000, 8, 4)));
        let g1 = p.acquire("example.com", Some(Duration::from_secs(2))).await;
        drop(g1); // records last_request_at = now.

        let (tx, rx) = oneshot::channel::<()>();
        let p2 = p.clone();
        let join = tokio::spawn(async move {
            let _g2 = p2
                .acquire("example.com", Some(Duration::from_secs(2)))
                .await;
            let _ = tx.send(());
            // Hold the guard until the test drops the receiver / returns.
            tokio::task::yield_now().await;
        });

        // Advance virtual time by 1.5s; tx should not have fired yet.
        tokio::time::advance(Duration::from_millis(1500)).await;
        assert!(!join.is_finished(), "should still be sleeping at 1.5s");

        // Advance past the floor; the spawned task should send shortly.
        tokio::time::advance(Duration::from_millis(700)).await;
        let _ = rx.await; // wait for guard acquisition signal
        let _ = join.await;
    }
}

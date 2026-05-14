//! Global + per-host concurrency caps.
//!
//! Owns two `tokio::sync::Semaphore` instances per the M5 design spec §3.2:
//! one global, one per host (constructed lazily on first sight). Per-host
//! permit is acquired before the global one so a single host cannot
//! monopolise the global cap.
//!
//! The full `Pacer` (with governor + min-interval map) lands in Task 7. This
//! task introduces the skeleton.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

/// Build-once-at-startup pacing state. `Arc<Pacer>` is shared across all
/// HTTP-bound code paths.
pub struct Pacer {
    pub(crate) global: Arc<Semaphore>,
    pub(crate) per_host: Mutex<HashMap<String, Arc<Semaphore>>>,
    pub(crate) per_host_limit: u32,
}

/// Permits + bookkeeping released when the guard is dropped.
pub struct PacerGuard {
    _per_host_permit: Option<OwnedSemaphorePermit>,
    _global_permit: OwnedSemaphorePermit,
    // The full guard in Task 7 also carries host + updates_min_interval +
    // a back-reference to Pacer. Kept minimal here.
}

impl Pacer {
    /// Build a Pacer with the given global cap and per-host cap. Per-host
    /// semaphores are created lazily on first acquire.
    pub fn new(global_concurrency: u32, per_host_concurrency: u32) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_concurrency as usize)),
            per_host: Mutex::new(HashMap::new()),
            per_host_limit: per_host_concurrency,
        }
    }

    /// Acquire (per-host, global) in that order.
    pub async fn acquire(&self, host: &str) -> PacerGuard {
        let per_host_sem = self.host_semaphore(host).await;
        let per_host = per_host_sem
            .acquire_owned()
            .await
            .expect("per-host semaphore must not be closed");
        let global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        PacerGuard {
            _per_host_permit: Some(per_host),
            _global_permit: global,
        }
    }

    /// Acquire only the global semaphore — used by robots fetches (per the
    /// chicken-and-egg argument in M5 design spec §3.4).
    pub async fn acquire_global_only(&self) -> PacerGuard {
        let global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore must not be closed");
        PacerGuard {
            _per_host_permit: None,
            _global_permit: global,
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
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    #[tokio::test]
    async fn acquire_returns_a_guard() {
        let p = Pacer::new(4, 2);
        let _g = p.acquire("example.com").await;
        // Drop g at end of scope; permits released.
    }

    #[tokio::test]
    async fn global_cap_blocks_when_exhausted() {
        let p = Arc::new(Pacer::new(1, 4));
        let g1 = p.acquire("a.example").await;
        // Second acquire must block; bounded wait verifies it doesn't proceed.
        let p2 = p.clone();
        let join = tokio::spawn(async move { p2.acquire("b.example").await });
        let result = timeout(Duration::from_millis(50), join).await;
        assert!(
            result.is_err(),
            "second acquire should block until g1 drops"
        );
        drop(g1);
        // After drop, second acquire should resolve quickly.
        // (Detached task may still be pending; spawn a new acquire.)
        let _g3 = timeout(Duration::from_millis(50), p.acquire("c.example"))
            .await
            .expect("global slot should be free after drop");
    }

    #[tokio::test]
    async fn per_host_cap_blocks_within_same_host() {
        let p = Arc::new(Pacer::new(8, 1));
        let g1 = p.acquire("example.com").await;
        let p2 = p.clone();
        let join = tokio::spawn(async move { p2.acquire("example.com").await });
        let result = timeout(Duration::from_millis(50), join).await;
        assert!(result.is_err(), "second acquire on same host should block");
        drop(g1);
        let _g2 = timeout(Duration::from_millis(50), p.acquire("example.com"))
            .await
            .expect("host slot should be free after drop");
    }

    #[tokio::test]
    async fn per_host_isolation_other_host_proceeds() {
        let p = Arc::new(Pacer::new(8, 1));
        let _g1 = p.acquire("a.example").await;
        // Different host: should proceed immediately even though "a.example"
        // has its 1-slot bucket fully occupied.
        let _g2 = timeout(Duration::from_millis(50), p.acquire("b.example"))
            .await
            .expect("different host should not be blocked");
    }

    #[tokio::test]
    async fn acquire_global_only_skips_per_host() {
        let p = Arc::new(Pacer::new(8, 1));
        let _g1 = p.acquire("example.com").await; // uses 1/1 per-host slot
        // acquire_global_only must not contend on the per-host semaphore.
        let _g2 = timeout(Duration::from_millis(50), p.acquire_global_only())
            .await
            .expect("global-only acquire should ignore per-host bucket");
        // Touch the variable so clippy doesn't complain.
        sleep(Duration::from_millis(1)).await;
    }
}

//! Per-domain concurrency limiter for image HTTP requests.
//!
//! [`DomainLimiter`] keeps a lazily-created `Arc<Semaphore>` per host name.
//! Callers acquire an [`OwnedSemaphorePermit`] before issuing an HTTP request
//! so that no more than `per_domain` requests fly concurrently to the same
//! origin.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Limits concurrent image HTTP requests on a per-hostname basis.
pub struct DomainLimiter {
    cap: usize,
    map: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl DomainLimiter {
    /// Create a new limiter with `per_domain` concurrent permits per host.
    /// A value of 0 is normalised to 1 (at least one permit is always issued).
    pub fn new(per_domain: u32) -> Self {
        Self {
            cap: per_domain.max(1) as usize,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire one permit for `host`, creating the host's semaphore on first
    /// use. The permit is released when the returned [`OwnedSemaphorePermit`]
    /// is dropped.
    pub async fn acquire(&self, host: &str) -> OwnedSemaphorePermit {
        let sem = {
            let mut map = self.map.lock().unwrap();
            map.entry(host.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.cap)))
                .clone()
        };
        sem.acquire_owned()
            .await
            .expect("DomainLimiter semaphore unexpectedly closed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_caps_concurrent_permits_per_host() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let lim = Arc::new(DomainLimiter::new(2));
        let inflight = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let mut hs = vec![];
        for _ in 0..8 {
            let (lim, inflight, max) = (lim.clone(), inflight.clone(), max.clone());
            hs.push(tokio::spawn(async move {
                let _p = lim.acquire("example.com").await;
                let n = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(n, Ordering::SeqCst);
                tokio::task::yield_now().await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in hs {
            h.await.unwrap();
        }
        assert!(
            max.load(Ordering::SeqCst) <= 2,
            "max in-flight was {}",
            max.load(Ordering::SeqCst)
        );
    }
}

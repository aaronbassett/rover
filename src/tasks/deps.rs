//! Shared dependency bundle for every worker.
//!
//! `batch_fetch`, `retry`, and `revalidate` all need the same HTTP client +
//! pacer + four config blocks + SSRF level. The original M6 implementation
//! defined three byte-identical structs (`BatchDeps`/`RetryDeps`/`RevalidateDeps`);
//! collapsing them into one keeps the three-clone wiring in `mcp::server`
//! honest and saves three identical test-fixture helpers.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{CacheConfig, FetchConfig, RateLimitConfig, RobotsConfig};
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;

#[derive(Clone)]
pub struct WorkerDeps {
    pub client: reqwest::Client,
    pub pacer: Arc<Pacer>,
    pub cache_cfg: CacheConfig,
    pub rate_cfg: RateLimitConfig,
    pub robots_cfg: RobotsConfig,
    pub fetch_cfg: FetchConfig,
    pub ssrf_level: SsrfLevel,
    /// Pre-canonicalized project root used when `ssrf_level == Project` to
    /// validate `file://` URLs. `None` for every other level.
    pub ssrf_project_root: Option<PathBuf>,
    /// Optional HAR recorder. When `Some`, every fetch in background workers
    /// is recorded into the shared recorder used by the foreground server.
    pub har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
}

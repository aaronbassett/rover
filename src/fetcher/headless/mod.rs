//! Headless browser support for SPA pages.
//!
//! Gated by the `headless` Cargo feature. Public surface:
//! - `HeadlessRenderer` — owns one `chromiumoxide::Browser` for the process
//!   lifetime + a page-level `Semaphore`.
//! - `HeadlessMode` — per-call mode: `Off | On | Auto`.
//! - `RenderedPage` — output of `HeadlessRenderer::render`.
//! - `HeadlessError` — per-module thiserror enum.
//!
//! Submodules:
//! - `browser` — browser launch + page-pool helpers.
//! - `detect` — SPA detection heuristics for the `Auto` mode.
//! - `intercept` — CDP Fetch domain handler.
//! - `third_party` — minimal EasyList-derived block list.

pub mod browser;
pub mod detect;
pub mod intercept;
pub mod third_party;

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessMode {
    Off,
    On,
    Auto,
}

impl HeadlessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            HeadlessMode::Off => "off",
            HeadlessMode::On => "on",
            HeadlessMode::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub final_url: Url,
    pub html: String,
    pub status: u16,
}

#[derive(Debug, Error)]
pub enum HeadlessError {
    #[error("browser launch failed: {0}")]
    LaunchFailed(String),

    #[error("browser config invalid: {0}")]
    ConfigInvalid(String),

    #[error("render timeout after {timeout_secs}s on {url}")]
    Timeout { url: String, timeout_secs: u32 },

    #[error("page closed unexpectedly: {0}")]
    PageClosed(String),

    #[error("CDP error: {0}")]
    Cdp(String),

    #[error("renderer semaphore closed")]
    SemaphoreClosed,
}

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::fetch::{
    EnableParams as FetchEnableParams, EventRequestPaused,
};
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkEnableParams, EventLoadingFailed, EventLoadingFinished,
    EventRequestWillBeSent, RequestId,
};
use futures::StreamExt;

use crate::config::HeadlessConfig;
use crate::fetcher::ssrf::SsrfLevel;

/// Owns one `chromiumoxide::Browser` instance for the process lifetime plus
/// the per-page concurrency semaphore. Built once at server startup (when the
/// `headless` Cargo feature is enabled and the config block is present) and
/// shared via `Arc` through `FetchOptions`.
pub struct HeadlessRenderer {
    browser: Browser,
    handler_task: JoinHandle<()>,
    permit: Arc<Semaphore>,
    asset_cfg: HeadlessConfig,
    /// Per-instance Chrome user-data directory. Held for the browser's
    /// lifetime and removed on drop; keeping it here is what prevents
    /// concurrent renderers from sharing (and deadlocking on) one profile.
    _profile_dir: tempfile::TempDir,
}

impl std::fmt::Debug for HeadlessRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessRenderer")
            .field("asset_cfg", &self.asset_cfg)
            .finish_non_exhaustive()
    }
}

impl HeadlessRenderer {
    /// Launch the browser and prepare the renderer. The returned value owns
    /// a background tokio task that drives chromiumoxide's event loop; call
    /// [`HeadlessRenderer::shutdown`] to stop it cleanly.
    pub async fn new(cfg: &HeadlessConfig) -> Result<Self, HeadlessError> {
        let (browser, handler_task, profile_dir) = browser::launch(cfg).await?;
        let permits = if cfg.max_concurrent == 0 {
            4
        } else {
            cfg.max_concurrent
        };
        Ok(Self {
            browser,
            handler_task,
            permit: Arc::new(Semaphore::new(permits)),
            asset_cfg: cfg.clone(),
            _profile_dir: profile_dir,
        })
    }

    /// Render the given URL in a fresh page. The page-level intercept
    /// listener applies the SSRF gate and the resource-type/third-party
    /// block list to every sub-request before it leaves the browser.
    pub async fn render(
        &self,
        url: &url::Url,
        ssrf_level: SsrfLevel,
        ssrf_project_root: Option<&std::path::Path>,
    ) -> Result<RenderedPage, HeadlessError> {
        let _guard = self
            .permit
            .acquire()
            .await
            .map_err(|_| HeadlessError::SemaphoreClosed)?;

        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        // Enable the Fetch domain for request interception.
        page.execute(FetchEnableParams::default())
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        // Spawn an interception listener scoped to this page.
        let asset_cfg = self.asset_cfg.clone();
        let project_root = ssrf_project_root.map(|p| p.to_path_buf());
        let level = ssrf_level;
        let page_for_intercept = page.clone();
        let mut events = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
        let intercept_task: JoinHandle<()> = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let pr = project_root.as_deref();
                let _ = intercept::handle_paused(
                    &page_for_intercept,
                    (*event).clone(),
                    &asset_cfg,
                    level,
                    pr,
                )
                .await;
            }
        });

        // Start network-idle tracking *before* navigating. chromiumoxide's
        // event listeners only deliver events emitted after they attach, so a
        // tracker created after `goto()` races the page's own subresource
        // requests and can miss the very XHR a `networkidle0` wait exists for.
        let idle_tracker = if self.asset_cfg.default_wait == "networkidle0" {
            match NetworkIdleTracker::start(&page).await {
                Ok(t) => Some(t),
                Err(e) => {
                    intercept_task.abort();
                    let _ = page.close().await;
                    return Err(e);
                }
            }
        } else {
            None
        };

        let url_str = url.to_string();

        // Navigate. The wait phase below is what enforces the per-render
        // timeout; `goto` itself returns once the CDP `Page.navigate` command
        // has been ack'd.
        if let Err(e) = page.goto(url.as_str()).await {
            intercept_task.abort();
            let _ = page.close().await;
            return Err(HeadlessError::Cdp(e.to_string()));
        }

        let timeout = self.asset_cfg.timeout();
        let timeout_secs = timeout.as_secs() as u32;
        let wait_result = match &idle_tracker {
            Some(tracker) => tracker.wait_until_idle(&page, timeout).await,
            None => wait_dom_content_loaded(&page, timeout).await,
        };
        if wait_result.is_err() {
            intercept_task.abort();
            let _ = page.close().await;
            return Err(HeadlessError::Timeout {
                url: url_str,
                timeout_secs,
            });
        }

        let html = page
            .content()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()));
        let final_url = page
            .url()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))
            .map(|opt| {
                opt.and_then(|s| url::Url::parse(&s).ok())
                    .unwrap_or_else(|| url.clone())
            });

        intercept_task.abort();
        let _ = page.close().await;

        let html = html?;
        let final_url = final_url?;

        Ok(RenderedPage {
            final_url,
            html,
            // chromiumoxide doesn't surface the top-level navigation status
            // cleanly without an extra Network-domain dance; treat a
            // successful render as 200. Errors map to `HeadlessError` above.
            status: 200,
        })
    }

    /// Request the browser to exit, then stop the background handler task.
    /// Consumes the renderer.
    ///
    /// Ordering matters: `browser.close()` issues the CDP `Browser.close`
    /// command and waits for the acknowledgement, which only arrives if the
    /// handler task is still pumping the WebSocket. Aborting the handler first
    /// (as we used to) strands the close handshake, so chromiumoxide's
    /// `Browser::drop` later logs `Browser was not closed manually` and the
    /// connection dies with `ResetWithoutClosingHandshake`. Close first, then
    /// abort the now-idle handler.
    pub async fn shutdown(mut self) {
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        self.handler_task.abort();
    }
}

/// A lazily-initialized handle to a [`HeadlessRenderer`].
///
/// Constructing a handle is cheap — it does **not** launch a browser. Chromium
/// is launched on the first [`HeadlessHandle::get`] call and then reused. This
/// lets callers wire a renderer into a fetch unconditionally while paying the
/// browser-launch cost only when a render actually happens (an SPA is detected,
/// a bot-challenge needs bypassing, or `headless.mode = on`). A plain reqwest
/// fetch that never needs the browser launches nothing — and therefore emits
/// none of chromiumoxide's process-teardown noise.
///
/// The inner `OnceCell` is shareable: the one-shot CLI uses a fresh handle per
/// invocation, while the long-running MCP server wraps a single process-shared
/// cell ([`HeadlessHandle::with_cell`]) so one Chromium instance serves every
/// request.
#[derive(Clone)]
pub struct HeadlessHandle {
    cell: Arc<tokio::sync::OnceCell<Arc<HeadlessRenderer>>>,
    cfg: HeadlessConfig,
}

impl std::fmt::Debug for HeadlessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessHandle")
            .field("initialized", &self.cell.get().is_some())
            .finish_non_exhaustive()
    }
}

impl HeadlessHandle {
    /// Create a handle backed by a fresh, empty renderer cell. No browser is
    /// launched until [`HeadlessHandle::get`] is first called.
    pub fn new(cfg: HeadlessConfig) -> Self {
        Self {
            cell: Arc::new(tokio::sync::OnceCell::new()),
            cfg,
        }
    }

    /// Create a handle wrapping an existing shared renderer cell. Used by the
    /// long-running MCP server so a single Chromium instance is reused across
    /// requests for the server's lifetime.
    pub fn with_cell(
        cell: Arc<tokio::sync::OnceCell<Arc<HeadlessRenderer>>>,
        cfg: HeadlessConfig,
    ) -> Self {
        Self { cell, cfg }
    }

    /// Get the renderer, launching the browser on first use. Subsequent calls
    /// return the same instance. Concurrent first-callers race on the
    /// `OnceCell`; the loser reuses the winner's renderer.
    pub async fn get(&self) -> Result<Arc<HeadlessRenderer>, HeadlessError> {
        let cfg = self.cfg.clone();
        self.cell
            .get_or_try_init(|| async move { HeadlessRenderer::new(&cfg).await.map(Arc::new) })
            .await
            .cloned()
    }

    /// Whether the browser has been launched (the cell is populated).
    pub fn is_initialized(&self) -> bool {
        self.cell.get().is_some()
    }

    /// The configured Auto-mode delay to apply before escalating to a headless
    /// render (after the SPA/bot-challenge detection, before launching the
    /// browser). `Duration::ZERO` when disabled.
    pub fn launch_delay(&self) -> std::time::Duration {
        self.cfg.launch_delay()
    }

    /// Cleanly shut the browser down if it was ever launched. Consumes the
    /// handle. A no-op when the browser was never launched (the common case for
    /// non-SPA, unchallenged fetches), so callers can invoke it unconditionally
    /// on every exit path.
    pub async fn shutdown(self) {
        let Self { cell, .. } = self;
        match Arc::try_unwrap(cell) {
            Ok(cell) => {
                if let Some(renderer_arc) = cell.into_inner() {
                    match Arc::try_unwrap(renderer_arc) {
                        Ok(renderer) => renderer.shutdown().await,
                        Err(_still_shared) => {
                            tracing::warn!(
                                target: "rover::fetcher::headless",
                                "headless renderer still has outstanding references at shutdown; skipping explicit close",
                            );
                        }
                    }
                }
            }
            Err(_still_shared) => {
                tracing::warn!(
                    target: "rover::fetcher::headless",
                    "headless handle still shared at shutdown; skipping explicit close",
                );
            }
        }
    }
}

async fn wait_dom_content_loaded(
    page: &chromiumoxide::Page,
    timeout: Duration,
) -> Result<(), HeadlessError> {
    tokio::time::timeout(timeout, page.wait_for_navigation())
        .await
        .map_err(|_| HeadlessError::Cdp("dom_content_loaded timeout".into()))?
        .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

/// Tracks in-flight network requests for a `networkidle0` wait.
///
/// **Must be started before navigation.** chromiumoxide's `event_listener`
/// subscriptions only deliver events emitted after they attach; a tracker
/// created after `page.goto()` races the page's own subresource requests and
/// can miss the very XHR a `networkidle0` wait exists to wait for.
///
/// In-flight requests are tracked by `RequestId` in a set — inserted on
/// `Network.requestWillBeSent`, removed on `loadingFinished`/`loadingFailed`.
/// (`responseReceived` is *not* a completion: the body is still streaming, so
/// counting it would under-count in-flight work and return too early.)
struct NetworkIdleTracker {
    inflight: Arc<StdMutex<HashSet<RequestId>>>,
    tasks: Vec<JoinHandle<()>>,
}

impl NetworkIdleTracker {
    /// Enable the Network domain and begin tracking in-flight requests. Call
    /// this *before* `page.goto()`.
    async fn start(page: &chromiumoxide::Page) -> Result<Self, HeadlessError> {
        page.execute(NetworkEnableParams::default())
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        let inflight: Arc<StdMutex<HashSet<RequestId>>> = Arc::new(StdMutex::new(HashSet::new()));

        let mut will = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
        let mut finished = page
            .event_listener::<EventLoadingFinished>()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
        let mut failed = page
            .event_listener::<EventLoadingFailed>()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        let started = inflight.clone();
        let t_will: JoinHandle<()> = tokio::spawn(async move {
            while let Some(e) = will.next().await {
                started.lock().unwrap().insert(e.request_id.clone());
            }
        });
        let done = inflight.clone();
        let t_finished: JoinHandle<()> = tokio::spawn(async move {
            while let Some(e) = finished.next().await {
                done.lock().unwrap().remove(&e.request_id);
            }
        });
        let errored = inflight.clone();
        let t_failed: JoinHandle<()> = tokio::spawn(async move {
            while let Some(e) = failed.next().await {
                errored.lock().unwrap().remove(&e.request_id);
            }
        });

        Ok(Self {
            inflight,
            tasks: vec![t_will, t_finished, t_failed],
        })
    }

    /// Wait until the page reaches `domcontentloaded` and the network then
    /// settles to zero in-flight requests for a continuous 500 ms (Puppeteer's
    /// `networkidle0`). Captures content loaded by a post-load XHR — the common
    /// SPA pattern — which the looser `networkidle2` (≤2 in-flight) would skip,
    /// since a single pending request already counts as "idle". Bounded by
    /// `timeout`, so a page holding a persistent connection
    /// (websocket/SSE/analytics beacon) returns at the timeout rather than
    /// hanging.
    async fn wait_until_idle(
        &self,
        page: &chromiumoxide::Page,
        timeout: Duration,
    ) -> Result<(), HeadlessError> {
        use std::time::Instant;

        let poll = self.inflight.clone();
        let waited = tokio::time::timeout(timeout, async {
            // Reach domcontentloaded first so the initial document's
            // subresource requests have a chance to start before we judge the
            // page "quiet".
            page.wait_for_navigation()
                .await
                .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
            let mut quiet_since: Option<Instant> = None;
            loop {
                let n = poll.lock().unwrap().len();
                if n == 0 {
                    let since = *quiet_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_millis(500) {
                        return Ok::<(), HeadlessError>(());
                    }
                } else {
                    quiet_since = None;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        match waited {
            Ok(inner) => inner,
            // The caller maps any Err here into HeadlessError::Timeout with the
            // url + configured timeout_secs.
            Err(_elapsed) => Err(HeadlessError::Cdp("network idle0 wait timed out".into())),
        }
    }
}

impl Drop for NetworkIdleTracker {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_mode_as_str_round_trips() {
        assert_eq!(HeadlessMode::Off.as_str(), "off");
        assert_eq!(HeadlessMode::On.as_str(), "on");
        assert_eq!(HeadlessMode::Auto.as_str(), "auto");
    }
}

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

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::fetch::{
    EnableParams as FetchEnableParams, EventRequestPaused,
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
}

impl HeadlessRenderer {
    /// Launch the browser and prepare the renderer. The returned value owns
    /// a background tokio task that drives chromiumoxide's event loop; call
    /// [`HeadlessRenderer::shutdown`] to stop it cleanly.
    pub async fn new(cfg: &HeadlessConfig) -> Result<Self, HeadlessError> {
        let (browser, handler_task) = browser::launch(cfg).await?;
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
        let wait_result = match self.asset_cfg.default_wait.as_str() {
            "networkidle2" => wait_network_idle2(&page, timeout).await,
            _ => wait_dom_content_loaded(&page, timeout).await,
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

    /// Stop the background handler task and request the browser to exit.
    /// Consumes the renderer.
    pub async fn shutdown(mut self) {
        self.handler_task.abort();
        let _ = self.browser.close().await;
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

async fn wait_network_idle2(
    page: &chromiumoxide::Page,
    timeout: Duration,
) -> Result<(), HeadlessError> {
    // v1 approximation: wait for domcontentloaded then sleep 500ms. The
    // chromiumoxide 0.9 API does not expose a built-in `networkidle2`
    // helper; open item §7.3 #5 tracks the proper implementation against
    // Network domain in-flight counts.
    wait_dom_content_loaded(page, timeout).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
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

//! Browser launch helpers for the headless renderer.
//!
//! `BrowserConfig::default()` auto-detects an installed Chrome/Chromium on
//! Linux/macOS/Windows (PATH lookup + standard install paths). The
//! `chrome_executable` config key overrides that path explicitly.

use chromiumoxide::browser::{Browser, BrowserConfig, BrowserConfigBuilder};
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::config::HeadlessConfig;
use crate::fetcher::headless::HeadlessError;

/// Build a `BrowserConfig` from the Rover headless config block.
pub fn build_browser_config(cfg: &HeadlessConfig) -> Result<BrowserConfig, HeadlessError> {
    let mut builder: BrowserConfigBuilder = BrowserConfig::builder();
    if !cfg.chrome_executable.is_empty() {
        builder = builder.chrome_executable(&cfg.chrome_executable);
    }
    builder = builder.enable_request_intercept();
    builder
        .build()
        .map_err(|e| HeadlessError::ConfigInvalid(e.to_string()))
}

/// Launch the browser and spawn the background handler task. The handler
/// task drives `chromiumoxide::Browser`'s event loop for the browser's
/// lifetime. Returns `(Browser, JoinHandle)` — callers must `abort()` the
/// handle on shutdown.
pub async fn launch(cfg: &HeadlessConfig) -> Result<(Browser, JoinHandle<()>), HeadlessError> {
    let bc = build_browser_config(cfg)?;
    let (browser, mut handler) = Browser::launch(bc)
        .await
        .map_err(|e| HeadlessError::LaunchFailed(e.to_string()))?;
    let task = tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // The handler returns Result<(), ...> events; we drop them.
            // chromiumoxide internally dispatches them to the page.
        }
    });
    Ok((browser, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_with_empty_chrome_executable_uses_default_detection() {
        let cfg = HeadlessConfig {
            chrome_executable: String::new(),
            ..HeadlessConfig::default()
        };
        let bc = build_browser_config(&cfg);
        assert!(
            bc.is_ok(),
            "config builds even without chrome installed; launch is the failing step"
        );
    }
}

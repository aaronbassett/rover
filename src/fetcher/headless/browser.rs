//! Browser launch helpers for the headless renderer.
//!
//! `BrowserConfig::default()` auto-detects an installed Chrome/Chromium on
//! Linux/macOS/Windows (PATH lookup + standard install paths). The
//! `chrome_executable` config key overrides that path explicitly.

use std::path::Path;

use chromiumoxide::browser::{Browser, BrowserConfig, BrowserConfigBuilder};
use futures::StreamExt;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use crate::config::HeadlessConfig;
use crate::fetcher::headless::HeadlessError;

/// Build a `BrowserConfig` from the Rover headless config block.
///
/// `profile_dir` is the throwaway Chrome user-data directory for this
/// instance. It must be unique per launch: without an explicit
/// `user_data_dir`, chromiumoxide points every browser at a single shared
/// `<tmp>/chromiumoxide-runner` profile, and Chrome's `ProcessSingleton`
/// then refuses to start a second instance against the same profile
/// (`Failed to create .../SingletonLock: File exists`). That aborts every
/// renderer but the first whenever two launch concurrently — e.g. the
/// parallel headless smoketests, or concurrent renders in one process.
pub fn build_browser_config(
    cfg: &HeadlessConfig,
    profile_dir: &Path,
) -> Result<BrowserConfig, HeadlessError> {
    let mut builder: BrowserConfigBuilder = BrowserConfig::builder();
    if !cfg.chrome_executable.is_empty() {
        builder = builder.chrome_executable(&cfg.chrome_executable);
    }
    // Use Chrome's *new* headless mode (`--headless=new`). chromiumoxide
    // defaults to the legacy `--headless`, which is both deprecated (removed in
    // recent Chrome) and far more readily flagged by bot-mitigation services
    // (e.g. Vercel/Cloudflare managed challenges). New headless is a real
    // browser surface, so it executes JS challenges and renders SPAs the way a
    // user's browser would — which is exactly what the challenge-bypass path
    // relies on.
    builder = builder.new_headless_mode();
    builder = builder.enable_request_intercept();
    builder = builder.user_data_dir(profile_dir);
    builder
        .build()
        .map_err(|e| HeadlessError::ConfigInvalid(e.to_string()))
}

/// Launch the browser and spawn the background handler task. The handler
/// task drives `chromiumoxide::Browser`'s event loop for the browser's
/// lifetime. Returns `(Browser, JoinHandle, TempDir)` — callers must
/// `abort()` the handle on shutdown and keep the `TempDir` alive for the
/// browser's lifetime (it is the per-instance profile directory and is
/// removed when dropped).
pub async fn launch(
    cfg: &HeadlessConfig,
) -> Result<(Browser, JoinHandle<()>, TempDir), HeadlessError> {
    let profile_dir = tempfile::Builder::new()
        .prefix("rover-headless-")
        .tempdir()
        .map_err(|e| {
            HeadlessError::LaunchFailed(format!("could not create browser profile dir: {e}"))
        })?;
    let bc = build_browser_config(cfg, profile_dir.path())?;
    let (browser, mut handler) = Browser::launch(bc)
        .await
        .map_err(|e| HeadlessError::LaunchFailed(e.to_string()))?;
    let task = tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // The handler returns Result<(), ...> events; we drop them.
            // chromiumoxide internally dispatches them to the page.
        }
    });
    Ok((browser, task, profile_dir))
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
        let profile_dir = tempfile::tempdir().expect("tempdir");
        let bc = build_browser_config(&cfg, profile_dir.path());
        assert!(
            bc.is_ok(),
            "config builds even without chrome installed; launch is the failing step"
        );
    }
}

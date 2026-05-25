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

// The renderer struct itself ships in Task 30/34.

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

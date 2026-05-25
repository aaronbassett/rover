//! Image captioning subsystem.
//!
//! Exposes a `VlmCaptioner` trait (Task 3) with two implementations:
//! - `CloudCaptioner` (Task 6) — always compiled, wraps `genai::Client` for
//!   vision-capable cloud models (OpenAI gpt-4o, Anthropic Claude with vision,
//!   Gemini, ...).
//! - `MistralRsCaptioner` (Task 23) — gated by the `local-vision` feature,
//!   wraps `mistralrs` for local SmolVLM inference.
//!
//! The `CaptionerRegistry` (Task 5) holds the configured captioners and is
//! injected into MCP server state (Task 10).
//!
//! Caption results are deterministically cached in `summary_cache` via the
//! `cache` module (Task 7), keyed on `(sha256(image_bytes), captioner_name,
//! captioner_model_id, max_tokens)`.

pub mod cache;
pub mod cloud;
pub mod error;
pub mod prompts;

#[cfg(feature = "local-vision")]
pub mod local;

pub use error::VlmError;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// One captioning request after defaults have been merged.
///
/// The trait is intentionally narrow: callers pass image bytes (already
/// fetched, already filtered against the dimension/size gates), optional
/// `alt` text as a hint, and a token budget. Implementations are free to
/// add their own internal concurrency caps (semaphores) — the caller does
/// not need to know.
#[async_trait]
pub trait VlmCaptioner: Send + Sync {
    /// Config-key name (e.g. "openai", "local").
    fn name(&self) -> &str;

    /// Resolved model identifier for cache partitioning.
    fn model_id(&self) -> &str;

    /// Generate a single short caption for the image. On success, returns
    /// the caption string with leading/trailing whitespace trimmed.
    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError>;
}

/// Frozen registry of captioners. Construction in `build()` (Task 5).
#[derive(Clone)]
pub struct CaptionerRegistry {
    captioners: HashMap<String, Arc<dyn VlmCaptioner>>,
    default: Option<String>,
}

impl std::fmt::Debug for CaptionerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.captioners.keys().collect();
        names.sort();
        f.debug_struct("CaptionerRegistry")
            .field("captioners", &names)
            .field("default", &self.default)
            .finish()
    }
}

impl CaptionerRegistry {
    pub fn empty() -> Self {
        Self {
            captioners: HashMap::new(),
            default: None,
        }
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn VlmCaptioner>, VlmError> {
        self.captioners
            .get(name)
            .cloned()
            .ok_or_else(|| VlmError::NoSuchCaptioner {
                name: name.to_string(),
            })
    }

    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.captioners.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.captioners.is_empty()
    }

    /// Test-only direct construction.
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-loopback"))]
    pub fn __test_construct(
        captioners: HashMap<String, Arc<dyn VlmCaptioner>>,
        default: Option<String>,
    ) -> Self {
        Self {
            captioners,
            default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_returns_no_default() {
        let r = CaptionerRegistry::empty();
        assert!(r.is_empty());
        assert!(r.default_name().is_none());
    }

    #[test]
    fn unknown_captioner_returns_typed_error() {
        let r = CaptionerRegistry::empty();
        match r.get("missing") {
            Err(VlmError::NoSuchCaptioner { name }) => assert_eq!(name, "missing"),
            _ => panic!("expected NoSuchCaptioner"),
        }
    }
}

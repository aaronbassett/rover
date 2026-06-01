//! Image captioning subsystem.
//!
//! Exposes a `VlmCaptioner` trait (Task 3). The sole implementation is
//! `CloudCaptioner` (Task 6) — always compiled, wraps `genai::Client` for
//! vision-capable cloud models (OpenAI gpt-4o, Anthropic Claude with vision,
//! Gemini, ...) and any OpenAI-compatible server (ollama, LM Studio, ...) via
//! `provider = "openai_compat"`. The former `local-vision` mistralrs path
//! (SmolVLM) was removed; see git history to restore it.
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

pub use error::VlmError;

use crate::config::{CaptionerConfig, Config};
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

/// Build a `CaptionerRegistry` from config. Validation:
/// 1. Every `[captioners.<name>]` parses into a concrete captioner.
/// 2. `[image_captions] default` (if set) must refer to a configured captioner.
/// 3. If unset and exactly one captioner exists, that one becomes the default.
/// 4. `kind = "local"` is no longer supported (local mistralrs captioning was
///    removed) → `LocalFeatureNotCompiled`. Use `kind = "cloud"` instead.
///
/// An empty config (no `[captioners.*]` blocks) returns an empty registry —
/// `ImagesMode::Caption` calls will then error at fetch time with
/// `NoCaptionersConfigured`. Lazy by design: a user who never opts into
/// captioning pays no startup cost.
pub fn build(config: &Config) -> Result<CaptionerRegistry, VlmError> {
    let mut captioners: HashMap<String, Arc<dyn VlmCaptioner>> = HashMap::new();
    for (name, cfg) in &config.captioners {
        let c = build_one(name, cfg, &config.image_captions)?;
        captioners.insert(name.clone(), c);
    }

    let default = match &config.image_captions.default {
        Some(d) => {
            if !captioners.contains_key(d) {
                return Err(VlmError::NoSuchCaptioner { name: d.clone() });
            }
            Some(d.clone())
        }
        None => {
            if captioners.len() == 1 {
                captioners.keys().next().cloned()
            } else {
                None
            }
        }
    };

    Ok(CaptionerRegistry {
        captioners,
        default,
    })
}

fn build_one(
    name: &str,
    cfg: &CaptionerConfig,
    _ic: &crate::config::ImageCaptionsConfig,
) -> Result<Arc<dyn VlmCaptioner>, VlmError> {
    match cfg.kind.as_str() {
        "cloud" => {
            let provider = cfg
                .provider
                .as_deref()
                .ok_or_else(|| VlmError::Unavailable {
                    name: name.to_string(),
                    reason: "cloud captioner requires `provider`".into(),
                })?;
            let model = cfg.model.as_deref().ok_or_else(|| VlmError::Unavailable {
                name: name.to_string(),
                reason: "cloud captioner requires `model`".into(),
            })?;
            let provider_kind =
                crate::summarizer::cloud::ProviderKind::parse(provider).map_err(|reason| {
                    VlmError::Unavailable {
                        name: name.to_string(),
                        reason,
                    }
                })?;
            let api_key = cfg
                .api_key_env
                .as_deref()
                .and_then(|var| std::env::var(var).ok())
                .filter(|v| !v.is_empty());
            let base_url = cfg.base_url.clone();
            Ok(Arc::new(cloud::CloudCaptioner::new(
                name,
                provider_kind,
                model,
                base_url,
                api_key,
            )?))
        }
        // Local mistralrs captioning was removed (the SmolVLM/idefics3 path was
        // broken on the CPU backend). `LocalFeatureNotCompiled` now carries a
        // message pointing users at a cloud captioner (incl. local
        // OpenAI-compatible servers via `provider = "openai_compat"`).
        "local" => Err(VlmError::LocalFeatureNotCompiled),
        other => Err(VlmError::Unavailable {
            name: name.to_string(),
            reason: format!("unknown captioner kind: {other}"),
        }),
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

    #[test]
    fn build_with_no_captioners_returns_empty_registry() {
        let cfg = crate::config::Config::default();
        let r = build(&cfg).unwrap();
        assert!(r.is_empty());
        assert!(r.default_name().is_none());
    }

    #[test]
    fn build_with_cloud_captioner_succeeds() {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert(
            "openai".to_string(),
            crate::config::CaptionerConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                api_key_env: Some("OPENAI_API_KEY".into()),
                base_url: None,
            },
        );
        cfg.image_captions.default = Some("openai".into());
        let r = build(&cfg).unwrap();
        assert!(r.get("openai").is_ok());
        assert_eq!(r.default_name(), Some("openai"));
    }

    #[test]
    fn build_with_default_pointing_at_missing_captioner_errors() {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert(
            "openai".to_string(),
            crate::config::CaptionerConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                api_key_env: Some("OPENAI_API_KEY".into()),
                base_url: None,
            },
        );
        cfg.image_captions.default = Some("nonsense".into());
        let err = build(&cfg).unwrap_err();
        assert!(matches!(err, VlmError::NoSuchCaptioner { name } if name == "nonsense"));
    }

    #[test]
    fn build_with_local_kind_errors_after_removal() {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert(
            "local".to_string(),
            crate::config::CaptionerConfig {
                kind: "local".into(),
                provider: None,
                model: Some("any-model".into()),
                api_key_env: None,
                base_url: None,
            },
        );
        let err = build(&cfg).unwrap_err();
        assert!(matches!(err, VlmError::LocalFeatureNotCompiled));
    }

    #[test]
    fn build_unknown_kind_errors() {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert(
            "weird".to_string(),
            crate::config::CaptionerConfig {
                kind: "weird".into(),
                ..Default::default()
            },
        );
        let err = build(&cfg).unwrap_err();
        assert!(matches!(err, VlmError::Unavailable { .. }));
    }

    #[test]
    fn build_default_inferred_when_only_one_captioner_and_no_default_set() {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert(
            "openai".to_string(),
            crate::config::CaptionerConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                api_key_env: Some("OPENAI_API_KEY".into()),
                base_url: None,
            },
        );
        // image_captions.default left as None.
        let r = build(&cfg).unwrap();
        assert_eq!(r.default_name(), Some("openai"));
    }
}

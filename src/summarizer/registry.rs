//! Backend registry construction.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::config::{BackendConfig, Config};
use crate::summarizer::backend::SummarizerBackend;
use crate::summarizer::cloud::{CloudBackend, ProviderKind};
use crate::summarizer::error::SummarizerError;
use crate::summarizer::extractive::ExtractiveBackend;
use crate::tokenizer::Tokenizer;

/// Frozen registry of summarizer backends.
#[derive(Clone)]
pub struct SummarizerRegistry {
    backends: HashMap<String, Arc<dyn SummarizerBackend>>,
    default_backend: String,
    extractive_fallback: Option<String>,
}

impl fmt::Debug for SummarizerRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<&String> = self.backends.keys().collect();
        names.sort();
        f.debug_struct("SummarizerRegistry")
            .field("backends", &names)
            .field("default_backend", &self.default_backend)
            .field("extractive_fallback", &self.extractive_fallback)
            .finish()
    }
}

impl SummarizerRegistry {
    // Consumed in Task 7 (SummarizerService).
    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Result<Arc<dyn SummarizerBackend>, SummarizerError> {
        self.backends
            .get(name)
            .cloned()
            .ok_or_else(|| SummarizerError::NoSuchBackend {
                name: name.to_string(),
            })
    }

    // Consumed in Task 7 (SummarizerService).
    #[allow(dead_code)]
    pub fn default_backend_name(&self) -> &str {
        &self.default_backend
    }

    // Consumed in Task 7 (SummarizerService).
    #[allow(dead_code)]
    pub fn extractive_fallback_name(&self) -> Option<&str> {
        self.extractive_fallback.as_deref()
    }

    // Consumed in Task 7 (SummarizerService).
    #[allow(dead_code)]
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.backends.keys().map(String::as_str)
    }

    /// Direct-construction helper for sibling-module unit tests. Skips
    /// the validation in `build`; tests are responsible for passing a
    /// coherent set of backends. Consumed in Task 7's `service_tests`.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn __test_construct(
        backends: HashMap<String, Arc<dyn SummarizerBackend>>,
        default_backend: String,
        extractive_fallback: Option<String>,
    ) -> Self {
        Self {
            backends,
            default_backend,
            extractive_fallback,
        }
    }
}

/// Build a registry from a config + tokenizer family.
///
/// Validation:
/// 1. Every `[backends.<name>]` parses into a concrete backend.
/// 2. `summarization.default_backend` refers to one of those names.
/// 3. If `summarization.fallback_to_extractive == true`, at least one
///    extractive backend exists (any name).
///
/// If `config.backends` is empty entirely, the registry installs an
/// implicit `default` extractive backend so a fresh install works
/// offline without any configuration. This is the only case where we
/// silently inject — once a user adds any `[backends.*]` block, the
/// validation rules apply strictly.
// Consumed in Task 8 (server.rs / main.rs wiring).
#[allow(dead_code)]
pub fn build(config: &Config, tokenizer: Tokenizer) -> Result<SummarizerRegistry, SummarizerError> {
    let mut backends: HashMap<String, Arc<dyn SummarizerBackend>> = HashMap::new();

    if config.backends.is_empty() {
        backends.insert(
            "default".to_string(),
            Arc::new(ExtractiveBackend::new("default", tokenizer)),
        );
    } else {
        for (name, cfg) in &config.backends {
            let b = build_one(name, cfg, tokenizer)?;
            backends.insert(name.clone(), b);
        }
    }

    let default_backend = config.summarization.default_backend.clone();
    if !backends.contains_key(&default_backend) {
        return Err(SummarizerError::NoSuchBackend {
            name: default_backend,
        });
    }

    let extractive_fallback = find_extractive_fallback(&backends);
    if config.summarization.fallback_to_extractive && extractive_fallback.is_none() {
        return Err(SummarizerError::NoExtractiveBackendForFallback);
    }

    Ok(SummarizerRegistry {
        backends,
        default_backend,
        extractive_fallback,
    })
}

fn build_one(
    name: &str,
    cfg: &BackendConfig,
    tokenizer: Tokenizer,
) -> Result<Arc<dyn SummarizerBackend>, SummarizerError> {
    match cfg.kind.as_str() {
        "extractive" => Ok(Arc::new(ExtractiveBackend::new(name, tokenizer))),
        "cloud" => {
            let provider =
                cfg.provider
                    .as_deref()
                    .ok_or_else(|| SummarizerError::BackendUnavailable {
                        name: name.to_string(),
                        reason: "cloud backend requires `provider`".into(),
                    })?;
            let model =
                cfg.model
                    .as_deref()
                    .ok_or_else(|| SummarizerError::BackendUnavailable {
                        name: name.to_string(),
                        reason: "cloud backend requires `model`".into(),
                    })?;
            let provider_kind = ProviderKind::parse(provider).map_err(|reason| {
                SummarizerError::BackendUnavailable {
                    name: name.to_string(),
                    reason,
                }
            })?;
            let api_key = cfg
                .api_key_env
                .as_deref()
                .and_then(|var| std::env::var(var).ok());
            let be = CloudBackend::new(name, provider_kind, model, cfg.base_url.clone(), api_key)
                .map_err(|e| SummarizerError::BackendUnavailable {
                name: name.to_string(),
                reason: e.to_string(),
            })?;
            Ok(Arc::new(be))
        }
        other => Err(SummarizerError::BackendUnavailable {
            name: name.to_string(),
            reason: format!("unknown backend kind: {other}"),
        }),
    }
}

fn find_extractive_fallback(
    backends: &HashMap<String, Arc<dyn SummarizerBackend>>,
) -> Option<String> {
    // Prefer "default" if it's an extractive backend; otherwise the first
    // extractive backend by name lex order for determinism.
    let mut names: Vec<&String> = backends.keys().collect();
    names.sort();
    for n in &names {
        // model_id == "" is the convention for extractive backends.
        if backends[*n].model_id().is_empty() && *n == "default" {
            return Some((*n).clone());
        }
    }
    for n in names {
        if backends[n].model_id().is_empty() {
            return Some(n.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SummarizationConfig;

    fn cfg_with_backends(map: &[(&str, BackendConfig)]) -> Config {
        Config {
            summarization: SummarizationConfig::default(),
            backends: map
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
            ..Config::default()
        }
    }

    #[test]
    fn empty_backends_installs_implicit_extractive_default() {
        let cfg = Config::default();
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
        assert_eq!(reg.default_backend_name(), "default");
        assert_eq!(reg.extractive_fallback_name(), Some("default"));
    }

    #[test]
    fn explicit_extractive_backend_builds() {
        let cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "extractive".into(),
                ..Default::default()
            },
        )]);
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
    }

    #[test]
    fn default_backend_missing_errors() {
        let mut cfg = cfg_with_backends(&[(
            "alt",
            BackendConfig {
                kind: "extractive".into(),
                ..Default::default()
            },
        )]);
        cfg.summarization.default_backend = "missing".into();
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(r, Err(SummarizerError::NoSuchBackend { .. })));
    }

    #[test]
    fn cloud_backend_requires_provider_and_model() {
        let cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: None,
                model: None,
                base_url: None,
                api_key_env: None,
            },
        )]);
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(r, Err(SummarizerError::BackendUnavailable { .. })));
    }

    #[test]
    fn fallback_disabled_allows_cloud_only_registry() {
        let mut cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                base_url: None,
                api_key_env: None,
            },
        )]);
        cfg.summarization.fallback_to_extractive = false;
        let reg = build(&cfg, Tokenizer::O200k).unwrap();
        assert!(reg.get("default").is_ok());
        assert!(reg.extractive_fallback_name().is_none());
    }

    #[test]
    fn fallback_enabled_requires_extractive_backend() {
        let mut cfg = cfg_with_backends(&[(
            "default",
            BackendConfig {
                kind: "cloud".into(),
                provider: Some("openai".into()),
                model: Some("gpt-4o-mini".into()),
                base_url: None,
                api_key_env: None,
            },
        )]);
        cfg.summarization.fallback_to_extractive = true;
        let r = build(&cfg, Tokenizer::O200k);
        assert!(matches!(
            r,
            Err(SummarizerError::NoExtractiveBackendForFallback)
        ));
    }
}

//! Summarization subsystem.
//!
//! Exposes a `SummarizerBackend` trait and three concrete impls — `Extractive`
//! (TextRank, offline), `Cloud` (wraps `genai::Client`), and (M9-future)
//! `LocalMistralRs`. The `SummarizerService` (Task 7) wraps a `Registry`
//! (Task 6) plus the storage handle and owns the cache hot path.

pub mod backend;
pub mod cloud;
pub mod error;
pub mod extractive;
pub mod prompts;
pub mod registry;

pub use backend::{CompactMode, CompactOpts, PreserveSection, Style, SummarizerBackend};
pub use error::{BackendError, SummarizerError};

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::storage::Db;
use crate::storage::summaries;
use crate::summarizer::registry::SummarizerRegistry;

/// Deterministic params_hash for `summary_cache` lookups. Inputs are
/// serialized as plain strings — never via serde — so reorderings or
/// crate version changes can't shift the hash. Length-prefix framing
/// (`{byte_len}:{content}`) makes the format unambiguous regardless of
/// whether any field contains delimiter-like bytes.
pub fn params_hash(opts: &CompactOpts, model_id: &str) -> String {
    let target = opts
        .target_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_string());
    let focus = opts
        .focus
        .as_deref()
        .map(|s| s.trim())
        .unwrap_or("")
        .to_string();
    let mut preserve_sorted: Vec<&'static str> = opts.preserve.iter().map(|p| p.as_str()).collect();
    preserve_sorted.sort();
    preserve_sorted.dedup();
    let preserve_csv = preserve_sorted.join(",");

    let mut serialized = String::new();
    for s in [
        opts.backend_name.as_str(),
        model_id,
        opts.mode.as_str(),
        target.as_str(),
        focus.as_str(),
        preserve_csv.as_str(),
        opts.style.as_str(),
    ] {
        serialized.push_str(&format!("{}:{}", s.len(), s));
    }

    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    let bytes = h.finalize();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        write!(hex, "{b:02x}").expect("write to string never fails");
    }
    hex
}

/// Outcome of a `SummarizerService::compact` call. Carries enough context
/// for the MCP tool to render the response envelope (cache_status,
/// fallback metadata).
#[derive(Debug, Clone)]
pub struct SummaryResult {
    pub summary_md: String,
    pub cache_status: SummaryCacheStatus,
    pub effective_backend: String,
    pub effective_model_id: String,
    pub fallback: Option<FallbackInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryCacheStatus {
    Hit,
    Miss,
}

#[derive(Debug, Clone)]
pub struct FallbackInfo {
    pub from: String,
    pub reason: &'static str,
}

/// Service over the registry + storage. Cheap to clone; both fields are
/// `Arc`.
#[derive(Debug, Clone)]
pub struct SummarizerService {
    db: Db,
    registry: Arc<SummarizerRegistry>,
    fallback_to_extractive: bool,
}

impl SummarizerService {
    pub fn new(db: Db, registry: Arc<SummarizerRegistry>, fallback_to_extractive: bool) -> Self {
        Self {
            db,
            registry,
            fallback_to_extractive,
        }
    }

    // Consumed in Task 8 (MCP wiring) and beyond.
    pub fn registry(&self) -> &SummarizerRegistry {
        &self.registry
    }

    /// Compact `content` per `opts`. `content_hash` is the cache key —
    /// the caller decides what it represents (extracted_md hash, table
    /// hash, etc.). Defaults for `opts.backend_name` are resolved by
    /// the registry's `default_backend_name()` *before* calling — the
    /// service trusts whatever name is in the opts.
    // Consumed in Task 8 (MCP wiring) and beyond. Tests exercise it.
    pub async fn compact(
        &self,
        content_hash: &str,
        content: &str,
        opts: &CompactOpts,
    ) -> Result<SummaryResult, SummarizerError> {
        let backend = self.registry.get(&opts.backend_name)?;
        let model_id = backend.model_id().to_string();
        let ph = params_hash(opts, &model_id);

        // Cache lookup.
        if let Some(row) = summaries::lookup(&self.db, content_hash, &ph).await? {
            return Ok(SummaryResult {
                summary_md: row.summary_md,
                cache_status: SummaryCacheStatus::Hit,
                effective_backend: opts.backend_name.clone(),
                effective_model_id: model_id,
                fallback: None,
            });
        }

        // Miss: dispatch.
        match backend.compact(content, opts).await {
            Ok(md) => {
                summaries::insert(&self.db, content_hash, &ph, &md).await?;
                Ok(SummaryResult {
                    summary_md: md,
                    cache_status: SummaryCacheStatus::Miss,
                    effective_backend: opts.backend_name.clone(),
                    effective_model_id: model_id,
                    fallback: None,
                })
            }
            Err(orig_err) => {
                let translated = SummarizerError::from_backend(&opts.backend_name, orig_err);
                if !self.fallback_to_extractive {
                    return Err(translated);
                }
                let Some(fb_name) = self.registry.extractive_fallback_name() else {
                    return Err(translated);
                };
                if fb_name == opts.backend_name {
                    // Already extractive; nothing to fall back to.
                    return Err(translated);
                }
                let fb_name = fb_name.to_string();
                // Build the fallback opts: same shape, swapped backend name.
                let mut fb_opts = opts.clone();
                fb_opts.backend_name = fb_name.clone();
                // Force the prompt-free path: extractive backend ignores
                // mode=Abstractive but produces sensible output.
                if fb_opts.mode == CompactMode::Abstractive {
                    fb_opts.mode = CompactMode::Extractive;
                }
                let fb_backend = self.registry.get(&fb_name)?;
                let fb_model = fb_backend.model_id().to_string();
                let fb_params = params_hash(&fb_opts, &fb_model);
                if let Some(row) = summaries::lookup(&self.db, content_hash, &fb_params).await? {
                    return Ok(SummaryResult {
                        summary_md: row.summary_md,
                        cache_status: SummaryCacheStatus::Hit,
                        effective_backend: fb_name.clone(),
                        effective_model_id: fb_model,
                        fallback: Some(FallbackInfo {
                            from: opts.backend_name.clone(),
                            reason: translated.fallback_reason(),
                        }),
                    });
                }
                let md = fb_backend
                    .compact(content, &fb_opts)
                    .await
                    .map_err(|e| SummarizerError::from_backend(&fb_name, e))?;
                summaries::insert(&self.db, content_hash, &fb_params, &md).await?;
                Ok(SummaryResult {
                    summary_md: md,
                    cache_status: SummaryCacheStatus::Miss,
                    effective_backend: fb_name.clone(),
                    effective_model_id: fb_model,
                    fallback: Some(FallbackInfo {
                        from: opts.backend_name.clone(),
                        reason: translated.fallback_reason(),
                    }),
                })
            }
        }
    }

    /// Convenience: build opts using `[summarization]` defaults for
    /// unset fields. Returns the opts plus the resolved backend name
    /// (in case the caller wants to log it).
    // Consumed in Task 9 (compact_content MCP tool).
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_defaults(
        &self,
        mode: Option<CompactMode>,
        style: Option<Style>,
        target_tokens: Option<usize>,
        focus: Option<String>,
        preserve: Vec<PreserveSection>,
        backend: Option<String>,
        defaults: &DefaultsHint,
    ) -> CompactOpts {
        CompactOpts {
            mode: mode.unwrap_or(defaults.mode),
            style: style.unwrap_or(defaults.style),
            target_tokens,
            focus,
            preserve,
            backend_name: backend.unwrap_or_else(|| defaults.backend.clone()),
        }
    }
}

/// Compact form of `[summarization]` defaults so callers don't have to
/// carry the whole `Config` reference.
#[derive(Debug, Clone)]
pub struct DefaultsHint {
    pub backend: String,
    pub mode: CompactMode,
    pub style: Style,
}

impl DefaultsHint {
    /// Parse string-typed values from `SummarizationConfig`. Unknown
    /// strings fall back to `Abstractive`/`Prose` with a warning logged.
    // Consumed in Task 9 (compact_content MCP tool).
    pub fn from_config(c: &crate::config::SummarizationConfig) -> Self {
        let mode = match c.default_mode.as_str() {
            "extractive" => CompactMode::Extractive,
            "abstractive" => CompactMode::Abstractive,
            "headlines" => CompactMode::Headlines,
            other => {
                tracing::warn!(
                    target: "rover::summarizer",
                    value = other,
                    "unknown summarization.default_mode; falling back to abstractive",
                );
                CompactMode::Abstractive
            }
        };
        let style = match c.default_style.as_str() {
            "bullet" => Style::Bullet,
            "prose" => Style::Prose,
            "executive" => Style::Executive,
            other => {
                tracing::warn!(
                    target: "rover::summarizer",
                    value = other,
                    "unknown summarization.default_style; falling back to prose",
                );
                Style::Prose
            }
        };
        Self {
            backend: c.default_backend.clone(),
            mode,
            style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(500),
            focus: Some("api shape".to_string()),
            preserve: vec![PreserveSection::Code, PreserveSection::Tables],
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn hash_is_deterministic_for_same_inputs() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o-mini");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn hash_changes_when_backend_name_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let mut other = baseline();
        other.backend_name = "smart".to_string();
        let b = params_hash(&other, "gpt-4o-mini");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_model_id_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_invariant_to_preserve_ordering() {
        let mut a_opts = baseline();
        a_opts.preserve = vec![PreserveSection::Code, PreserveSection::Tables];
        let mut b_opts = baseline();
        b_opts.preserve = vec![PreserveSection::Tables, PreserveSection::Code];
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_treats_target_none_as_null_string() {
        let mut o = baseline();
        o.target_tokens = None;
        let h_none = params_hash(&o, "m");
        o.target_tokens = Some(500);
        let h_some = params_hash(&o, "m");
        assert_ne!(h_none, h_some);
    }

    #[test]
    fn focus_whitespace_normalization_collapses_to_same_hash() {
        let mut a_opts = baseline();
        a_opts.focus = Some("api shape".to_string());
        let mut b_opts = baseline();
        b_opts.focus = Some("  api shape  ".to_string());
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_resists_focus_delimiter_injection() {
        // Two distinct inputs must NOT collide even if focus contains
        // characters that resemble the framing.
        let mut a_opts = baseline();
        a_opts.focus = Some("a:b".to_string());
        a_opts.preserve = vec![];
        let mut b_opts = baseline();
        b_opts.focus = Some("a".to_string());
        b_opts.preserve = vec![PreserveSection::Code]; // arbitrary distinct value
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_ne!(a, b);

        // And U+001E specifically (the old separator) must not collide either.
        let mut c_opts = baseline();
        c_opts.focus = Some("a\u{1E}b".to_string());
        c_opts.preserve = vec![];
        let mut d_opts = baseline();
        d_opts.focus = Some("a".to_string());
        d_opts.preserve = vec![];
        let c = params_hash(&c_opts, "m");
        let d = params_hash(&d_opts, "m");
        assert_ne!(c, d);
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::summarizer::registry::SummarizerRegistry;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Recording backend whose call count and forced-error mode the
    /// service tests inspect.
    struct RecordingBackend {
        name: String,
        model: String,
        calls: Arc<AtomicUsize>,
        fail: Option<BackendError>,
    }

    impl std::fmt::Debug for RecordingBackend {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RecordingBackend")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl SummarizerBackend for RecordingBackend {
        async fn compact(&self, _: &str, _: &CompactOpts) -> Result<String, BackendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = &self.fail {
                Err(match e {
                    BackendError::Unavailable(s) => BackendError::Unavailable(s.clone()),
                    BackendError::RateLimited => BackendError::RateLimited,
                    BackendError::AuthFailed(s) => BackendError::AuthFailed(s.clone()),
                    BackendError::ModelError(s) => BackendError::ModelError(s.clone()),
                    BackendError::Invalid(s) => BackendError::Invalid(s.clone()),
                })
            } else {
                Ok(format!("(from {})", self.name))
            }
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn model_id(&self) -> &str {
            &self.model
        }
    }

    async fn make_db() -> (Db, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rover.db");
        (Db::open(&path).await.unwrap(), tmp)
    }

    fn registry_with(
        backends: Vec<(&str, &str, Option<BackendError>)>,
        default_name: &str,
    ) -> Arc<SummarizerRegistry> {
        // Build a tiny registry by directly constructing the internal map.
        let mut map: std::collections::HashMap<String, Arc<dyn SummarizerBackend>> =
            Default::default();
        for (n, model, fail) in backends {
            map.insert(
                n.to_string(),
                Arc::new(RecordingBackend {
                    name: n.to_string(),
                    model: model.to_string(),
                    calls: Arc::new(AtomicUsize::new(0)),
                    fail,
                }),
            );
        }
        let extractive = map
            .iter()
            .find(|(_, b)| b.model_id().is_empty())
            .map(|(n, _)| n.clone());
        let reg = SummarizerRegistry::__test_construct(map, default_name.to_string(), extractive);
        Arc::new(reg)
    }

    fn opts(name: &str, mode: CompactMode) -> CompactOpts {
        CompactOpts {
            mode,
            style: Style::Prose,
            target_tokens: None,
            focus: None,
            preserve: vec![],
            backend_name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn cache_hit_short_circuits_backend() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(vec![("default", "", None)], "default");
        let svc = SummarizerService::new(db.clone(), reg, true);
        let o = opts("default", CompactMode::Extractive);

        // First call inserts; second call hits the cache.
        let r1 = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
        let r2 = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
        assert_eq!(r1.summary_md, r2.summary_md);
    }

    #[tokio::test]
    async fn backend_failure_falls_back_to_extractive() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![
                (
                    "fast",
                    "gpt-4o-mini",
                    Some(BackendError::AuthFailed("401".into())),
                ),
                ("default", "", None),
            ],
            "default",
        );
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("fast", CompactMode::Abstractive);

        let r = svc.compact("h1", "hello world.", &o).await.unwrap();
        assert_eq!(r.effective_backend, "default");
        assert!(r.fallback.is_some());
        assert_eq!(r.fallback.unwrap().reason, "auth_failed");
        assert!(r.summary_md.contains("from default"));
    }

    #[tokio::test]
    async fn fallback_backend_failure_surfaces_with_fallback_name() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![
                (
                    "fast",
                    "gpt-4o-mini",
                    Some(BackendError::AuthFailed("401".into())),
                ),
                (
                    "default",
                    "",
                    Some(BackendError::Invalid("empty fallback content".into())),
                ),
            ],
            "default",
        );
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("fast", CompactMode::Abstractive);

        let r = svc.compact("h1", "hello world.", &o).await;
        // The original ("fast") failed with auth_failed → fallback dispatched
        // to ("default") → that also failed with Invalid → user must see
        // the fallback backend's error, not the original.
        match r {
            Err(SummarizerError::InvalidRequest { ref name, .. }) => {
                assert_eq!(
                    name, "default",
                    "error should carry fallback's name, not 'fast'"
                );
            }
            other => panic!("expected InvalidRequest from fallback, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_fallback_attempted_when_failing_backend_is_extractive_fallback() {
        let (db, _tmp) = make_db().await;
        // Single extractive backend named "default" that's also the fallback target.
        // When it errors, the service must NOT try to fall back to itself.
        let reg = registry_with(
            vec![("default", "", Some(BackendError::Invalid("empty".into())))],
            "default",
        );
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("default", CompactMode::Extractive);

        let r = svc.compact("h1", "anything.", &o).await;
        // Should return the original error (translated) with name = "default".
        // No fallback dispatch should happen.
        match r {
            Err(SummarizerError::InvalidRequest { ref name, .. }) => {
                assert_eq!(name, "default");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn backend_failure_propagates_when_fallback_disabled() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(
            vec![
                ("fast", "gpt-4o-mini", Some(BackendError::RateLimited)),
                ("default", "", None),
            ],
            "default",
        );
        let svc = SummarizerService::new(db, reg, false);
        let o = opts("fast", CompactMode::Abstractive);
        let r = svc.compact("h1", "hello world.", &o).await;
        assert!(matches!(r, Err(SummarizerError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn no_such_backend_errors_immediately() {
        let (db, _tmp) = make_db().await;
        let reg = registry_with(vec![("default", "", None)], "default");
        let svc = SummarizerService::new(db, reg, true);
        let o = opts("missing", CompactMode::Abstractive);
        let r = svc.compact("h", "x.", &o).await;
        assert!(matches!(r, Err(SummarizerError::NoSuchBackend { .. })));
    }
}

//! Built-in `rover doctor` checks.

use async_trait::async_trait;
use std::path::Path;

use super::{Check, CheckCtx, CheckReport, CheckStatus};

pub struct SqliteOpen;

#[async_trait]
impl Check for SqliteOpen {
    fn name(&self) -> &'static str {
        "sqlite_open"
    }
    async fn run(&self, _ctx: &CheckCtx) -> CheckReport {
        // Db is already open in CheckCtx — if we got here, it opened.
        CheckReport {
            check: self.name(),
            status: CheckStatus::Ok,
            detail: None,
        }
    }
}

pub struct SqliteWalMode;

#[async_trait]
impl Check for SqliteWalMode {
    fn name(&self) -> &'static str {
        "sqlite_wal_mode"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let mode_res: Result<String, _> = ctx
            .db
            .conn
            .call(|c| {
                c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                    .map_err(tokio_rusqlite::Error::from)
            })
            .await;
        match mode_res {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("journal_mode = {mode}")),
            },
            Ok(mode) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("expected wal, got {mode}")),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("query failed: {e}")),
            },
        }
    }
}

pub struct SqliteSchemaVersion;

#[async_trait]
impl Check for SqliteSchemaVersion {
    fn name(&self) -> &'static str {
        "sqlite_schema_version"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        // Sync with MIGRATIONS.len() in src/storage/mod.rs.
        const EXPECTED: u32 = 5;
        match ctx.db.schema_version().await {
            Ok(v) if v == EXPECTED => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("schema_version = {v}")),
            },
            Ok(v) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("schema_version = {v}, expected {EXPECTED}")),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("query failed: {e}")),
            },
        }
    }
}

pub struct OutputDirWritable;

#[async_trait]
impl Check for OutputDirWritable {
    fn name(&self) -> &'static str {
        "output_dir_writable"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let dir = match crate::extractor::output::OutputPaths::resolve(
            ctx.config.output.dir.as_deref(),
        ) {
            Ok(p) => p.root().to_path_buf(),
            Err(e) => {
                return CheckReport {
                    check: self.name(),
                    status: CheckStatus::Fail,
                    detail: Some(format!("could not resolve: {e}")),
                };
            }
        };
        let probe = dir.join(".rover_doctor_probe");
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                CheckReport {
                    check: self.name(),
                    status: CheckStatus::Ok,
                    detail: Some(format!("writable: {}", short(&dir))),
                }
            }
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("write probe failed at {}: {e}", short(&dir))),
            },
        }
    }
}

pub struct NetworkReachable;

#[async_trait]
impl Check for NetworkReachable {
    fn name(&self) -> &'static str {
        "network_reachable"
    }
    async fn run(&self, _ctx: &CheckCtx) -> CheckReport {
        crate::fetcher::client::install_ring_provider();
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return CheckReport {
                    check: self.name(),
                    status: CheckStatus::Fail,
                    detail: Some(format!("client build failed: {e}")),
                };
            }
        };
        match client.head("https://example.com").send().await {
            Ok(resp) if resp.status().is_success() => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("HEAD https://example.com → {}", resp.status())),
            },
            Ok(resp) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("HEAD https://example.com → {}", resp.status())),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("HEAD failed: {e}")),
            },
        }
    }
}

pub struct ExtractiveSynthesis;

#[async_trait]
impl Check for ExtractiveSynthesis {
    fn name(&self) -> &'static str {
        "extractive_synthesis"
    }
    async fn run(&self, _ctx: &CheckCtx) -> CheckReport {
        use crate::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
        // The extractive backend's `target_tokens` budget code uses the
        // tokenizer for per-sentence accounting. If the tokenizer isn't
        // loaded it falls back to a chars/4 heuristic and emits a warn —
        // accurate defense-in-depth signalling, but confusing as a user
        // experience ("why am I seeing a WARN above a green ✓?"). Every
        // other caller in the codebase (cli::fetch, the mcp tools) loads
        // the tokenizer first; the doctor check should follow the same
        // contract so the synthesis path runs at full fidelity.
        let family = crate::tokenizer::Tokenizer::O200k;
        if let Err(e) = crate::tokenizer::ensure_loaded(family).await {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("tokenizer {family:?} load failed: {e}")),
            };
        }
        let be = crate::summarizer::extractive::ExtractiveBackend::new("doctor", family);
        let opts = CompactOpts {
            mode: CompactMode::Extractive,
            style: Style::Prose,
            target_tokens: Some(50),
            focus: None,
            preserve: vec![],
            backend_name: "doctor".to_string(),
        };
        let content = "Rover is a polite scraper. It caches what it fetches. It summarizes \
                       what it caches. The summarizer is offline-first.";
        match be.compact(content, &opts).await {
            Ok(out) if !out.trim().is_empty() => CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some(format!("produced {} chars", out.chars().count())),
            },
            Ok(_) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some("extractive backend returned empty output".to_string()),
            },
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("extractive backend errored: {e}")),
            },
        }
    }
}

pub struct BackendsAuthenticate;

#[async_trait]
impl Check for BackendsAuthenticate {
    fn name(&self) -> &'static str {
        "backends_authenticate"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let cloud: Vec<(&String, &crate::config::BackendConfig)> = ctx
            .config
            .backends
            .iter()
            .filter(|(_, c)| c.kind == "cloud")
            .filter(|(_, c)| {
                c.api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
            })
            .collect();
        if cloud.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no configured cloud backends with non-empty api_key_env".to_string()),
            };
        }
        // Trivial completion per cloud backend. Each gets a 5s timeout.
        let mut failures: Vec<String> = Vec::new();
        for (name, cfg) in cloud {
            let provider_str = cfg.provider.as_deref().unwrap_or("");
            let model = cfg.model.as_deref().unwrap_or("");
            let api_key = cfg
                .api_key_env
                .as_deref()
                .and_then(|e| std::env::var(e).ok());
            let provider = match crate::summarizer::cloud::ProviderKind::parse(provider_str) {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!("{name}: invalid provider `{provider_str}`: {e}"));
                    continue;
                }
            };
            let backend = match crate::summarizer::cloud::CloudBackend::new(
                name.clone(),
                provider,
                model.to_string(),
                cfg.base_url.clone(),
                api_key,
            ) {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{name}: build failed: {e}"));
                    continue;
                }
            };
            use crate::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
            let opts = CompactOpts {
                mode: CompactMode::Abstractive,
                style: Style::Prose,
                target_tokens: Some(1),
                focus: None,
                preserve: vec![],
                backend_name: name.clone(),
            };
            let probe = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                backend.compact("ping", &opts),
            )
            .await;
            match probe {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => failures.push(format!("{name}: timeout after 5s")),
            }
        }
        if failures.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured cloud backends authenticated".to_string()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(failures.join("; ")),
            }
        }
    }
}

pub struct CaptionersAuthenticate;

#[async_trait]
impl Check for CaptionersAuthenticate {
    fn name(&self) -> &'static str {
        "captioners_authenticate"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        // 1x1 transparent PNG (67 bytes).
        const PROBE_PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        let cloud: Vec<(&String, &crate::config::CaptionerConfig)> = ctx
            .config
            .captioners
            .iter()
            .filter(|(_, c)| c.kind == "cloud")
            .filter(|(_, c)| {
                c.api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
            })
            .collect();
        if cloud.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no cloud captioners with non-empty api_key_env".into()),
            };
        }
        let mut failures = Vec::new();
        for (name, cfg) in cloud {
            let provider = match crate::summarizer::cloud::ProviderKind::parse(
                cfg.provider.as_deref().unwrap_or(""),
            ) {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!("{name}: invalid provider: {e}"));
                    continue;
                }
            };
            let api_key = cfg
                .api_key_env
                .as_deref()
                .and_then(|e| std::env::var(e).ok());
            let cap = match crate::vlm::cloud::CloudCaptioner::new(
                name,
                provider,
                cfg.model.as_deref().unwrap_or(""),
                cfg.base_url.clone(),
                api_key,
            ) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{name}: build failed: {e}"));
                    continue;
                }
            };
            use crate::vlm::VlmCaptioner;
            let probe = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cap.caption(PROBE_PNG, None, 1),
            )
            .await;
            match probe {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => failures.push(format!("{name}: timeout after 5s")),
            }
        }
        if failures.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured cloud captioners authenticated".into()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(failures.join("; ")),
            }
        }
    }
}

#[cfg(feature = "local-inference")]
pub struct LocalInferenceModelCached;

#[cfg(feature = "local-inference")]
#[async_trait]
impl Check for LocalInferenceModelCached {
    fn name(&self) -> &'static str {
        "local_inference_model_cached"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let locals: Vec<(&String, &crate::config::BackendConfig)> = ctx
            .config
            .backends
            .iter()
            .filter(|(_, c)| c.kind == "local")
            .collect();
        if locals.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no [backends.<name>] kind = \"local\" configured".into()),
            };
        }
        let mut missing: Vec<String> = Vec::new();
        for (name, cfg) in locals {
            let model = match cfg.model.as_deref() {
                Some(m) => m,
                None => {
                    missing.push(format!("{name}: model missing in config"));
                    continue;
                }
            };
            if !crate::summarizer::local::hf_cache_has(model) {
                missing.push(format!(
                    "{name}: model {model} not cached. Run `rover model download {model}`"
                ));
            }
        }
        if missing.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured local-inference backends have cached weights".into()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(missing.join("; ")),
            }
        }
    }
}

#[cfg(feature = "local-vision")]
pub struct LocalVisionModelCached;

#[cfg(feature = "local-vision")]
#[async_trait]
impl Check for LocalVisionModelCached {
    fn name(&self) -> &'static str {
        "local_vision_model_cached"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let locals: Vec<(&String, &crate::config::CaptionerConfig)> = ctx
            .config
            .captioners
            .iter()
            .filter(|(_, c)| c.kind == "local")
            .collect();
        if locals.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no [captioners.<name>] kind = \"local\" configured".into()),
            };
        }
        let mut missing: Vec<String> = Vec::new();
        for (name, cfg) in locals {
            let model = match cfg.model.as_deref() {
                Some(m) => m,
                None => {
                    missing.push(format!("{name}: model missing in config"));
                    continue;
                }
            };
            if !crate::vlm::local::hf_cache_has(model) {
                missing.push(format!(
                    "{name}: model {model} not cached. Run `rover model download {model}`"
                ));
            }
        }
        if missing.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured local-vision captioners have cached weights".into()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(missing.join("; ")),
            }
        }
    }
}

fn short(p: &Path) -> String {
    if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from)
        && let Ok(stripped) = p.strip_prefix(&home)
    {
        return format!("~/{}", stripped.display());
    }
    p.display().to_string()
}

#[cfg(feature = "headless")]
pub struct HeadlessBrowserLaunches;

#[cfg(feature = "headless")]
#[async_trait]
impl Check for HeadlessBrowserLaunches {
    fn name(&self) -> &'static str {
        "headless_browser_launches"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let result = crate::fetcher::headless::browser::launch(&ctx.config.headless).await;
        match result {
            Ok((browser, handler)) => {
                let exec = format!(
                    "(launched via {})",
                    if ctx.config.headless.chrome_executable.is_empty() {
                        "auto-detection"
                    } else {
                        &ctx.config.headless.chrome_executable
                    }
                );
                drop(browser);
                handler.abort();
                CheckReport {
                    check: self.name(),
                    status: CheckStatus::Ok,
                    detail: Some(format!("browser launched {exec}")),
                }
            }
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!(
                    "{e}. See docs/features.md for install instructions."
                )),
            },
        }
    }
}

//! `rover doctor` — diagnostic checks.

pub mod checks;

use std::sync::Arc;
use thiserror::Error;

use crate::config::Config;
use crate::storage::Db;

#[derive(Debug, Error)]
pub enum DoctorError {
    #[error("doctor check infrastructure error: {0}")]
    Infrastructure(String),
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Fail,
    Skip,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckReport {
    pub check: &'static str,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct CheckCtx {
    pub config: Arc<Config>,
    pub db: Db,
}

#[async_trait::async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, ctx: &CheckCtx) -> CheckReport;
}

/// Run every built-in check sequentially. Order: cheap → expensive.
/// Returns the full report list and a summary status (`Fail` if any
/// check failed; `Ok` otherwise — `Skip` is non-failing).
pub async fn run_all(ctx: &CheckCtx) -> (Vec<CheckReport>, CheckStatus) {
    #[allow(unused_mut)]
    let mut checks: Vec<Box<dyn Check>> = vec![
        Box::new(checks::SqliteOpen),
        Box::new(checks::SqliteWalMode),
        Box::new(checks::SqliteSchemaVersion),
        Box::new(checks::OutputDirWritable),
        Box::new(checks::NetworkReachable),
        Box::new(checks::ExtractiveSynthesis),
        Box::new(checks::BackendsAuthenticate),
        Box::new(checks::CaptionersAuthenticate),
    ];
    #[cfg(feature = "local-inference")]
    checks.push(Box::new(checks::LocalInferenceModelCached));
    #[cfg(feature = "local-vision")]
    checks.push(Box::new(checks::LocalVisionModelCached));
    #[cfg(feature = "headless")]
    checks.push(Box::new(checks::HeadlessBrowserLaunches));
    let mut reports = Vec::with_capacity(checks.len());
    let mut summary = CheckStatus::Ok;
    for c in &checks {
        let r = c.run(ctx).await;
        if r.status == CheckStatus::Fail {
            summary = CheckStatus::Fail;
        }
        reports.push(r);
    }
    (reports, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_ctx() -> (CheckCtx, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let mut cfg = Config::default();
        cfg.output.dir = Some(tmp.path().to_path_buf());
        (
            CheckCtx {
                config: Arc::new(cfg),
                db,
            },
            tmp,
        )
    }

    #[tokio::test]
    async fn sqlite_open_passes_on_fresh_db() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::SqliteOpen.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok);
    }

    #[tokio::test]
    async fn sqlite_wal_mode_passes_on_fresh_db() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::SqliteWalMode.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn sqlite_schema_version_passes_on_fresh_db() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::SqliteSchemaVersion.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn output_dir_writable_passes_on_writable_temp() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::OutputDirWritable.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn backends_authenticate_skips_when_no_cloud_configured() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::BackendsAuthenticate.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Skip);
    }

    #[tokio::test]
    async fn extractive_synthesis_produces_output() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::ExtractiveSynthesis.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);
    }

    #[tokio::test]
    async fn captioners_authenticate_skips_when_no_cloud_configured() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::CaptionersAuthenticate.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Skip);
    }

    #[cfg(feature = "local-inference")]
    #[tokio::test]
    async fn local_inference_model_cached_skips_when_no_local_configured() {
        let (ctx, _g) = fresh_ctx().await;
        let r = checks::LocalInferenceModelCached.run(&ctx).await;
        assert_eq!(r.status, CheckStatus::Skip);
    }
}

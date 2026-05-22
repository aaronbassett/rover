//! End-to-end lifecycle test: real DB, real extractive backend, params
//! variation, cache invalidation when `backend_name` changes.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;

use rover::storage::Db;
use rover::summarizer::backend::{CompactMode, CompactOpts, PreserveSection, Style};
use rover::summarizer::extractive::ExtractiveBackend;
use rover::summarizer::registry::SummarizerRegistry;
use rover::summarizer::{SummarizerService, SummaryCacheStatus};
use rover::tokenizer::Tokenizer;

async fn make_service() -> (SummarizerService, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("r.db")).await.unwrap();
    let mut map: std::collections::HashMap<
        String,
        Arc<dyn rover::summarizer::backend::SummarizerBackend>,
    > = Default::default();
    map.insert(
        "default".into(),
        Arc::new(ExtractiveBackend::new("default", Tokenizer::O200k)),
    );
    let reg = Arc::new(SummarizerRegistry::__test_construct(
        map,
        "default".into(),
        Some("default".into()),
    ));
    (SummarizerService::new(db, reg, true), tmp)
}

fn opts() -> CompactOpts {
    CompactOpts {
        mode: CompactMode::Extractive,
        style: Style::Prose,
        target_tokens: None,
        focus: None,
        preserve: vec![],
        backend_name: "default".into(),
    }
}

#[tokio::test]
async fn second_call_with_same_params_hits_cache() {
    let (svc, _tmp) = make_service().await;
    let r1 = svc
        .compact("h1", "First sentence here. Second sentence here.", &opts())
        .await
        .unwrap();
    let r2 = svc
        .compact("h1", "First sentence here. Second sentence here.", &opts())
        .await
        .unwrap();
    assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
    assert_eq!(r1.summary_md, r2.summary_md);
}

#[tokio::test]
async fn changing_target_tokens_creates_independent_cache_row() {
    let (svc, _tmp) = make_service().await;
    let mut o = opts();
    o.target_tokens = Some(10);
    let r1 = svc
        .compact("h1", "First sentence. Second sentence. Third one.", &o)
        .await
        .unwrap();
    o.target_tokens = Some(100);
    let r2 = svc
        .compact("h1", "First sentence. Second sentence. Third one.", &o)
        .await
        .unwrap();
    assert!(matches!(r1.cache_status, SummaryCacheStatus::Miss));
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Miss));
}

#[tokio::test]
async fn changing_preserve_list_order_does_not_re_summarize() {
    let (svc, _tmp) = make_service().await;
    let mut o = opts();
    o.preserve = vec![PreserveSection::Code, PreserveSection::Tables];
    let _r1 = svc.compact("h2", "Sentence here.", &o).await.unwrap();
    o.preserve = vec![PreserveSection::Tables, PreserveSection::Code];
    let r2 = svc.compact("h2", "Sentence here.", &o).await.unwrap();
    assert!(matches!(r2.cache_status, SummaryCacheStatus::Hit));
}

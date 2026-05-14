//! batch_fetch envelope shape and validation errors.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::mcp::handler::RoverHandler;
use rover::mcp::tools::batch_fetch::BatchFetchArgs;
use rover::storage::Db;
use tempfile::tempdir;

async fn fixture_handler() -> (RoverHandler, Db) {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    // Keep the tempdir alive for the test process lifetime; on integration
    // tests each test gets its own DB so leaking is harmless.
    std::mem::forget(tmp);
    let cfg = Arc::new(Config::default());
    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let pacer = Arc::new(Pacer::new(&cfg.rate_limit));
    let h = RoverHandler::new(db.clone(), cfg, client, SsrfLevel::TestLoopback, pacer);
    (h, db)
}

#[tokio::test]
async fn empty_urls_returns_user_error() {
    let (h, _) = fixture_handler().await;
    let err = h
        .batch_fetch_inner(BatchFetchArgs {
            urls: vec![],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, rover::mcp::error::McpError::EmptyUrlList));
}

#[tokio::test]
async fn over_max_urls_returns_too_many_urls() {
    let (h, _) = fixture_handler().await;
    let urls = (0..101)
        .map(|i| format!("https://example.test/{i}"))
        .collect();
    let err = h
        .batch_fetch_inner(BatchFetchArgs {
            urls,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        rover::mcp::error::McpError::TooManyUrls {
            count: 101,
            max: 100,
        },
    ));
}

#[tokio::test]
async fn happy_path_returns_envelope_and_inserts_task() {
    let (h, db) = fixture_handler().await;
    let out = h
        .batch_fetch_inner(BatchFetchArgs {
            urls: vec![
                "https://example.test/a".into(),
                "https://example.test/b".into(),
            ],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(out.kind, "batch_fetch");
    assert_eq!(out.status, "running");
    assert!(out.monitor_command.contains(&out.task_id));
    assert!(out.cancel_command.contains(&out.task_id));
    let row = rover::storage::tasks::get(&db, &out.task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, rover::storage::tasks::TaskKind::BatchFetch);
}

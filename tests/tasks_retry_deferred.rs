//! end-to-end: long retry-after produces a retry task that runs to completion.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::tasks::{TaskKind, TaskStatus, get};
use rover::tasks::WorkerDeps;
use rover::tasks::retry::run as retry_run;
use rover::tasks::types::{RetryParams, TaskId};

// Readability needs a meaningful article body; a bare `<body>ok</body>` makes
// the extractor return "no article" and surfaces as `FetcherError::Extract`,
// which would cause `retry_succeeded` to never be emitted.
const MEATY_BODY: &str = "<html><head><title>t</title></head><body>\
    <article><h1>Hello</h1>\
    <p>This is a sufficiently long paragraph so that readability \
    recognises it as the primary article content and the extractor \
    does not bail out with `readabilityrs returned no article`.</p>\
    <p>Another paragraph to give the readability heuristics enough \
    signal to lock onto this block as the article body.</p>\
    </article></body></html>";

#[tokio::test]
async fn retry_succeeds_on_second_attempt() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MEATY_BODY))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = WorkerDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::Loopback,
    };

    let params = RetryParams {
        url: format!("{}/", server.uri()),
        attempt: 1,
        wait_ms_initial: 50,
        max_attempts: 3,
        parent_task_id: None,
    };
    rover::storage::tasks::insert(
        &db,
        rover::storage::tasks::TaskInsert {
            id: "r1".into(),
            kind: TaskKind::Retry,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    retry_run(deps, db.clone(), TaskId("r1".into()), cancel).await;

    let row = get(&db, "r1").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Completed);
    let evs = rover::storage::events::range_since(&db, "r1", 0, 100)
        .await
        .unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"retry_attempted"));
    assert!(kinds.contains(&"retry_succeeded"));
    assert!(kinds.contains(&"task_completed"));
}

#[tokio::test]
async fn retry_max_attempts_exhausted_terminal_failure() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let deps = WorkerDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::Loopback,
    };

    let params = RetryParams {
        url: format!("{}/", server.uri()),
        attempt: 3,
        wait_ms_initial: 10,
        max_attempts: 3,
        parent_task_id: None,
    };
    rover::storage::tasks::insert(
        &db,
        rover::storage::tasks::TaskInsert {
            id: "r2".into(),
            kind: TaskKind::Retry,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    retry_run(
        deps,
        db.clone(),
        TaskId("r2".into()),
        CancellationToken::new(),
    )
    .await;
    let row = get(&db, "r2").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Failed);
    assert_eq!(row.error.as_deref(), Some("retries_exhausted"));
}

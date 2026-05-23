//! batch_fetch worker honours cancellation between items.

#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::config::Config;
use rover::fetcher::client::build_http_client;
use rover::fetcher::concurrency::Pacer;
use rover::fetcher::ssrf::SsrfLevel;
use rover::storage::Db;
use rover::storage::events;
use rover::storage::tasks::{
    TaskInsert, TaskKind, TaskStatus, get, insert, set_cancellation_requested,
};
use rover::tasks::WorkerDeps;
use rover::tasks::batch_fetch::run as batch_run;
use rover::tasks::types::{BatchFetchParams, TaskId};

#[tokio::test]
async fn cancellation_between_items_stops_loop() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let server = MockServer::start().await;
    // Readability needs a meaningful article body; otherwise extraction fails
    // and the worker records `item_failed` for every URL — which would still
    // exercise the cancellation path, but `item_done >= 1` is what the test
    // gates on per the plan.
    let body = "<html><head><title>t</title></head><body>\
        <article><h1>Hello</h1>\
        <p>This is a sufficiently long paragraph so that readability \
        recognises it as the primary article content and the extractor \
        does not bail out with `readabilityrs returned no article`.</p>\
        <p>Another paragraph to give the readability heuristics enough \
        signal to lock onto this block as the article body.</p>\
        </article></body></html>";
    Mock::given(path_regex(r"^/page/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut cfg = Config::default();
    cfg.robots.respect = false;
    // Raise the per-domain rpm so 5 serial fetches don't blow the timeout
    // budget; the test gates cancellation on item completion timing, not
    // throughput.
    cfg.rate_limit.requests_per_minute_per_domain = 6000;
    let deps = WorkerDeps {
        client: build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout()),
        pacer: Arc::new(Pacer::new(&cfg.rate_limit)),
        cache_cfg: cfg.cache.clone(),
        rate_cfg: cfg.rate_limit.clone(),
        robots_cfg: cfg.robots.clone(),
        fetch_cfg: cfg.fetch.clone(),
        ssrf_level: SsrfLevel::Loopback,
    };

    let urls: Vec<String> = (0..5)
        .map(|i| format!("{}/page/{i}", server.uri()))
        .collect();
    let params = BatchFetchParams {
        urls: urls.clone(),
        concurrency: 1,
        per_domain_concurrency: 1,
        force_refresh: false,
    };
    insert(
        &db,
        TaskInsert {
            id: "cx".into(),
            kind: TaskKind::BatchFetch,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    let db_c = db.clone();
    let worker = tokio::spawn(async move {
        batch_run(deps, db_c, TaskId("cx".into()), cancel.clone()).await;
    });

    // Wait until at least one item is done, then request cancellation.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let counts = events::count_by_kind(&db, "cx").await.unwrap();
            let done = counts
                .iter()
                .find_map(|(k, n)| if k == "item_done" { Some(*n) } else { None })
                .unwrap_or(0);
            if done >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no item completed in time");
    let _ = set_cancellation_requested(&db, "cx").await;

    worker.await.unwrap();
    let row = get(&db, "cx").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Cancelled);

    let counts = events::count_by_kind(&db, "cx").await.unwrap();
    let done = counts
        .iter()
        .find(|(k, _)| k == "item_done")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let started = counts
        .iter()
        .find(|(k, _)| k == "item_started")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    assert!(done >= 1);
    assert!(
        started < urls.len() as i64,
        "expected fewer than {} item_started, got {started}",
        urls.len(),
    );
    let evs = events::range_since(&db, "cx", 0, 1000).await.unwrap();
    assert!(evs.iter().any(|e| e.kind == "task_cancelled"));
}

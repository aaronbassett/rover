//! Integration test for the M6 SWR fast-path.
//!
//! Verifies that a cache lookup which returns an expired row produces a
//! `CacheStatus::Stale { revalidation_task_id: Some(_) }` envelope and that
//! a corresponding `revalidate` task row is inserted into `tasks`.

#![cfg(feature = "test-loopback")]

#[tokio::test]
async fn stale_path_inserts_revalidate_task() {
    use rover::config::Config;
    use rover::fetcher::cached::{
        CacheStatus, FetchOptions, HeadlessMode, fetch_with_cache, sha256_hex,
    };
    use rover::fetcher::client::build_http_client;
    use rover::fetcher::concurrency::Pacer;
    use rover::fetcher::ssrf::SsrfLevel;
    use rover::storage::Db;
    use rover::storage::pages::{self, Page, url_hash};
    use rover::storage::tasks::{self, TaskKind};
    use tempfile::tempdir;
    use url::Url;

    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let url = Url::parse("https://example.test/article").unwrap();

    // Seed a stale page row.
    let now = jiff::Timestamp::now().as_second();
    pages::upsert(
        &db,
        Page {
            url_hash: url_hash(url.as_str()),
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("t".into()),
            fetched_at: now - 7200,
            expires_at: Some(now - 60),
            etag: Some("\"abc\"".into()),
            last_modified: None,
            content_hash: sha256_hex(b"old"),
            extracted_md: "old".into(),
            metadata_json: None,
            raw_html: None,
        },
    )
    .await
    .unwrap();

    // No HTTP path will be hit — we expect the stale return before any fetch.
    let mut cfg = Config::default();
    cfg.robots.respect = false;
    let pacer = Pacer::new(&cfg.rate_limit);
    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let cf = fetch_with_cache(
        &db,
        &client,
        &pacer,
        &cfg.rate_limit,
        &cfg.robots,
        &url,
        &cfg.cache,
        FetchOptions {
            force_refresh: false,
            ssrf_level: SsrfLevel::Loopback,
            ssrf_project_root: None,
            har_recorder: None,
            ignore_robots: true,
            user_agent: cfg.fetch.user_agent.clone(),
            #[cfg(feature = "headless")]
            headless: None,
            headless_mode: HeadlessMode::Off,
            synchronous_revalidation: false,
        },
        |_b, _u| panic!("extract_fn should not run on stale-served path"),
    )
    .await
    .unwrap();

    let task_id = match cf.cache_status {
        CacheStatus::Stale {
            revalidation_task_id: Some(id),
        } => id,
        other => panic!("expected stale with revalidation_task_id, got {other:?}"),
    };
    let row = tasks::get(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(row.kind, TaskKind::Revalidate);
}

// Readability needs a meaningful article body; a bare `<body>fresh</body>`
// makes the extractor return "no article" and surfaces as
// `FetcherError::Extract`, flipping the worker into the failure arm.
const MEATY_BODY: &str = "<html><head><title>t</title></head><body>\
    <article><h1>Hello</h1>\
    <p>This is a sufficiently long paragraph so that readability \
    recognises it as the primary article content and the extractor \
    does not bail out with `readabilityrs returned no article`.</p>\
    <p>Another paragraph to give the readability heuristics enough \
    signal to lock onto this block as the article body.</p>\
    </article></body></html>";

#[tokio::test]
async fn revalidate_marks_completed_after_fresh_fetch() {
    use std::sync::Arc;

    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use rover::config::Config;
    use rover::fetcher::client::build_http_client;
    use rover::fetcher::concurrency::Pacer;
    use rover::fetcher::ssrf::SsrfLevel;
    use rover::storage::Db;
    use rover::storage::events;
    use rover::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert};
    use rover::tasks::WorkerDeps;
    use rover::tasks::revalidate::run as revalidate_run;
    use rover::tasks::types::{RevalidateParams, TaskId};
    use tempfile::tempdir;

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
        ssrf_project_root: None,
        har_recorder: None,
    };
    let params = RevalidateParams {
        url: format!("{}/page", server.uri()),
        etag_at_serve: None,
        last_modified_at_serve: None,
    };
    insert(
        &db,
        TaskInsert {
            id: "rv".into(),
            kind: TaskKind::Revalidate,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    revalidate_run(
        deps,
        db.clone(),
        TaskId("rv".into()),
        CancellationToken::new(),
    )
    .await;
    let row = get(&db, "rv").await.unwrap().unwrap();
    assert_eq!(row.status, TaskStatus::Completed);
    let evs = events::range_since(&db, "rv", 0, 100).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"revalidation_started"));
    assert!(kinds.contains(&"revalidation_completed"));
}

/// Regression: every `storage::tasks::insert` must notify a listener
/// installed via `Db::set_new_task_sender`. Before the M6 fix, only the
/// MCP `batch_fetch` tool sent on the scheduler channel directly; tasks
/// inserted by the fetcher SWR path, the deferred retry path, and the
/// retry-chain worker never reached the scheduler because the orphan scan
/// excludes the live `servers.pid` of the inserting process.
#[tokio::test]
async fn insert_notifies_scheduler_via_storage_channel() {
    use std::time::Duration;

    use rover::storage::{self, Db};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    db.set_new_task_sender(tx);

    storage::tasks::insert(
        &db,
        storage::tasks::TaskInsert {
            id: "rv".into(),
            kind: storage::tasks::TaskKind::Revalidate,
            params_json: "{}".into(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();

    let got = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("notify did not fire");
    assert_eq!(got.as_deref(), Some("rv"));
}

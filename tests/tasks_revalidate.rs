//! Integration test for the M6 SWR fast-path.
//!
//! Verifies that a cache lookup which returns an expired row produces a
//! `CacheStatus::Stale { revalidation_task_id: Some(_) }` envelope and that
//! a corresponding `revalidate` task row is inserted into `tasks`.

#![cfg(feature = "test-loopback")]

#[tokio::test]
async fn stale_path_inserts_revalidate_task() {
    use rover::config::Config;
    use rover::fetcher::cached::{CacheStatus, FetchOptions, fetch_with_cache, sha256_hex};
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
            ssrf_level: SsrfLevel::TestLoopback,
            ignore_robots: true,
            user_agent: cfg.fetch.user_agent.clone(),
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

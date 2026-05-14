//! `rover batch <id>` snapshot in human and ndjson formats.

// Each test holds `ENV_LOCK` across awaits to keep `ROVER_DATA_DIR`
// stable for the duration of `run_with_writers`. This is intentional —
// the alternative (tokio::sync::Mutex) buys nothing here since the
// tests are sequential by design.
#![allow(clippy::await_holding_lock)]

use std::sync::Mutex;

use tempfile::tempdir;

use rover::cli::task::{Args, OutputFormat, run_with_writers};
use rover::storage::Db;
use rover::storage::events::{EventInsert, append};
use rover::storage::tasks::{TaskInsert, TaskKind, insert};

/// Serialise `ROVER_DATA_DIR` mutations: cargo runs `#[tokio::test]`s in
/// the same process on a shared thread pool, so two tests racing
/// `set_var` would let one observe the other's tempdir and open the
/// wrong DB.
static ENV_LOCK: Mutex<()> = Mutex::new(());

async fn seed_running_batch(db: &Db, id: &str) {
    let params = rover::tasks::types::BatchFetchParams {
        urls: vec!["https://a/".into(), "https://b/".into()],
        concurrency: 2,
        per_domain_concurrency: 1,
        force_refresh: false,
    };
    insert(
        db,
        TaskInsert {
            id: id.into(),
            kind: TaskKind::BatchFetch,
            params_json: serde_json::to_string(&params).unwrap(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    append(
        db,
        EventInsert {
            task_id: id.into(),
            kind: "item_done".into(),
            payload_json: r#"{"index":0,"url":"https://a/"}"#.into(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn snapshot_human_includes_tip_when_running() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    // SAFETY: `set_var` is unsafe in Rust 2024; the `ENV_LOCK` mutex
    // serialises every test in this file that touches `ROVER_DATA_DIR`.
    unsafe {
        std::env::set_var("ROVER_DATA_DIR", tmp.path());
    }
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id1").await;
    db.upsert_server_self(std::process::id() as i64, "v".into())
        .await
        .unwrap();
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id1".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Human,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Batch id1"), "got: {out}");
    assert!(
        out.contains("Tip: use `rover task id1 --cancel`"),
        "got: {out}"
    );
}

#[tokio::test]
async fn snapshot_ndjson_is_single_line() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    // SAFETY: see snapshot_human_includes_tip_when_running.
    unsafe {
        std::env::set_var("ROVER_DATA_DIR", tmp.path());
    }
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id2").await;
    db.upsert_server_self(std::process::id() as i64, "v".into())
        .await
        .unwrap();
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id2".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Ndjson,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "expected single line, got {lines:?}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["task_id"], "id2");
    assert_eq!(v["task_kind"], "batch_fetch");
    assert!(v.get("succeeded").is_some());
    assert!(v.get("failed").is_some());
    assert!(v.get("in_flight").is_some());
}

#[tokio::test]
async fn snapshot_human_emits_liveness_warning_when_no_server_row() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    // SAFETY: see snapshot_human_includes_tip_when_running.
    unsafe {
        std::env::set_var("ROVER_DATA_DIR", tmp.path());
    }
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    seed_running_batch(&db, "id3").await;
    // Deliberately do not call upsert_server_self.
    drop(db);

    let mut buf: Vec<u8> = Vec::new();
    run_with_writers(
        Args {
            id: "id3".into(),
            monitor: false,
            cancel: false,
            format: OutputFormat::Human,
            from_event: None,
            expect_kind: Some("batch_fetch"),
        },
        None,
        &mut buf,
    )
    .await
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("⚠"), "expected liveness warning, got: {out}");
    assert!(out.contains("rover mcp"));
}

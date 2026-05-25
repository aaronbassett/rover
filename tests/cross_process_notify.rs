//! In-process scheduler wake-up via SQLite `update_hook`.
//!
//! A second task on a cloned `Db` handle (sharing the storage actor)
//! inserts a `tasks` row; the `update_hook` installed by
//! `register_tasks_update_hook` fires and wakes the `Notify` well before
//! the scheduler's polling tick (default 10s) would have.
//!
//! Note: SQLite `update_hook` is **per-connection**. It does not fire
//! for writes via a separate `rusqlite::Connection` (whether in another
//! `Db` in the same process or in another OS process). The storage layer
//! funnels every write through `Db`'s single actor connection, so any
//! caller that shares the same `Db` benefits. Code that opens its own
//! `Db` or writes from a separate process still relies on the polling
//! tick fallback.
//!
//! The test name retains the plan's original wording for traceability;
//! the actual mechanism is single-actor, multi-caller, same-process.
#![cfg(feature = "test-loopback")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rover::storage::Db;
use rover::storage::register_tasks_update_hook;
use rover::storage::tasks::{TaskInsert, TaskKind, insert};

#[tokio::test]
async fn same_process_notify() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("rover.db");

    let db = Db::open(&db_path).await.unwrap();
    let notify = Arc::new(tokio::sync::Notify::new());
    let _guard = register_tasks_update_hook(&db, notify.clone())
        .await
        .unwrap();

    // A separate task uses a clone of the same `Db` (sharing the storage
    // actor) to insert a row. This is the path update_hook is meant to
    // cover: code that performs a `tasks` write without going through the
    // scheduler's mpsc channel.
    let db_clone = db.clone();
    tokio::spawn(async move {
        insert(
            &db_clone,
            TaskInsert {
                id: "task-test-id".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
    });

    let start = Instant::now();
    tokio::time::timeout(Duration::from_millis(200), notify.notified())
        .await
        .expect("notify did not fire within 200ms");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(200),
        "observed at {elapsed:?}"
    );
}

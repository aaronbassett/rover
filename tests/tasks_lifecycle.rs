//! End-to-end scheduler + worker lifecycle.

use std::time::Duration;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use rover::storage::Db;
use rover::storage::events;
use rover::storage::tasks::{TaskInsert, TaskKind, TaskStatus, get, insert};
use rover::tasks::default_spawner;
use rover::tasks::scheduler::{Scheduler, SchedulerConfig};
use rover::tasks::types::TaskId;

#[tokio::test]
async fn summarize_stub_runs_through_scheduler() {
    let tmp = tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
    let (tx, rx) = Scheduler::channel();
    insert(
        &db,
        TaskInsert {
            id: "t1".into(),
            kind: TaskKind::Summarize,
            params_json: "{}".into(),
            owner_pid: Some(1),
        },
    )
    .await
    .unwrap();
    tx.send(TaskId("t1".into())).unwrap();
    let cancel = CancellationToken::new();
    let sched = Scheduler {
        db: db.clone(),
        cfg: SchedulerConfig {
            own_pid: 1,
            orphan_scan_interval: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(200),
        },
        cancel: cancel.clone(),
        new_task_rx: rx,
        spawner: default_spawner(),
    };
    let handle = tokio::spawn(sched.run());
    let row = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let r = get(&db, "t1").await.unwrap().unwrap();
            if r.status.is_terminal() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("task did not reach terminal status in time");
    assert_eq!(row.status, TaskStatus::Failed);
    assert_eq!(
        row.error.as_deref(),
        Some("summarization_not_yet_implemented")
    );
    let evs = events::range_since(&db, "t1", 0, 100).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["task_started", "task_failed"]);
    cancel.cancel();
    handle.await.unwrap().unwrap();
}

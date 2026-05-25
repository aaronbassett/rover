//! Single scheduler loop owning task lifecycle.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskKind, TaskStatus, set_status};
use crate::storage::{self, Db};
use crate::tasks::error::TasksError;
use crate::tasks::types::{CoreEvent, TaskId};

pub type NewTaskSender = mpsc::UnboundedSender<TaskId>;
pub type NewTaskReceiver = mpsc::UnboundedReceiver<TaskId>;

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub own_pid: i64,
    pub orphan_scan_interval: Duration,
    pub shutdown_grace: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            own_pid: std::process::id() as i64,
            orphan_scan_interval: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

pub struct Scheduler {
    pub db: Db,
    pub cfg: SchedulerConfig,
    pub cancel: CancellationToken,
    pub new_task_rx: NewTaskReceiver,
    pub spawner: Arc<dyn WorkerSpawner>,
}

/// Trait-object indirection so workers wire in via DefaultSpawner without
/// the scheduler depending on every concrete worker module.
pub trait WorkerSpawner: Send + Sync + 'static {
    fn spawn(
        &self,
        join_set: &mut JoinSet<()>,
        db: Db,
        task_id: TaskId,
        kind: TaskKind,
        cancel: CancellationToken,
    );
}

impl Scheduler {
    pub fn channel() -> (NewTaskSender, NewTaskReceiver) {
        mpsc::unbounded_channel()
    }

    pub async fn run(mut self) -> Result<(), TasksError> {
        self.db
            .upsert_server_self(self.cfg.own_pid, env!("CARGO_PKG_VERSION").into())
            .await?;
        let mut tick = tokio::time::interval(self.cfg.orphan_scan_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut join_set: JoinSet<()> = JoinSet::new();

        // Fast in-process wake-up: any insert/update on `tasks` from any
        // Connection in this process fires the hook. Cross-process writes
        // still rely on the polling tick above.
        let hook_notify = Arc::new(tokio::sync::Notify::new());
        let _hook_guard =
            match storage::register_tasks_update_hook(&self.db, hook_notify.clone()).await {
                Ok(g) => Some(g),
                Err(e) => {
                    tracing::warn!(
                        target: "rover::tasks",
                        error = ?e,
                        "could not register tasks update_hook; falling back to polling only",
                    );
                    None
                }
            };

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => break,
                _ = tick.tick() => {
                    if let Err(e) = self.scan_and_claim_orphans(&mut join_set).await {
                        tracing::warn!(target: "rover::tasks", error = ?e, "orphan scan failed");
                    }
                }
                _ = hook_notify.notified() => {
                    if let Err(e) = self.scan_and_claim_orphans(&mut join_set).await {
                        tracing::warn!(target: "rover::tasks", error = ?e, "hook-driven scan failed");
                    }
                }
                Some(task_id) = self.new_task_rx.recv() => {
                    if let Err(e) = self.handle_new_task(&mut join_set, task_id).await {
                        tracing::warn!(target: "rover::tasks", error = ?e, "spawn failed");
                    }
                }
                Some(res) = join_set.join_next() => {
                    if let Err(e) = res {
                        tracing::error!(target: "rover::tasks", error = ?e, "worker panicked");
                    }
                }
            }
        }

        let _ = tokio::time::timeout(self.cfg.shutdown_grace, async {
            while join_set.join_next().await.is_some() {}
        })
        .await;
        Ok(())
    }

    pub async fn scan_and_claim_orphans(
        &self,
        join_set: &mut JoinSet<()>,
    ) -> Result<(), TasksError> {
        let orphans = storage::tasks::list_orphans(&self.db).await?;
        for orphan in orphans {
            let orphan_pid = match orphan.owner_pid {
                Some(p) => p,
                None => continue,
            };
            if !orphan.kind.is_resumable() {
                let claimed = storage::tasks::claim_orphan(
                    &self.db,
                    &orphan.id,
                    orphan_pid,
                    self.cfg.own_pid,
                )
                .await?;
                if claimed {
                    // Event-before-status ordering matches every worker, so
                    // a tailing CLI never sees `Failed` without a prior
                    // `task_failed` event.
                    append(
                        &self.db,
                        EventInsert {
                            task_id: orphan.id.clone(),
                            kind: CoreEvent::TaskFailed.as_str().into(),
                            payload_json: r#"{"error":"owner_died","message":"original owner pid disappeared and task kind is not resumable"}"#.into(),
                        },
                    )
                    .await?;
                    set_status(
                        &self.db,
                        &orphan.id,
                        TaskStatus::Failed,
                        None,
                        Some("owner_died".into()),
                    )
                    .await?;
                }
                continue;
            }
            let claimed =
                storage::tasks::claim_orphan(&self.db, &orphan.id, orphan_pid, self.cfg.own_pid)
                    .await?;
            if claimed {
                tracing::info!(
                    target: "rover::tasks",
                    task_id = %orphan.id,
                    kind = orphan.kind.as_str(),
                    "claimed orphaned task"
                );
                let task_cancel = self.cancel.child_token();
                self.spawner.spawn(
                    join_set,
                    self.db.clone(),
                    TaskId(orphan.id.clone()),
                    orphan.kind,
                    task_cancel,
                );
            }
        }
        Ok(())
    }

    async fn handle_new_task(
        &self,
        join_set: &mut JoinSet<()>,
        task_id: TaskId,
    ) -> Result<(), TasksError> {
        let row = storage::tasks::get(&self.db, task_id.as_str())
            .await?
            .ok_or_else(|| TasksError::NotFound(task_id.as_str().to_string()))?;
        let task_cancel = self.cancel.child_token();
        self.spawner
            .spawn(join_set, self.db.clone(), task_id, row.kind, task_cancel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    async fn fresh_db() -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        db
    }

    #[derive(Default)]
    struct RecordingSpawner {
        spawned: AtomicUsize,
        kinds: Mutex<Vec<TaskKind>>,
    }

    impl WorkerSpawner for RecordingSpawner {
        fn spawn(
            &self,
            join_set: &mut JoinSet<()>,
            _db: Db,
            _task_id: TaskId,
            kind: TaskKind,
            cancel: CancellationToken,
        ) {
            self.spawned.fetch_add(1, Ordering::SeqCst);
            self.kinds.lock().unwrap().push(kind);
            join_set.spawn(async move {
                cancel.cancelled().await;
            });
        }
    }

    async fn mk_sched(
        db: Db,
        pid: i64,
        cancel: CancellationToken,
    ) -> (Scheduler, Arc<RecordingSpawner>) {
        db.upsert_server_self(pid, "test".into()).await.unwrap();
        let (_tx, rx) = Scheduler::channel();
        let spawner = Arc::new(RecordingSpawner::default());
        let sched = Scheduler {
            db,
            cfg: SchedulerConfig {
                own_pid: pid,
                orphan_scan_interval: Duration::from_millis(50),
                shutdown_grace: Duration::from_millis(100),
            },
            cancel,
            new_task_rx: rx,
            spawner: spawner.clone(),
        };
        (sched, spawner)
    }

    #[tokio::test]
    async fn scan_claims_resumable_orphan() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "orphan".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 1, cancel.clone()).await;
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        let row = crate::storage::tasks::get(&db, "orphan")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.owner_pid, Some(1));
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
        assert_eq!(
            spawner.kinds.lock().unwrap().as_slice(),
            &[TaskKind::BatchFetch]
        );
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_millis(200), async {
            while js.join_next().await.is_some() {}
        })
        .await;
    }

    #[tokio::test]
    async fn scan_marks_non_resumable_orphan_failed() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "stub".into(),
                kind: TaskKind::Summarize,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 2, cancel.clone()).await;
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        cancel.cancel();
        let row = crate::storage::tasks::get(&db, "stub")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, TaskStatus::Failed);
        assert_eq!(row.error.as_deref(), Some("owner_died"));
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn scan_skips_live_pids() {
        let db = fresh_db().await;
        db.upsert_server_self(100, "v".into()).await.unwrap();
        insert(
            &db,
            TaskInsert {
                id: "owned".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(100),
            },
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let (sched, spawner) = mk_sched(db.clone(), 200, cancel.clone()).await;
        let mut js = JoinSet::new();
        sched.scan_and_claim_orphans(&mut js).await.unwrap();
        cancel.cancel();
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn race_two_schedulers_only_one_claims() {
        let db = fresh_db().await;
        insert(
            &db,
            TaskInsert {
                id: "race".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(999),
            },
        )
        .await
        .unwrap();
        let c1 = CancellationToken::new();
        let c2 = CancellationToken::new();
        let (s1, sp1) = mk_sched(db.clone(), 1, c1.clone()).await;
        let (s2, sp2) = mk_sched(db.clone(), 2, c2.clone()).await;
        let (mut js1, mut js2) = (JoinSet::new(), JoinSet::new());
        s1.scan_and_claim_orphans(&mut js1).await.unwrap();
        s2.scan_and_claim_orphans(&mut js2).await.unwrap();
        let total = sp1.spawned.load(Ordering::SeqCst) + sp2.spawned.load(Ordering::SeqCst);
        assert_eq!(total, 1, "expected exactly one claimer, got {total}");
        c1.cancel();
        c2.cancel();
    }

    #[tokio::test]
    async fn run_dispatches_for_inserted_id() {
        let db = fresh_db().await;
        let (tx, rx) = Scheduler::channel();
        insert(
            &db,
            TaskInsert {
                id: "live".into(),
                kind: TaskKind::BatchFetch,
                params_json: "{}".into(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        tx.send(TaskId("live".into())).unwrap();
        drop(tx);
        let cancel = CancellationToken::new();
        let spawner = Arc::new(RecordingSpawner::default());
        let sched = Scheduler {
            db: db.clone(),
            cfg: SchedulerConfig {
                own_pid: 1,
                orphan_scan_interval: Duration::from_millis(500),
                shutdown_grace: Duration::from_millis(100),
            },
            cancel: cancel.clone(),
            new_task_rx: rx,
            spawner: spawner.clone(),
        };
        let handle = tokio::spawn(sched.run());
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
    }
}

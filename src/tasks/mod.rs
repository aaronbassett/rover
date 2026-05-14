//! Long-running task subsystem.
//!
//! See `docs/superpowers/specs/2026-05-14-rover-m6-tasks-batching-design.md`.

pub mod batch_fetch;
pub mod error;
pub mod retry;
pub mod revalidate;
pub mod scheduler;
pub mod summarize;
pub mod types;

pub use error::TasksError;
pub use scheduler::{NewTaskSender, Scheduler};
pub use types::{
    BatchFetchParams, BatchFetchResult, CoreEvent, RetryParams, RevalidateParams, TaskId, TaskKind,
    TaskStatus,
};

use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::storage::Db;

/// Default production dispatch table. Routes by `TaskKind`. Tasks 9/11
/// replace the `Retry` / `Revalidate` arms with their real worker calls.
pub struct DefaultSpawner {
    pub batch_deps: batch_fetch::BatchDeps,
    pub retry_deps: retry::RetryDeps,
}

impl scheduler::WorkerSpawner for DefaultSpawner {
    fn spawn(
        &self,
        join_set: &mut JoinSet<()>,
        db: Db,
        task_id: TaskId,
        kind: TaskKind,
        cancel: CancellationToken,
    ) {
        match kind {
            TaskKind::Summarize => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            TaskKind::BatchFetch => {
                let deps = self.batch_deps.clone();
                join_set.spawn(batch_fetch::run(deps, db, task_id, cancel));
            }
            TaskKind::Retry => {
                let deps = self.retry_deps.clone();
                join_set.spawn(retry::run(deps, db, task_id, cancel));
            }
            // ↓ replaced in Task 11
            TaskKind::Revalidate => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
        }
    }
}

pub fn default_spawner(
    batch_deps: batch_fetch::BatchDeps,
    retry_deps: retry::RetryDeps,
) -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner {
        batch_deps,
        retry_deps,
    })
}

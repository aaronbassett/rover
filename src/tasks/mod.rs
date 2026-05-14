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

/// Default production dispatch table. Routes by `TaskKind`. Tasks 7/9/11
/// replace the `BatchFetch` / `Retry` / `Revalidate` arms with their real
/// worker calls.
pub struct DefaultSpawner;

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
            // ↓ replaced wholesale in Task 7
            TaskKind::BatchFetch => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced wholesale in Task 9
            TaskKind::Retry => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
            // ↓ replaced wholesale in Task 11
            TaskKind::Revalidate => {
                join_set.spawn(summarize::run(db, task_id, cancel));
            }
        }
    }
}

pub fn default_spawner() -> Arc<dyn scheduler::WorkerSpawner> {
    Arc::new(DefaultSpawner)
}

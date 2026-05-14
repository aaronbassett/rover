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

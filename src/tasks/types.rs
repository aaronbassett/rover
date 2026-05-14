//! Public types shared between the scheduler, workers, and the CLI.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::storage::tasks::{TaskKind, TaskStatus};

/// Bare UUIDv7 task identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(s)?;
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Core event kinds shared by every worker (per design spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreEvent {
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
}

impl CoreEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskStarted => "task_started",
            Self::TaskCompleted => "task_completed",
            Self::TaskFailed => "task_failed",
            Self::TaskCancelled => "task_cancelled",
        }
    }
}

/// `batch_fetch` params stored in `tasks.params_json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchFetchParams {
    pub urls: Vec<String>,
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    #[serde(default = "default_per_domain")]
    pub per_domain_concurrency: u32,
    #[serde(default)]
    pub force_refresh: bool,
}

fn default_concurrency() -> u32 {
    8
}
fn default_per_domain() -> u32 {
    2
}

/// `retry` params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryParams {
    pub url: String,
    pub attempt: u8,
    pub wait_ms_initial: u64,
    pub max_attempts: u8,
    #[serde(default)]
    pub parent_task_id: Option<String>,
}

/// `revalidate` params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevalidateParams {
    pub url: String,
    #[serde(default)]
    pub etag_at_serve: Option<String>,
    #[serde(default)]
    pub last_modified_at_serve: Option<String>,
}

/// Rollup written to `tasks.result_json` when a `batch_fetch` completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchFetchResult {
    pub total: u32,
    pub succeeded: u32,
    pub failed: u32,
    pub duration_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_is_uuid_v7_string() {
        let id = TaskId::new();
        let parsed = Uuid::parse_str(id.as_str()).unwrap();
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn task_id_parse_roundtrip() {
        let id = TaskId::new();
        let again = TaskId::parse(id.as_str()).unwrap();
        assert_eq!(id, again);
    }

    #[test]
    fn task_id_parse_rejects_garbage() {
        assert!(TaskId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn batch_fetch_params_defaults() {
        let v: BatchFetchParams = serde_json::from_str(r#"{"urls":["a"]}"#).unwrap();
        assert_eq!(v.concurrency, 8);
        assert_eq!(v.per_domain_concurrency, 2);
        assert!(!v.force_refresh);
    }

    #[test]
    fn core_event_as_str_table() {
        assert_eq!(CoreEvent::TaskStarted.as_str(), "task_started");
        assert_eq!(CoreEvent::TaskFailed.as_str(), "task_failed");
    }
}

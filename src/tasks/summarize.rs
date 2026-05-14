//! `summarize` stub worker.
//!
//! Always fails with `summarization_not_yet_implemented`. The real
//! summarizer lands in M7. The stub exists so the `tasks.kind` schema is
//! final in M6 and the scheduler has a concrete worker to dispatch.

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskStatus, set_status};
use crate::tasks::types::TaskId;

pub async fn run(db: Db, task_id: TaskId, _cancel: CancellationToken) {
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"summarize"}).to_string(),
        },
    )
    .await;
    let payload = json!({
        "error": "summarization_not_yet_implemented",
        "message": "Summarization will be implemented in M7.",
        "duration_ms": 0,
    });
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_failed".into(),
            payload_json: payload.to_string(),
        },
    )
    .await;
    let _ = set_status(
        &db,
        task_id.as_str(),
        TaskStatus::Failed,
        None,
        Some("summarization_not_yet_implemented".into()),
    )
    .await;
}

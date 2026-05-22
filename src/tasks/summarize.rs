//! `summarize` stub worker — schema-only.
//!
//! M7 implements all summarization synchronously through the
//! `SummarizerService`. No new task rows of `kind = "summarize"` are
//! ever inserted by M7. The worker remains because the M6 schema's
//! `tasks.kind` CHECK constraint includes "summarize" and removing it
//! would require a migration; the worker safely errors any pre-M7
//! row that somehow gets reclaimed with
//! `summarize_no_longer_a_task_kind` (defense-in-depth).

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskStatus, set_status};
use crate::tasks::types::{CoreEvent, TaskId};

pub async fn run(db: Db, task_id: TaskId, _cancel: CancellationToken) {
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: CoreEvent::TaskStarted.as_str().into(),
            payload_json: json!({"kind":"summarize"}).to_string(),
        },
    )
    .await;
    let payload = json!({
        "error": "summarize_no_longer_a_task_kind",
        "message": "summarize is no longer a task kind in M7; all summarization is synchronous via the SummarizerService.",
        "duration_ms": 0,
    });
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: CoreEvent::TaskFailed.as_str().into(),
            payload_json: payload.to_string(),
        },
    )
    .await;
    let _ = set_status(
        &db,
        task_id.as_str(),
        TaskStatus::Failed,
        None,
        Some("summarize_no_longer_a_task_kind".into()),
    )
    .await;
}

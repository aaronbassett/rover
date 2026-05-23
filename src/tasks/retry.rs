//! `retry` worker — long-deferred retries scheduled by the fetcher.

use std::time::{Duration, Instant};

use serde_json::json;
use tokio::time;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{
    TaskInsert, TaskKind, TaskStatus, get, insert, is_cancelled, set_status,
};
use crate::tasks::deps::WorkerDeps;
use crate::tasks::types::{CoreEvent, RetryParams, TaskId};

const RETRY_WAIT_CAP_MS: u64 = 5 * 60 * 1000;

pub async fn run(deps: WorkerDeps, db: Db, task_id: TaskId, cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: RetryParams = match serde_json::from_str(&row.params_json) {
        Ok(p) => p,
        Err(e) => {
            terminal_fail(&db, task_id.as_str(), "invalid_params", &e.to_string(), 0).await;
            return;
        }
    };
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: CoreEvent::TaskStarted.as_str().into(),
            payload_json: json!({"kind":"retry","attempt": params.attempt}).to_string(),
        },
    )
    .await;
    let url = match Url::parse(&params.url) {
        Ok(u) => u,
        Err(e) => {
            terminal_fail(
                &db,
                task_id.as_str(),
                "invalid_url",
                &e.to_string(),
                started.elapsed().as_millis() as i64,
            )
            .await;
            return;
        }
    };

    let shift = (params.attempt.saturating_sub(1) as u64).min(8);
    let wait_ms = params
        .wait_ms_initial
        .saturating_mul(1u64 << shift)
        .min(RETRY_WAIT_CAP_MS);
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "retry_attempted".into(),
            payload_json: json!({
                "url": params.url,
                "attempt": params.attempt,
                "wait_ms_used": wait_ms,
            })
            .to_string(),
        },
    )
    .await;
    // Wait — but bail early on cancellation.
    let wait = time::sleep(Duration::from_millis(wait_ms));
    tokio::select! {
        _ = wait => {}
        _ = cancel.cancelled() => {
            cancelled_terminal(&db, task_id.as_str(), started.elapsed().as_millis() as i64).await;
            return;
        }
    }
    if is_cancelled(&db, task_id.as_str()).await.unwrap_or(false) {
        cancelled_terminal(&db, task_id.as_str(), started.elapsed().as_millis() as i64).await;
        return;
    }

    let res = fetch_with_cache(
        &db,
        &deps.client,
        &deps.pacer,
        &deps.rate_cfg,
        &deps.robots_cfg,
        &url,
        &deps.cache_cfg,
        FetchOptions {
            force_refresh: true,
            ssrf_level: deps.ssrf_level,
            ssrf_project_root: deps.ssrf_project_root.clone(),
            ignore_robots: !deps.robots_cfg.respect,
            user_agent: deps.fetch_cfg.user_agent.clone(),
        },
        |body, base| {
            let extracted =
                extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
            Ok(ExtractResult {
                title: extracted.title.clone(),
                content_hash: sha256_hex(extracted.body_md.as_bytes()),
                body_md: extracted.body_md,
                metadata: extracted.metadata,
            })
        },
    )
    .await;

    let duration_ms = started.elapsed().as_millis() as i64;
    match res {
        Ok(_) => {
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "retry_succeeded".into(),
                    payload_json: json!({"url": params.url, "attempt": params.attempt}).to_string(),
                },
            )
            .await;
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: CoreEvent::TaskCompleted.as_str().into(),
                    payload_json: json!({"duration_ms": duration_ms}).to_string(),
                },
            )
            .await;
            let _ = set_status(&db, task_id.as_str(), TaskStatus::Completed, None, None).await;
        }
        Err(e) => {
            let exhausted = params.attempt >= params.max_attempts;
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "retry_failed".into(),
                    payload_json: json!({
                        "url": params.url,
                        "attempt": params.attempt,
                        "error": e.to_string(),
                        "will_retry": !exhausted,
                    })
                    .to_string(),
                },
            )
            .await;
            if exhausted {
                terminal_fail(
                    &db,
                    task_id.as_str(),
                    "retries_exhausted",
                    &e.to_string(),
                    duration_ms,
                )
                .await;
            } else {
                // Chain a new retry task for attempt+1.
                let next_wait = wait_ms.saturating_mul(2).min(RETRY_WAIT_CAP_MS);
                let next = RetryParams {
                    url: params.url.clone(),
                    attempt: params.attempt + 1,
                    wait_ms_initial: next_wait,
                    max_attempts: params.max_attempts,
                    parent_task_id: params.parent_task_id.clone(),
                };
                let new_id = uuid::Uuid::now_v7().to_string();
                let _ = insert(
                    &db,
                    TaskInsert {
                        id: new_id.clone(),
                        kind: TaskKind::Retry,
                        params_json: serde_json::to_string(&next).unwrap_or_else(|_| "{}".into()),
                        owner_pid: Some(std::process::id() as i64),
                    },
                )
                .await;
                // This task completes successfully — the *attempt* failed but
                // the *task* successfully handed off to the next attempt.
                let _ = append(
                    &db,
                    EventInsert {
                        task_id: task_id.as_str().to_string(),
                        kind: CoreEvent::TaskCompleted.as_str().into(),
                        payload_json: json!({
                            "chained_next_task_id": new_id,
                            "duration_ms": duration_ms,
                        })
                        .to_string(),
                    },
                )
                .await;
                let _ = set_status(&db, task_id.as_str(), TaskStatus::Completed, None, None).await;
            }
        }
    }
}

async fn terminal_fail(db: &Db, task_id: &str, slug: &str, message: &str, duration_ms: i64) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: CoreEvent::TaskFailed.as_str().into(),
            payload_json: json!({
                "error": slug,
                "message": message,
                "duration_ms": duration_ms,
            })
            .to_string(),
        },
    )
    .await;
    let _ = set_status(
        db,
        task_id,
        TaskStatus::Failed,
        None,
        Some(slug.to_string()),
    )
    .await;
}

async fn cancelled_terminal(db: &Db, task_id: &str, duration_ms: i64) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: CoreEvent::TaskCancelled.as_str().into(),
            payload_json: json!({"at": "during_wait", "duration_ms": duration_ms}).to_string(),
        },
    )
    .await;
    let _ = set_status(db, task_id, TaskStatus::Cancelled, None, None).await;
}

//! `revalidate` worker — refreshes a stale cache entry in the background.

use std::time::Instant;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{
    CacheStatus, ExtractResult, FetchOptions, fetch_with_cache, sha256_hex,
};
use crate::storage::Db;
use crate::storage::events::{EventInsert, append};
use crate::storage::tasks::{TaskStatus, get, set_status};
use crate::tasks::deps::WorkerDeps;
use crate::tasks::types::{CoreEvent, RevalidateParams, TaskId};

pub async fn run(deps: WorkerDeps, db: Db, task_id: TaskId, _cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: RevalidateParams = match serde_json::from_str(&row.params_json) {
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
            payload_json: json!({"kind":"revalidate"}).to_string(),
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
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "revalidation_started".into(),
            payload_json: json!({"url": params.url}).to_string(),
        },
    )
    .await;
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
            har_recorder: deps.har_recorder.clone(),
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
        Ok(cf) => {
            let changed = matches!(cf.cache_status, CacheStatus::Miss);
            let _ = append(
                &db,
                EventInsert {
                    task_id: task_id.as_str().to_string(),
                    kind: "revalidation_completed".into(),
                    payload_json: json!({
                        "url": params.url,
                        "changed": changed,
                        "status_code": if changed { 200 } else { 304 },
                    })
                    .to_string(),
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
            terminal_fail(
                &db,
                task_id.as_str(),
                "revalidation_failed",
                &e.to_string(),
                duration_ms,
            )
            .await;
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

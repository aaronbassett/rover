//! `batch_fetch` worker.
//!
//! Pulls a `BatchFetchParams` row, fans out via `fetch_with_cache`, emits
//! one `item_started`/`item_done`/`item_failed` event per URL plus the
//! shared `task_started`/`task_completed` envelope. Cancellation is checked
//! between items. On orphan claim, items already represented in
//! `task_events` are skipped so resumption is idempotent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{CacheConfig, FetchConfig, RateLimitConfig, RobotsConfig};
use crate::extractor::pipeline::extract;
use crate::fetcher::FetcherError;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache};
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::events::{EventInsert, append, range_since};
use crate::storage::tasks::{TaskStatus, get, is_cancelled, set_status};
use crate::tasks::types::{BatchFetchParams, BatchFetchResult, TaskId};

/// Dependencies needed by the worker. Built once by `mcp::server` and shared
/// across every batch worker via `Arc`.
#[derive(Clone)]
pub struct BatchDeps {
    pub client: reqwest::Client,
    pub pacer: Arc<Pacer>,
    pub cache_cfg: CacheConfig,
    pub rate_cfg: RateLimitConfig,
    pub robots_cfg: RobotsConfig,
    pub fetch_cfg: FetchConfig,
    pub ssrf_level: SsrfLevel,
}

/// Classify a fetcher error as a "deferred" failure that warrants a follow-up
/// retry task. `FetcherError::Deferred` is produced by `with_retries` when a
/// long `Retry-After` is converted into an out-of-band retry task.
fn is_deferred_error(e: &FetcherError) -> bool {
    matches!(e, FetcherError::Deferred { .. })
}

/// Return the set of `index` values already represented as `item_done` or
/// `item_failed` events. Used on startup / orphan claim to skip work that
/// already happened.
async fn already_processed_indices(db: &Db, task_id: &str) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut cursor = 0i64;
    loop {
        let rows = match range_since(db, task_id, cursor, 1000).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(target: "rover::tasks::batch_fetch", error = ?e, "scan events failed");
                return seen;
            }
        };
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            if r.kind == "item_done" || r.kind == "item_failed" {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.payload_json) {
                    if let Some(idx) = v.get("index").and_then(|x| x.as_u64()) {
                        seen.insert(idx as u32);
                    }
                }
            }
        }
        cursor = rows.last().map(|r| r.id).unwrap_or(cursor);
        if rows.len() < 1000 {
            break;
        }
    }
    seen
}

/// Worker entry point used by `DefaultSpawner`.
pub async fn run(deps: BatchDeps, db: Db, task_id: TaskId, cancel: CancellationToken) {
    let started = Instant::now();
    let row = match get(&db, task_id.as_str()).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let params: BatchFetchParams = match serde_json::from_str(&row.params_json) {
        Ok(p) => p,
        Err(e) => {
            emit_terminal_failure(&db, task_id.as_str(), "invalid_params", &e.to_string(), 0).await;
            return;
        }
    };

    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "task_started".into(),
            payload_json: json!({"kind":"batch_fetch","total":params.urls.len()}).to_string(),
        },
    )
    .await;
    let _ = append(
        &db,
        EventInsert {
            task_id: task_id.as_str().to_string(),
            kind: "batch_start".into(),
            payload_json: json!({"total": params.urls.len()}).to_string(),
        },
    )
    .await;

    let seen = already_processed_indices(&db, task_id.as_str()).await;
    let global = Arc::new(Semaphore::new(params.concurrency.max(1) as usize));
    let per_host: Arc<Mutex<HashMap<String, Arc<Semaphore>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let mut handles = Vec::new();
    for (index, url_str) in params.urls.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        if let Ok(true) = is_cancelled(&db, task_id.as_str()).await {
            break;
        }
        let idx = index as u32;
        if seen.contains(&idx) {
            continue;
        }
        let url = match Url::parse(url_str) {
            Ok(u) => u,
            Err(e) => {
                let _ = append(
                    &db,
                    EventInsert {
                        task_id: task_id.as_str().to_string(),
                        kind: "item_failed".into(),
                        payload_json: json!({
                            "index": idx,
                            "url": url_str,
                            "error": e.to_string(),
                            "will_retry": false,
                        })
                        .to_string(),
                    },
                )
                .await;
                continue;
            }
        };
        let host = url.host_str().unwrap_or("").to_string();
        let host_sem: Arc<Semaphore> = {
            let mut map = per_host.lock().await;
            map.entry(host.clone())
                .or_insert_with(|| {
                    Arc::new(Semaphore::new(params.per_domain_concurrency.max(1) as usize))
                })
                .clone()
        };
        let deps_c = deps.clone();
        let db_c = db.clone();
        let task_str = task_id.as_str().to_string();
        let global_c = global.clone();
        let url_clone = url.clone();
        let url_string = url_str.clone();
        let force_refresh = params.force_refresh;
        let cancel_c = cancel.clone();
        let handle = tokio::spawn(async move {
            let _gh = host_sem.acquire_owned().await.expect("host sem closed");
            let _gg = global_c.acquire_owned().await.expect("global sem closed");
            // Re-check cancellation after acquiring the permit so already-queued
            // futures don't fire after the caller asked us to stop.
            if cancel_c.is_cancelled() || is_cancelled(&db_c, &task_str).await.unwrap_or(false) {
                return;
            }
            let _ = append(
                &db_c,
                EventInsert {
                    task_id: task_str.clone(),
                    kind: "item_started".into(),
                    payload_json: json!({"index": idx, "url": url_string}).to_string(),
                },
            )
            .await;
            let item_started = Instant::now();
            let res = fetch_with_cache(
                &db_c,
                &deps_c.client,
                &deps_c.pacer,
                &deps_c.rate_cfg,
                &deps_c.robots_cfg,
                &url_clone,
                &deps_c.cache_cfg,
                FetchOptions {
                    force_refresh,
                    ssrf_level: deps_c.ssrf_level,
                    ignore_robots: !deps_c.robots_cfg.respect,
                    user_agent: deps_c.fetch_cfg.user_agent.clone(),
                },
                |body, base| {
                    let extracted =
                        extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
                    Ok(ExtractResult {
                        title: extracted.title.clone(),
                        content_hash: crate::fetcher::cached::sha256_hex(
                            extracted.body_md.as_bytes(),
                        ),
                        body_md: extracted.body_md,
                        metadata: extracted.metadata,
                    })
                },
            )
            .await;
            let dur = item_started.elapsed().as_millis() as i64;
            let (event_kind, payload) = match res {
                Ok(cf) => {
                    let tokens: Option<usize> = serde_json::from_str::<serde_json::Value>(
                        cf.page.metadata_json.as_deref().unwrap_or("{}"),
                    )
                    .ok()
                    .and_then(|v| v.get("token_count").and_then(|x| x.as_u64()))
                    .map(|n| n as usize);
                    (
                        "item_done",
                        json!({
                            "index": idx,
                            "url": url_string,
                            "tokens": tokens,
                            "cached": matches!(
                                cf.cache_status,
                                crate::fetcher::cached::CacheStatus::Hit
                            ),
                            "duration_ms": dur,
                        }),
                    )
                }
                Err(e) => {
                    let will_retry = is_deferred_error(&e);
                    (
                        "item_failed",
                        json!({
                            "index": idx,
                            "url": url_string,
                            "error": e.to_string(),
                            "will_retry": will_retry,
                            "duration_ms": dur,
                        }),
                    )
                }
            };
            let _ = append(
                &db_c,
                EventInsert {
                    task_id: task_str,
                    kind: event_kind.into(),
                    payload_json: payload.to_string(),
                },
            )
            .await;
        });
        handles.push(handle);
    }
    for h in handles {
        let _ = h.await;
    }

    // Final rollup: tally from events (including resumed items).
    let counts = crate::storage::events::count_by_kind(&db, task_id.as_str())
        .await
        .unwrap_or_default();
    let succeeded: u32 = counts
        .iter()
        .find_map(|(k, n)| {
            if k == "item_done" {
                Some(*n as u32)
            } else {
                None
            }
        })
        .unwrap_or(0);
    let failed: u32 = counts
        .iter()
        .find_map(|(k, n)| {
            if k == "item_failed" {
                Some(*n as u32)
            } else {
                None
            }
        })
        .unwrap_or(0);

    let cancelled_now =
        cancel.is_cancelled() || is_cancelled(&db, task_id.as_str()).await.unwrap_or(false);
    let duration_ms = started.elapsed().as_millis() as i64;
    let result = BatchFetchResult {
        total: params.urls.len() as u32,
        succeeded,
        failed,
        duration_ms,
    };
    if cancelled_now {
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "task_cancelled".into(),
                payload_json: json!({"at": "between_items", "duration_ms": duration_ms})
                    .to_string(),
            },
        )
        .await;
        let _ = set_status(
            &db,
            task_id.as_str(),
            TaskStatus::Cancelled,
            Some(serde_json::to_string(&result).unwrap()),
            None,
        )
        .await;
    } else {
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "final".into(),
                payload_json: json!({
                    "succeeded": succeeded,
                    "failed": failed,
                    "duration_s": (duration_ms as f64) / 1000.0,
                })
                .to_string(),
            },
        )
        .await;
        let _ = append(
            &db,
            EventInsert {
                task_id: task_id.as_str().to_string(),
                kind: "task_completed".into(),
                payload_json: json!({"result": result, "duration_ms": duration_ms}).to_string(),
            },
        )
        .await;
        let _ = set_status(
            &db,
            task_id.as_str(),
            TaskStatus::Completed,
            Some(serde_json::to_string(&result).unwrap()),
            None,
        )
        .await;
    }
}

async fn emit_terminal_failure(
    db: &Db,
    task_id: &str,
    error_slug: &str,
    message: &str,
    duration_ms: i64,
) {
    let _ = append(
        db,
        EventInsert {
            task_id: task_id.to_string(),
            kind: "task_failed".into(),
            payload_json: json!({
                "error": error_slug,
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
        Some(error_slug.to_string()),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tasks::{TaskInsert, TaskKind, insert};
    use tempfile::tempdir;

    async fn fresh_db_with_batch(id: &str, urls: &[&str]) -> Db {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        std::mem::forget(tmp);
        let params = BatchFetchParams {
            urls: urls.iter().map(|s| s.to_string()).collect(),
            concurrency: 2,
            per_domain_concurrency: 1,
            force_refresh: false,
        };
        insert(
            &db,
            TaskInsert {
                id: id.into(),
                kind: TaskKind::BatchFetch,
                params_json: serde_json::to_string(&params).unwrap(),
                owner_pid: Some(1),
            },
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn already_processed_collects_done_and_failed_indices() {
        let db = fresh_db_with_batch("t1", &["a", "b", "c"]).await;
        append(
            &db,
            EventInsert {
                task_id: "t1".into(),
                kind: "task_started".into(),
                payload_json: "{}".into(),
            },
        )
        .await
        .unwrap();
        append(
            &db,
            EventInsert {
                task_id: "t1".into(),
                kind: "item_started".into(),
                payload_json: r#"{"index":0,"url":"a"}"#.into(),
            },
        )
        .await
        .unwrap();
        append(
            &db,
            EventInsert {
                task_id: "t1".into(),
                kind: "item_done".into(),
                payload_json: r#"{"index":0,"url":"a"}"#.into(),
            },
        )
        .await
        .unwrap();
        append(
            &db,
            EventInsert {
                task_id: "t1".into(),
                kind: "item_failed".into(),
                payload_json: r#"{"index":2,"url":"c"}"#.into(),
            },
        )
        .await
        .unwrap();
        let seen = already_processed_indices(&db, "t1").await;
        assert!(seen.contains(&0));
        assert!(seen.contains(&2));
        assert!(!seen.contains(&1));
    }
}

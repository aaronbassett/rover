//! `rover task <id>` body (also drives `rover batch <id>` via `expect_kind`).
//!
//! Pure reader except for `--cancel`, which is a single UPDATE. Opens the
//! cache database directly and queries `tasks` + `task_events`. No HTTP, no
//! scheduler, no server-process responsibilities.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow};
use jiff::Timestamp;

use crate::config;
use crate::storage::Db;
use crate::storage::events::{EventRow, count_by_kind, last_for_task, range_since};
use crate::storage::tasks::{TaskKind, TaskStatus, get, set_cancellation_requested};

/// Arguments shared by `rover task <id>` and `rover batch <id>`.
pub struct Args {
    pub id: String,
    pub monitor: bool,
    pub cancel: bool,
    pub format: OutputFormat,
    pub from_event: Option<i64>,
    /// If `Some`, the loaded task's `kind` must match this string or the
    /// command errors out. `rover batch` sets this to `Some("batch_fetch")`;
    /// `rover task` leaves it `None`.
    pub expect_kind: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Human,
    Ndjson,
}

/// Entry point used by `main.rs`. Locks stdout and delegates to
/// [`run_with_writers`], which is the testable seam.
pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    run_with_writers(args, config_path, &mut out).await
}

/// Library entry point used by integration tests. Accepts any `Write`
/// implementation so the test suite can capture output without going through
/// a subprocess.
pub async fn run_with_writers<W: Write>(
    args: Args,
    config_path: Option<&Path>,
    out: &mut W,
) -> anyhow::Result<()> {
    let _cfg = config::load(config_path).context("loading config")?;
    let data_dir = crate::paths::data_dir();
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let row = get(&db, &args.id)
        .await?
        .ok_or_else(|| anyhow!("task {} not found", args.id))?;
    if let Some(want) = args.expect_kind {
        if row.kind.as_str() != want {
            return Err(anyhow!(
                "task {} is kind={}, expected {}",
                args.id,
                row.kind.as_str(),
                want
            ));
        }
    }

    if args.cancel {
        let changed = set_cancellation_requested(&db, &args.id).await?;
        if changed {
            writeln!(out, "Cancellation requested for {}.", args.id)?;
        } else if row.cancellation_requested {
            writeln!(out, "Cancellation already requested for {}.", args.id)?;
        } else {
            writeln!(
                out,
                "Task {} is in a terminal state; nothing to cancel.",
                args.id
            )?;
        }
        return Ok(());
    }

    if args.monitor {
        return monitor_loop(&db, &args, out).await;
    }
    print_snapshot(&db, &args, &row, out).await
}

async fn print_snapshot<W: Write>(
    db: &Db,
    args: &Args,
    row: &crate::storage::tasks::TaskRow,
    out: &mut W,
) -> anyhow::Result<()> {
    let liveness = check_liveness(db).await?;
    let now_ms = Timestamp::now().as_millisecond();
    let counts = count_by_kind(db, &args.id).await?;
    let succeeded = counts
        .iter()
        .find(|(k, _)| k == "item_done")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let failed = counts
        .iter()
        .find(|(k, _)| k == "item_failed")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let started = counts
        .iter()
        .find(|(k, _)| k == "item_started")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let total: i64 = if row.kind == TaskKind::BatchFetch {
        serde_json::from_str::<serde_json::Value>(&row.params_json)
            .ok()
            .and_then(|v| {
                v.get("urls")
                    .and_then(|u| u.as_array())
                    .map(|a| a.len() as i64)
            })
            .unwrap_or(0)
    } else {
        0
    };
    let in_flight = (started - succeeded - failed).max(0);
    let last = last_for_task(db, &args.id).await?;

    match args.format {
        OutputFormat::Ndjson => {
            let snap = serde_json::json!({
                "ts": rfc3339(now_ms),
                "kind": "snapshot",
                "task_id": row.id,
                "task_kind": row.kind.as_str(),
                "status": row.status.as_str(),
                "total": total,
                "succeeded": succeeded,
                "failed": failed,
                "in_flight": in_flight,
                "completed": succeeded + failed,
                "started_at": rfc3339(row.created_at),
                "last_event_id": last.as_ref().map(|e| e.id),
                "eta_s": eta_seconds(succeeded, total, row.created_at, now_ms),
            });
            writeln!(out, "{snap}")?;
        }
        OutputFormat::Human => {
            if let Some(warn) = liveness {
                writeln!(out, "{warn}")?;
            }
            if row.kind == TaskKind::BatchFetch {
                writeln!(out, "Batch {} — {}", row.id, row.status.as_str())?;
            } else {
                writeln!(
                    out,
                    "Task {} — {} (kind: {})",
                    row.id,
                    row.status.as_str(),
                    row.kind.as_str()
                )?;
            }
            writeln!(out, "Started {}", relative_human(now_ms - row.created_at))?;
            if row.kind == TaskKind::BatchFetch && total > 0 {
                let pct = (succeeded + failed) * 100 / total;
                writeln!(
                    out,
                    "Progress: {}/{} ({}%)  ✓ {}  ✗ {}  ⋯ {} in flight",
                    succeeded + failed,
                    total,
                    pct,
                    succeeded,
                    failed,
                    in_flight,
                )?;
                if let Some(eta) = eta_seconds(succeeded, total, row.created_at, now_ms) {
                    writeln!(out, "ETA ~{eta}s")?;
                }
            }
            if let Some(ev) = last.as_ref() {
                writeln!(
                    out,
                    "Last event: {} ({})",
                    summarise_event(ev),
                    relative_human(now_ms - ev.ts)
                )?;
            }
            if !row.status.is_terminal() {
                writeln!(out, "Tip: use `rover task {} --cancel` to stop.", row.id)?;
            }
        }
    }
    Ok(())
}

async fn monitor_loop<W: Write>(db: &Db, args: &Args, out: &mut W) -> anyhow::Result<()> {
    let mut last_seen = args.from_event.unwrap_or(0);
    loop {
        let rows = range_since(db, &args.id, last_seen, 1000).await?;
        for r in &rows {
            emit_wire_line(r, out)?;
            last_seen = r.id;
        }
        if rows.is_empty() {
            let row = get(db, &args.id)
                .await?
                .ok_or_else(|| anyhow!("task {} disappeared", args.id))?;
            if row.status.is_terminal() {
                // Drain one more time in case the terminal event was written
                // between our SELECT and now.
                let extras = range_since(db, &args.id, last_seen, 1000).await?;
                for r in &extras {
                    emit_wire_line(r, out)?;
                }
                return match row.status {
                    TaskStatus::Completed => Ok(()),
                    TaskStatus::Failed => Err(anyhow!("task failed")),
                    TaskStatus::Cancelled => Err(anyhow!("task cancelled")),
                    _ => Ok(()),
                };
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

fn emit_wire_line<W: Write>(r: &EventRow, out: &mut W) -> anyhow::Result<()> {
    // DB payload is already a JSON object; merge with the envelope keys.
    let payload: serde_json::Value =
        serde_json::from_str(&r.payload_json).unwrap_or(serde_json::json!({}));
    let mut obj = serde_json::Map::new();
    obj.insert("ts".into(), serde_json::Value::String(rfc3339(r.ts)));
    obj.insert("kind".into(), serde_json::Value::String(r.kind.clone()));
    obj.insert("event_id".into(), serde_json::Value::Number(r.id.into()));
    if let serde_json::Value::Object(map) = payload {
        for (k, v) in map {
            obj.entry(k).or_insert(v);
        }
    }
    writeln!(out, "{}", serde_json::Value::Object(obj))?;
    Ok(())
}

async fn check_liveness(db: &Db) -> anyhow::Result<Option<String>> {
    let servers = db.list_servers().await?;
    let now_s = Timestamp::now().as_second();
    let warn = match servers.iter().map(|s| s.last_heartbeat).max() {
        None => Some("⚠ No `rover mcp` process appears to be alive.".to_string()),
        Some(hb) if now_s - hb > 30 => Some(format!(
            "⚠ Task is marked `running` but no `rover mcp` process appears to be alive (last heartbeat {}s ago).",
            now_s - hb,
        )),
        _ => None,
    };
    Ok(warn)
}

fn relative_human(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m{}s ago", s / 60, s % 60)
    } else {
        format!("{}h{}m ago", s / 3600, (s % 3600) / 60)
    }
}

fn rfc3339(ms: i64) -> String {
    let ts = Timestamp::from_millisecond(ms).unwrap_or_else(|_| Timestamp::now());
    ts.to_string()
}

fn eta_seconds(succeeded: i64, total: i64, started_ms: i64, now_ms: i64) -> Option<i64> {
    if succeeded < 3 || total == 0 {
        return None;
    }
    let elapsed_ms = now_ms - started_ms;
    if elapsed_ms <= 0 {
        return None;
    }
    let avg_per_item = elapsed_ms as f64 / succeeded as f64;
    let remaining = (total - succeeded).max(0) as f64;
    Some(((avg_per_item * remaining) / 1000.0) as i64)
}

fn summarise_event(r: &EventRow) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&r.payload_json).unwrap_or(serde_json::json!({}));
    if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
        format!("{} {}", r.kind, url)
    } else {
        r.kind.clone()
    }
}

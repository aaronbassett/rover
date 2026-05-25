//! `rover mcp` server lifecycle.
//!
//! Wires together: startup reap of stale `servers` rows, upsert of the
//! current process's row, a tokio interval heartbeat task, a SIGINT/SIGTERM
//! handler, and the rmcp stdio service.

use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::handler::RoverHandler;
use crate::storage::Db;
use crate::tasks::WorkerDeps;
use crate::tasks::default_spawner;
use crate::tasks::scheduler::{Scheduler, SchedulerConfig};

pub async fn serve_stdio(
    db: Db,
    config: Arc<Config>,
    ssrf_level: SsrfLevel,
    ssrf_project_root: Option<std::path::PathBuf>,
    har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
) -> anyhow::Result<()> {
    let pid = std::process::id() as i64;
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Startup reap: drop dead rows from prior crashes before claiming our own.
    let reaped = db.reap_stale_servers(config.mcp.reap_threshold).await?;
    if reaped > 0 {
        tracing::info!(
            target: "rover::mcp",
            reaped,
            "reaped stale servers rows on startup"
        );
    }

    db.upsert_server_self(pid, version.clone()).await?;
    tracing::info!(
        target: "rover::mcp",
        pid,
        version = %version,
        "rover mcp registered"
    );

    let cancel = CancellationToken::new();

    // Heartbeat task.
    {
        let db = db.clone();
        let interval = config.mcp.heartbeat_interval;
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(e) = db.heartbeat_server(pid).await {
                            tracing::warn!(target: "rover::mcp", error = ?e, "heartbeat failed");
                        } else {
                            tracing::trace!(target: "rover::mcp", "heartbeat");
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // Signal handler task.
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT");
            let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM");
            tokio::select! {
                _ = sigint.recv() => tracing::info!(target: "rover::mcp", "SIGINT received"),
                _ = sigterm.recv() => tracing::info!(target: "rover::mcp", "SIGTERM received"),
            }
            cancel.cancel();
        });
    }

    let client =
        crate::fetcher::client::build_http_client(&config.fetch.user_agent, config.fetch.timeout());
    let pacer = Arc::new(crate::fetcher::concurrency::Pacer::new(&config.rate_limit));

    // Build the in-process scheduler so MCP tools can hand off long-running
    // work (batch_fetch, retry, revalidate) to background workers.
    let (sched_task_tx, new_task_rx) = Scheduler::channel();

    // Storage → scheduler bridge. Every `storage::tasks::insert` (MCP tool,
    // fetcher SWR, deferred retry, retry chain) routes through the storage
    // notifier, which this task forwards into the scheduler's typed channel.
    // The bridge dies when the storage sender drops on `Db` teardown.
    let (storage_tx, mut storage_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    db.set_new_task_sender(storage_tx);
    let bridge_tx = sched_task_tx.clone();
    tokio::spawn(async move {
        while let Some(id) = storage_rx.recv().await {
            if bridge_tx.send(crate::tasks::types::TaskId(id)).is_err() {
                break;
            }
        }
    });
    let worker_deps = WorkerDeps {
        client: client.clone(),
        pacer: pacer.clone(),
        cache_cfg: config.cache.clone(),
        rate_cfg: config.rate_limit.clone(),
        robots_cfg: config.robots.clone(),
        fetch_cfg: config.fetch.clone(),
        ssrf_level,
        ssrf_project_root: ssrf_project_root.clone(),
        har_recorder: har_recorder.clone(),
    };
    let spawner = default_spawner(worker_deps);
    // The orphan scan interval is normally 10s, but integration tests need
    // to observe orphan reclaim within a few seconds, so they override via
    // `ROVER_ORPHAN_SCAN_MS` when the test-loopback build is active.
    let orphan_scan_interval = {
        #[cfg(feature = "test-loopback")]
        {
            std::env::var("ROVER_ORPHAN_SCAN_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_millis)
                .unwrap_or_else(|| SchedulerConfig::default().orphan_scan_interval)
        }
        #[cfg(not(feature = "test-loopback"))]
        {
            SchedulerConfig::default().orphan_scan_interval
        }
    };
    let sched = Scheduler {
        db: db.clone(),
        cfg: SchedulerConfig {
            own_pid: pid,
            orphan_scan_interval,
            ..SchedulerConfig::default()
        },
        cancel: cancel.clone(),
        new_task_rx,
        spawner,
    };
    let sched_handle = tokio::spawn(sched.run());

    // `sched_task_tx` is kept alive by the bridge spawn above; the handler
    // no longer holds its own sender (single source of truth: storage layer).
    let _ = sched_task_tx;

    // Build the summarizer service before the handler so the MCP tools
    // share a single registry/cache for the server's lifetime.
    let registry = Arc::new(
        crate::summarizer::registry::build(&config, config.tokenizer.default)
            .map_err(anyhow::Error::from)?,
    );
    let summarizer = Arc::new(crate::summarizer::SummarizerService::new(
        db.clone(),
        registry,
        config.summarization.fallback_to_extractive,
    ));

    // M9: build the captioner registry from `[captioners.*]` config. An
    // empty config yields an empty registry, which is fine: caption-mode
    // calls error at fetch time with `CaptionerNotConfigured`.
    let captioners = Arc::new(crate::vlm::build(&config).map_err(anyhow::Error::from)?);

    // Keep a clone for the shutdown flush below. The handler also holds a
    // clone for the foreground tools; background workers get yet another via
    // `WorkerDeps` above.
    let har_recorder_for_shutdown = har_recorder.clone();

    // M9 fix C1: lazily-initialized headless renderer. Pay the browser-launch
    // cost only when the first client request actually asks for headless
    // rendering. The `OnceCell` is shared between the handler (which inits
    // it on first use) and the shutdown path below (which calls `shutdown`
    // if the cell was populated).
    #[cfg(feature = "headless")]
    let headless_renderer: Arc<
        tokio::sync::OnceCell<Arc<crate::fetcher::headless::HeadlessRenderer>>,
    > = Arc::new(tokio::sync::OnceCell::new());
    #[cfg(feature = "headless")]
    let headless_renderer_for_shutdown = headless_renderer.clone();

    let handler = RoverHandler::new(
        db.clone(),
        config,
        client,
        ssrf_level,
        ssrf_project_root,
        har_recorder,
        pacer,
        summarizer,
        captioners,
        #[cfg(feature = "headless")]
        headless_renderer,
    );

    let service = handler.serve(stdio()).await?;

    // Wait until either the client closes the transport or a signal fires.
    // We wrap `service` in an `Option` so the cancel branch can drop it
    // explicitly — releasing the handler (and its `Arc` clone of the
    // headless `OnceCell`) before the renderer shutdown below.
    let mut service_holder = Some(service);
    tokio::select! {
        res = async {
            let s = service_holder.take().expect("service present");
            s.waiting().await
        } => {
            match res {
                Ok(reason) => tracing::info!(
                    target: "rover::mcp",
                    reason = ?reason,
                    "service loop ended"
                ),
                Err(e) => tracing::warn!(
                    target: "rover::mcp",
                    error = ?e,
                    "service task join error"
                ),
            }
        }
        _ = cancel.cancelled() => {
            tracing::info!(target: "rover::mcp", "shutting down on signal");
        }
    }
    // Drop the rmcp service explicitly (cancel-branch case) so the handler is
    // released before we try to take exclusive ownership of the renderer.
    drop(service_holder);

    // Make sure the heartbeat + signal tasks see the cancel before we
    // delete the row — otherwise the heartbeat can race and re-touch a
    // soon-to-be-deleted row.
    cancel.cancel();

    // Await the scheduler with a short deadline so a wedged worker can't
    // hang shutdown. The scheduler's own `shutdown_grace` already bounds the
    // join-set wait inside `run()`.
    match tokio::time::timeout(std::time::Duration::from_secs(5), sched_handle).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            tracing::warn!(target: "rover::mcp", error = ?e, "scheduler exited with error");
        }
        Ok(Err(e)) => {
            tracing::warn!(target: "rover::mcp", error = ?e, "scheduler task join error");
        }
        Err(_) => {
            tracing::warn!(target: "rover::mcp", "scheduler shutdown timed out");
        }
    }

    // Final HAR flush on shutdown so a clean client disconnect leaves a
    // complete file on disk (rather than depending on the next interval tick).
    if let Some(r) = &har_recorder_for_shutdown {
        if let Err(e) = r.flush().await {
            tracing::warn!(target: "rover::fetcher", error = ?e, "har shutdown flush failed");
        }
    }

    // M9 fix C1: stop the headless browser if it was ever launched. We've
    // already dropped `service` (which owns the handler holding the only
    // other strong reference to the OnceCell + inner Arc) above, so the
    // `try_unwrap` should normally succeed and let us cleanly close the
    // browser. If it doesn't, log and let the renderer's own destructor
    // close the underlying chromiumoxide handle.
    #[cfg(feature = "headless")]
    if let Some(renderer_arc) = headless_renderer_for_shutdown.get().cloned() {
        drop(headless_renderer_for_shutdown);
        match Arc::try_unwrap(renderer_arc) {
            Ok(renderer) => renderer.shutdown().await,
            Err(_still_shared) => {
                tracing::warn!(
                    target: "rover::mcp",
                    "headless renderer still has outstanding Arc references at shutdown; skipping explicit shutdown",
                );
            }
        }
    }

    db.delete_server_self(pid).await?;
    Ok(())
}

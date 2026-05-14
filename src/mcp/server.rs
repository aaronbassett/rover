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

pub async fn serve_stdio(db: Db, config: Arc<Config>, ssrf_level: SsrfLevel) -> anyhow::Result<()> {
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
    let handler = RoverHandler::new(db.clone(), config, client, ssrf_level, pacer);

    let service = handler.serve(stdio()).await?;

    // Wait until either the client closes the transport or a signal fires.
    tokio::select! {
        res = service.waiting() => {
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

    // Make sure the heartbeat + signal tasks see the cancel before we
    // delete the row — otherwise the heartbeat can race and re-touch a
    // soon-to-be-deleted row.
    cancel.cancel();

    db.delete_server_self(pid).await?;
    Ok(())
}

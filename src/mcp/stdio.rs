//! `rover mcp` over stdio — the default transport.

use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use crate::config::Config;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::TransportKind;
use crate::mcp::runtime::build_runtime;
use crate::storage::Db;

pub async fn serve_stdio(
    db: Db,
    config: Arc<Config>,
    ssrf_level: SsrfLevel,
    ssrf_project_root: Option<std::path::PathBuf>,
    har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
) -> anyhow::Result<()> {
    let runtime = build_runtime(
        db,
        config,
        ssrf_level,
        ssrf_project_root,
        har_recorder,
        TransportKind::Stdio,
    )
    .await?;

    let cancel = runtime.cancel.clone();
    let service = runtime.handler.clone().serve(stdio()).await?;

    // Wait until either the client closes the transport or a signal fires.
    // The service is wrapped in an Option so the cancel branch can drop it
    // explicitly, releasing its handler clone before `shutdown` runs.
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
    drop(service_holder);

    runtime.shutdown().await
}

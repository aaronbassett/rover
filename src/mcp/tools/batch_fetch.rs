//! MCP `batch_fetch` tool.
//!
//! Validates inputs, runs each URL through the SSRF policy, inserts a
//! `batch_fetch` task carrying serialised `BatchFetchParams`, sends the new
//! ID on the in-process MPSC, returns the immediate `TaskCreatedResponse`
//! envelope. SSRF rejects pre-empt the task insert.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::fetcher::ssrf::{SsrfError, validate_url};
use crate::mcp::envelope::TaskCreatedResponse;
use crate::mcp::error::McpError;
use crate::mcp::handler::RoverHandler;
use crate::storage::tasks::{TaskInsert, TaskKind, insert};
use crate::tasks::types::{BatchFetchParams, TaskId};

const MAX_URLS: usize = 100;
const MAX_CONCURRENCY: u32 = 32;
const MAX_PER_DOMAIN: u32 = 8;

/// Wire-side `batch_fetch` tool arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchFetchArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub force_refresh: bool,
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub per_domain_concurrency: Option<u32>,
}

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    pub async fn batch_fetch_inner(
        &self,
        args: BatchFetchArgs,
    ) -> Result<TaskCreatedResponse, McpError> {
        if args.urls.is_empty() {
            return Err(McpError::EmptyUrlList);
        }
        if args.urls.len() > MAX_URLS {
            return Err(McpError::TooManyUrls {
                count: args.urls.len(),
                max: MAX_URLS,
            });
        }
        for raw in &args.urls {
            let url = Url::parse(raw).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
            // Quick SSRF reject so we don't insert a task that will only fail.
            validate_url(&url, self.ssrf_level)
                .map_err(|e: SsrfError| McpError::Fetcher(crate::fetcher::FetcherError::Ssrf(e)))?;
        }
        let params = BatchFetchParams {
            urls: args.urls.clone(),
            concurrency: args.concurrency.unwrap_or(8).clamp(1, MAX_CONCURRENCY),
            per_domain_concurrency: args
                .per_domain_concurrency
                .unwrap_or(2)
                .clamp(1, MAX_PER_DOMAIN),
            force_refresh: args.force_refresh,
        };
        let id = TaskId::new();
        let params_json =
            serde_json::to_string(&params).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
        insert(
            &self.db,
            TaskInsert {
                id: id.as_str().to_string(),
                kind: TaskKind::BatchFetch,
                params_json,
                owner_pid: Some(std::process::id() as i64),
            },
        )
        .await?;
        // Notify scheduler. Failure to send means the channel is closed —
        // log + continue: the next orphan scan picks it up.
        if let Err(e) = self.new_task_tx.send(id.clone()) {
            tracing::warn!(target: "rover::mcp", error = ?e, "scheduler channel closed");
        }
        Ok(TaskCreatedResponse {
            task_id: id.as_str().to_string(),
            status: "running".into(),
            kind: "batch_fetch".into(),
            monitor_command: format!("rover batch {id} --monitor"),
            poll_command: format!("rover batch {id}"),
            cancel_command: format!("rover task {id} --cancel"),
            hint: "Use the Monitor tool with monitor_command for live updates, \
                   or call poll_command to check status."
                .into(),
        })
    }
}

//! Shared MCP server state.

use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router};

use crate::config::Config;
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::tools::count_tokens::CountTokensArgs;
use crate::mcp::tools::fetch::{FetchArgs, FetchOutput};
use crate::storage::Db;

/// State shared across all MCP tool invocations.
///
/// Note: this struct no longer carries its own scheduler `Sender`. Every
/// `storage::tasks::insert` call notifies the scheduler via the `Db`-owned
/// notifier installed by `mcp::server::serve_stdio`, so the MCP tool layer
/// has no extra wiring to do — it just inserts.
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) ssrf_level: SsrfLevel,
    /// Pre-canonicalized project root used when `ssrf_level == Project` to
    /// validate `file://` URLs. `None` for every other level.
    pub(crate) ssrf_project_root: Option<std::path::PathBuf>,
    /// Optional HAR recorder shared with background workers. `Some` when
    /// `[debug] har_path` is set in config.
    pub(crate) har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
    pub(crate) pacer: Arc<Pacer>,
    pub(crate) summarizer: Arc<crate::summarizer::SummarizerService>,
    /// M9: image captioner registry. Always present in default builds since
    /// cloud captioners ship in every binary; may be empty when the user
    /// hasn't configured any `[captioners.*]` blocks.
    pub(crate) captioners: Arc<crate::vlm::CaptionerRegistry>,
    /// Prompt-injection guard. Always present; default config yields the
    /// `moderate` output level with methods 1+2 active.
    #[allow(dead_code)]
    pub(crate) guard: std::sync::Arc<crate::guard::Guard>,
    /// M9 fix C1: lazily-initialized headless renderer. The handler owns a
    /// shared `OnceCell` so the first call requesting `headless.mode = On`
    /// (or `Auto` when the SPA heuristic triggers) pays the
    /// browser-launch cost; subsequent calls reuse the same `Arc<HeadlessRenderer>`.
    /// `serve_stdio` keeps a clone for shutdown.
    #[cfg(feature = "headless")]
    pub(crate) headless_renderer:
        Arc<tokio::sync::OnceCell<Arc<crate::fetcher::headless::HeadlessRenderer>>>,
    tool_router: ToolRouter<Self>,
}

impl RoverHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Db,
        config: Arc<Config>,
        client: reqwest::Client,
        ssrf_level: SsrfLevel,
        ssrf_project_root: Option<std::path::PathBuf>,
        har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
        pacer: Arc<Pacer>,
        summarizer: Arc<crate::summarizer::SummarizerService>,
        captioners: Arc<crate::vlm::CaptionerRegistry>,
        guard: Arc<crate::guard::Guard>,
        #[cfg(feature = "headless")] headless_renderer: Arc<
            tokio::sync::OnceCell<Arc<crate::fetcher::headless::HeadlessRenderer>>,
        >,
    ) -> Self {
        Self {
            db,
            config,
            client,
            ssrf_level,
            ssrf_project_root,
            har_recorder,
            pacer,
            summarizer,
            captioners,
            guard,
            #[cfg(feature = "headless")]
            headless_renderer,
            tool_router: Self::tool_router(),
        }
    }
}

/// Resolve the tokenizer family from an optional wire-arg string, falling
/// back to the config default. Returns [`crate::mcp::error::McpError::InvalidArgs`]
/// for unknown family strings so both tools surface the same error code.
pub(crate) fn resolve_tokenizer(
    arg: Option<&str>,
    cfg: &Config,
) -> Result<crate::tokenizer::Tokenizer, crate::mcp::error::McpError> {
    use std::str::FromStr;
    match arg {
        Some(s) => crate::tokenizer::Tokenizer::from_str(s)
            .map_err(|e| crate::mcp::error::McpError::InvalidArgs(e.to_string())),
        None => Ok(cfg.tokenizer.default),
    }
}

#[tool_router]
impl RoverHandler {
    /// Fetch a URL and return cleaned Markdown with frontmatter.
    #[tool(
        description = "Fetch a URL and return cleaned Markdown with frontmatter. \
                       Set count_only=true to return only token counts."
    )]
    pub async fn fetch_tool(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<Json<FetchOutput>, ErrorData> {
        match self.fetch_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }

    /// Count tokens in either an inline `text` or a fetched `url`.
    #[tool(description = "Count tokens for a URL or inline text. \
                       mode=\"single\" (default) returns one token count. \
                       mode=\"estimates\" returns four counts: raw_html, \
                       extracted_md, summary_short (~250 tokens), summary_medium (~750 tokens). \
                       Estimates mode requires url and uses the extractive backend.")]
    pub async fn count_tokens_tool(
        &self,
        Parameters(args): Parameters<CountTokensArgs>,
    ) -> Result<Json<crate::mcp::envelope::CountResponse>, ErrorData> {
        match self.count_tokens_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }

    /// Fetch a URL and return ONLY its structured metadata (no markdown body).
    #[tool(description = "Fetch a URL and return only its structured metadata: \
                       title, description, author, published/modified dates, \
                       schema_types, image, canonical, language, extraction_quality.")]
    pub async fn get_metadata_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::get_metadata::GetMetadataArgs>,
    ) -> Result<Json<crate::mcp::envelope::MetadataResponse>, ErrorData> {
        match self.get_metadata_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }

    /// Apply summarization to a URL's cached or freshly-fetched markdown.
    #[tool(
        description = "Apply summarization to a URL. If the URL isn't cached, \
                       Rover fetches it with default options first. Returns the \
                       summary_md plus metadata including cache status, the \
                       effective backend, and (when applicable) fallback details."
    )]
    pub async fn summarize_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::summarize::SummarizeArgs>,
    ) -> Result<Json<crate::mcp::envelope::SummarizeResponse>, ErrorData> {
        match self.summarize_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }

    /// Fetch multiple URLs concurrently in the background.
    #[tool(
        description = "Fetch multiple URLs concurrently. Returns a task_id immediately; \
                          use rover batch <id> --monitor to stream progress."
    )]
    pub async fn batch_fetch_tool(
        &self,
        Parameters(args): Parameters<crate::mcp::tools::batch_fetch::BatchFetchArgs>,
    ) -> Result<Json<crate::mcp::envelope::TaskCreatedResponse>, ErrorData> {
        match self.batch_fetch_inner(args).await {
            Ok(out) => Ok(Json(out)),
            Err(e) => Err(into_error_data(e)),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RoverHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "rover",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Web fetch & prep for LLM agents. \
                 Tools: fetch, summarize, count_tokens, get_metadata, batch_fetch.",
            )
    }
}

fn into_error_data(err: crate::mcp::error::McpError) -> ErrorData {
    use crate::mcp::error::McpError;
    let is_user_error = matches!(
        &err,
        McpError::InvalidArgs(_)
            | McpError::InvalidUrl(_)
            | McpError::TooManyUrls { .. }
            | McpError::EmptyUrlList
            | McpError::Summarizer(
                crate::summarizer::SummarizerError::NoSuchBackend { .. }
                    | crate::summarizer::SummarizerError::InvalidRequest { .. }
            ),
    );
    let r = crate::mcp::error::log_and_translate(err);
    let code = if is_user_error {
        rmcp::model::ErrorCode::INVALID_PARAMS
    } else {
        rmcp::model::ErrorCode::INTERNAL_ERROR
    };
    let message = format!("{}: {}", r.code, r.message);
    let data = serde_json::to_value(&r).ok();
    ErrorData::new(code, message, data)
}

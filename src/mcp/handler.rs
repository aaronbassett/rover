//! Shared MCP server state.

use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router};

use crate::config::Config;
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::tools::count_tokens::CountTokensArgs;
use crate::mcp::tools::fetch::{FetchArgs, FetchOutput};
use crate::storage::Db;

/// State shared across all MCP tool invocations.
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) ssrf_level: SsrfLevel,
    tool_router: ToolRouter<Self>,
}

impl RoverHandler {
    pub fn new(
        db: Db,
        config: Arc<Config>,
        client: reqwest::Client,
        ssrf_level: SsrfLevel,
    ) -> Self {
        Self {
            db,
            config,
            client,
            ssrf_level,
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
    #[tool(
        description = "Count tokens in either an inline text string or a URL's \
                       extracted Markdown. Exactly one of text/url is required."
    )]
    pub async fn count_tokens_tool(
        &self,
        Parameters(args): Parameters<CountTokensArgs>,
    ) -> Result<Json<crate::mcp::envelope::CountResponse>, ErrorData> {
        match self.count_tokens_inner(args).await {
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
            .with_instructions("Web fetch & prep for LLM agents. Tools: fetch, count_tokens.")
    }
}

fn into_error_data(err: crate::mcp::error::McpError) -> ErrorData {
    let r = crate::mcp::error::log_and_translate(err);
    let message = format!("{}: {}", r.code, r.message);
    let data = serde_json::to_value(&r).ok();
    ErrorData::new(rmcp::model::ErrorCode::INTERNAL_ERROR, message, data)
}

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
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) ssrf_level: SsrfLevel,
    pub(crate) pacer: Arc<Pacer>,
    tool_router: ToolRouter<Self>,
}

impl RoverHandler {
    pub fn new(
        db: Db,
        config: Arc<Config>,
        client: reqwest::Client,
        ssrf_level: SsrfLevel,
        pacer: Arc<Pacer>,
    ) -> Self {
        Self {
            db,
            config,
            client,
            ssrf_level,
            pacer,
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
                "Web fetch & prep for LLM agents. Tools: fetch, count_tokens, get_metadata.",
            )
    }
}

fn into_error_data(err: crate::mcp::error::McpError) -> ErrorData {
    use crate::mcp::error::McpError;
    let is_user_error = matches!(&err, McpError::InvalidArgs(_) | McpError::InvalidUrl(_));
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

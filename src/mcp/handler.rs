//! Shared MCP server state.

use std::sync::Arc;

use crate::config::Config;
use crate::storage::Db;

/// State shared across all MCP tool invocations.
#[derive(Clone)]
pub struct RoverHandler {
    pub(crate) db: Db,
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
}

impl RoverHandler {
    pub fn new(db: Db, config: Arc<Config>, client: reqwest::Client) -> Self {
        Self { db, config, client }
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

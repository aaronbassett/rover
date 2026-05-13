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

//! CLI command implementations.

pub mod batch;
pub mod cache;
pub mod config;
pub mod doctor;
pub mod fetch;
pub mod mcp;
pub mod meta;
#[cfg(feature = "local-inference")]
pub mod model;
pub mod task;

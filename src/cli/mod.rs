//! CLI command implementations.

pub mod batch;
pub mod cache;
pub mod config;
pub mod doctor;
pub mod fetch;
pub mod mcp;
#[cfg(any(feature = "local-inference", feature = "local-vision"))]
pub mod model;
pub mod task;

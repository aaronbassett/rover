//! Rover — an MCP server for fetching and prepping web content for LLM agents.
//!
//! See `docs/superpowers/prd/2026-05-07-rover-prd.md` for product spec and
//! `docs/superpowers/specs/2026-05-07-rover-design.md` for architectural decisions.

pub mod cli;
pub mod config;
pub mod doctor;
pub mod error;
pub mod extractor;
pub mod fetcher;
pub mod mcp;
#[cfg(feature = "local-inference")]
pub mod model_integrity;
pub mod paths;
pub mod storage;
pub mod summarizer;
pub mod tasks;
pub mod telemetry;
pub mod tokenizer;
pub mod vlm;

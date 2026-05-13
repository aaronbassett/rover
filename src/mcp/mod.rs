//! MCP server mode (rover mcp).
//!
//! Architecture: a `RoverHandler` (Task 9) backed by `rmcp`'s `#[tool_router]`
//! macros holds the `Db` + `Config` + `reqwest::Client` shared state. Two
//! tools (`fetch`, `count_tokens`) wrap the M1/M2 pipeline behind typed
//! arg structs. Errors are translated to a stable wire envelope.

pub mod envelope;
pub mod error;
pub mod handler;
pub mod server;
pub mod tools;

pub use envelope::{CacheStatus, CountResponse, CountSource, FetchResponse, RoverError};
pub use error::McpError;
pub use handler::RoverHandler;
pub use server::serve_stdio;

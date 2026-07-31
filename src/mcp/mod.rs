//! MCP server mode (rover mcp).
//!
//! Architecture: a `RoverHandler` (Task 9) backed by `rmcp`'s `#[tool_router]`
//! macros holds the `Db` + `Config` + `reqwest::Client` shared state. Two
//! tools (`fetch`, `count_tokens`) wrap the M1/M2 pipeline behind typed
//! arg structs. Errors are translated to a stable wire envelope.

/// Which transport the server is running under.
///
/// Threaded through `RoverHandler` because two behaviours differ by
/// transport: tool modes that emit server-side filesystem paths are refused
/// over HTTP (they are meaningless to a remote caller), and the server span
/// is tagged so a mixed deployment's logs stay attributable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Stdio,
    Http,
}

impl TransportKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

pub mod envelope;
pub mod error;
pub mod handler;
pub mod http;
pub mod runtime;
pub mod stdio;
pub mod tools;

pub use envelope::{CacheStatus, CountResponse, CountSource, FetchResponse, RoverError};
pub use error::McpError;
pub use handler::RoverHandler;
pub use stdio::serve_stdio;

#[cfg(test)]
mod tests {
    use super::TransportKind;

    #[test]
    fn transport_kind_as_str() {
        assert_eq!(TransportKind::Stdio.as_str(), "stdio");
        assert_eq!(TransportKind::Http.as_str(), "http");
    }
}

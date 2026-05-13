//! MCP server mode (rover mcp).
//!
//! Tasks 8-11 fill this module in. The shape is:
//!   - envelope.rs: wire types returned to clients
//!   - error.rs:    internal McpError
//!   - handler.rs:  RoverHandler { db, config, client }
//!   - tools/:      #[tool] handlers
//!   - server.rs:   serve_stdio + lifecycle

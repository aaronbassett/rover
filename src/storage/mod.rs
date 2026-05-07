//! SQLite-backed cache and task storage.
//!
//! The storage layer is a thin async API over a single `tokio-rusqlite`
//! connection actor. All access is async; sync rusqlite is reachable only via
//! the actor's `call` closure.
//!
//! Per design §2.1 and §4.2: a single connection writer per process; multi-
//! process safety via WAL mode + `busy_timeout`. Migrations applied on open.

pub mod error;
pub mod pages;
pub mod system;

pub use error::StorageError;

// Db wrapper and open() function come in Task 2.

//! Agent-harness wiring: `rover meta use <harness>` and the runtime hook handler.
//!
//! See `docs/superpowers/specs/2026-06-24-rover-meta-use-harness-design.md`.

pub mod claude;
pub mod edits;
pub mod general;
pub mod hook;

use std::path::PathBuf;

/// A single file action performed by `run_use`, for the summary printout.
pub struct Change {
    pub path: PathBuf,
    pub action: &'static str,
}

impl Change {
    pub fn new(path: PathBuf, action: &'static str) -> Self {
        Self { path, action }
    }
}

/// Configuration scope, mirroring the Claude CLI's `--scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    Local,
    User,
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// The agent harness to wire Rover into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    Claude,
    General,
}

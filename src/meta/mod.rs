//! Agent-harness wiring: `rover meta use <harness>` and the runtime hook handler.
//!
//! See `docs/superpowers/specs/2026-06-24-rover-meta-use-harness-design.md`.

pub mod claude;
pub mod edits;
pub mod general;
pub mod hook;

use std::path::{Path, PathBuf};

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

/// Orchestrate `rover meta use`: validate-then-apply, then print a summary.
pub fn run_use(harness: Harness, scope: Scope, root: &Path) -> anyhow::Result<i32> {
    let changes = match harness {
        Harness::Claude => {
            claude::preflight(scope, root)?; // aborts before any write
            claude::apply(scope, root)?
        }
        Harness::General => {
            if !matches!(scope, Scope::Project) {
                eprintln!(
                    "note: `general` supports project scope only; writing to the project root"
                );
            }
            general::preflight(root)?;
            general::apply(root)?
        }
    };
    print_summary(harness, &changes);
    Ok(0)
}

fn print_summary(harness: Harness, changes: &[Change]) {
    eprintln!("✓ wired Rover into `{}`:", harness_name(harness));
    for c in changes {
        eprintln!("  - {} ({})", c.path.display(), c.action);
    }
}

fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "claude",
        Harness::General => "general",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn run_use_general_writes_files_and_returns_zero() {
        let tmp = tempdir().unwrap();
        let code = run_use(Harness::General, Scope::Project, tmp.path()).unwrap();
        assert_eq!(code, 0);
        assert!(tmp.path().join("mcp.json").exists());
        assert!(tmp.path().join("AGENTS.md").exists());
    }
}

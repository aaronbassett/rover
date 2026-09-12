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
///
/// `caps` is a snapshot of what this install can actually do; the rules
/// blocks it writes are generated from it, so a build without `web-search`
/// (or one with no API key) never installs steering that recommends the
/// `search` tool. The hooks re-evaluate capabilities on every session, so
/// only the on-disk rules files are frozen at install time — the summary
/// says so when search is not yet available.
pub fn run_use(
    harness: Harness,
    scope: Scope,
    root: &Path,
    caps: hook::Capabilities,
) -> anyhow::Result<i32> {
    let changes = match harness {
        Harness::Claude => {
            claude::preflight(scope, root)?; // aborts before any write
            claude::apply(scope, root, caps)?
        }
        Harness::General => {
            if !matches!(scope, Scope::Project) {
                eprintln!(
                    "note: `general` supports project scope only; writing to the project root"
                );
            }
            general::preflight(root)?;
            general::apply(root, caps)?
        }
    };
    print_summary(harness, &changes);
    print_search_note(caps);
    Ok(0)
}

/// Tell the user why the written rules file says nothing about `search`,
/// and what would change that. Silent when search is ready.
fn print_search_note(caps: hook::Capabilities) {
    use crate::search::SearchAvailability as A;
    match caps.search {
        A::Ready => {}
        A::NotConfigured => eprintln!(
            "note: web search is compiled in but has no API key, so the rules file omits it. \
             Export one (see `[search] api_key_env`, default BRAVE_SEARCH_API_KEY) and re-run \
             `rover meta use` to add it; the Claude Code hooks pick it up on the next session \
             with no re-run needed."
        ),
        A::NotCompiled => eprintln!(
            "note: this build has no web search (compiled without the `web-search` feature), so \
             the rules file omits it. The prebuilt binaries include it."
        ),
    }
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

    fn caps(search: crate::search::SearchAvailability) -> hook::Capabilities {
        hook::Capabilities { search }
    }

    #[test]
    fn run_use_general_writes_files_and_returns_zero() {
        let tmp = tempdir().unwrap();
        let code = run_use(
            Harness::General,
            Scope::Project,
            tmp.path(),
            caps(crate::search::SearchAvailability::NotConfigured),
        )
        .unwrap();
        assert_eq!(code, 0);
        assert!(tmp.path().join("mcp.json").exists());
        assert!(tmp.path().join("AGENTS.md").exists());
    }

    /// The rules block written to disk must reflect the capability
    /// snapshot it was given, in both directions.
    #[test]
    fn written_rules_track_search_availability() {
        let tmp = tempdir().unwrap();
        run_use(
            Harness::General,
            Scope::Project,
            tmp.path(),
            caps(crate::search::SearchAvailability::Ready),
        )
        .unwrap();
        let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert!(
            agents.contains(r#"{ "query": "rust async trait" }"#),
            "{agents}"
        );

        let tmp2 = tempdir().unwrap();
        run_use(
            Harness::General,
            Scope::Project,
            tmp2.path(),
            caps(crate::search::SearchAvailability::NotCompiled),
        )
        .unwrap();
        let agents2 = std::fs::read_to_string(tmp2.path().join("AGENTS.md")).unwrap();
        assert!(!agents2.contains(r#"{ "query": "#), "{agents2}");
    }
}

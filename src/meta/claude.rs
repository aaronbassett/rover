//! The `claude` harness: delegate MCP to the `claude` CLI; write hooks + CLAUDE.md.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context;

use crate::meta::{Change, Scope, edits, hook};

/// The command string written into `settings.json` for both hook events.
pub const HOOK_COMMAND: &str = "rover meta hook claude";

/// Resolve the `claude` binary name. `ROVER_CLAUDE_BIN` overrides (used by tests).
pub fn claude_bin() -> String {
    std::env::var("ROVER_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string())
}

/// True if the `claude` CLI is runnable (`<bin> --version` exits 0).
pub fn claude_available() -> bool {
    Command::new(claude_bin())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn settings_path(scope: Scope, root: &Path) -> PathBuf {
    match scope {
        Scope::Local => root.join(".claude").join("settings.local.json"),
        Scope::Project => root.join(".claude").join("settings.json"),
        Scope::User => home().join(".claude").join("settings.json"),
    }
}

fn claude_md_path(scope: Scope, root: &Path) -> Option<PathBuf> {
    match scope {
        // No clean private CLAUDE.md at local scope — the SessionStart hook
        // (in settings.local.json) carries the steering instead.
        Scope::Local => None,
        Scope::Project => Some(root.join("CLAUDE.md")),
        Scope::User => Some(home().join(".claude").join("CLAUDE.md")),
    }
}

/// Validate prerequisites without writing: the `claude` binary must be present,
/// and any existing target `settings.json` must parse.
pub fn preflight(scope: Scope, root: &Path) -> anyhow::Result<()> {
    if !claude_available() {
        anyhow::bail!(
            "the `claude` CLI was not found on PATH; install Claude Code, or run \
             `rover meta use general` to write an mcp.json + AGENTS.md instead"
        );
    }
    let settings = settings_path(scope, root);
    if settings.exists() {
        let text = std::fs::read_to_string(&settings)
            .with_context(|| format!("reading {}", settings.display()))?;
        if !text.trim().is_empty() {
            serde_json::from_str::<serde_json::Value>(&text)
                .with_context(|| format!("{} is not valid JSON", settings.display()))?;
        }
    }
    Ok(())
}

/// Register the MCP server, install the hooks, and (except at local scope)
/// write the CLAUDE.md steering block.
pub fn apply(scope: Scope, root: &Path) -> anyhow::Result<Vec<Change>> {
    let mut changes = Vec::new();

    register_mcp(scope)?;

    let settings = settings_path(scope, root);
    let existing = std::fs::read_to_string(&settings).unwrap_or_default();
    let merged = edits::merge_hooks(&existing, HOOK_COMMAND)?;
    edits::write_file(&settings, &merged)?;
    changes.push(Change::new(settings, "hooks installed"));

    if let Some(md) = claude_md_path(scope, root) {
        let existing = std::fs::read_to_string(&md).unwrap_or_default();
        let updated = edits::upsert_managed_block(&existing, hook::RULES_BLOCK_CLAUDE);
        edits::write_file(&md, &updated)?;
        changes.push(Change::new(md, "rules block written"));
    }

    Ok(changes)
}

/// `claude mcp add rover -s <scope> -- rover mcp`, skipping if already present.
fn register_mcp(scope: Scope) -> anyhow::Result<()> {
    let present = Command::new(claude_bin())
        .args(["mcp", "get", "rover"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if present {
        return Ok(());
    }
    let status = Command::new(claude_bin())
        .args([
            "mcp",
            "add",
            "rover",
            "-s",
            scope.as_str(),
            "--",
            "rover",
            "mcp",
        ])
        .status()
        .context("running `claude mcp add`")?;
    if !status.success() {
        anyhow::bail!("`claude mcp add` failed (exit {:?})", status.code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Scope;
    use tempfile::tempdir;

    #[test]
    fn settings_path_maps_by_scope() {
        let root = std::path::Path::new("/proj");
        assert!(settings_path(Scope::Local, root).ends_with(".claude/settings.local.json"));
        assert!(settings_path(Scope::Project, root).ends_with(".claude/settings.json"));
        assert!(settings_path(Scope::User, root).ends_with(".claude/settings.json"));
        // User scope is NOT under the project root.
        assert!(!settings_path(Scope::User, root).starts_with("/proj"));
    }

    #[test]
    fn claude_md_skipped_at_local() {
        let root = std::path::Path::new("/proj");
        assert!(claude_md_path(Scope::Local, root).is_none());
        assert_eq!(
            claude_md_path(Scope::Project, root).unwrap(),
            std::path::Path::new("/proj/CLAUDE.md")
        );
    }

    #[test]
    fn preflight_fails_when_claude_missing() {
        // SAFETY: tests are single-threaded within a file by default; this var
        // is read only inside this process's claude_bin().
        unsafe { std::env::set_var("ROVER_CLAUDE_BIN", "/nonexistent/claude-xyz-rover-test") };
        let tmp = tempdir().unwrap();
        let result = preflight(Scope::Project, tmp.path());
        unsafe { std::env::remove_var("ROVER_CLAUDE_BIN") };
        assert!(result.is_err());
        // No files were created.
        assert!(!tmp.path().join(".claude").exists());
    }
}

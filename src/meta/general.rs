//! The `general` harness: project-root `mcp.json` + `AGENTS.md`.

use std::path::Path;

use anyhow::Context;

use crate::meta::{Change, edits, hook};

/// Validate that the existing `mcp.json` (if any) parses. No writes.
pub fn preflight(root: &Path) -> anyhow::Result<()> {
    let mcp = root.join("mcp.json");
    if mcp.exists() {
        let text =
            std::fs::read_to_string(&mcp).with_context(|| format!("reading {}", mcp.display()))?;
        if !text.trim().is_empty() {
            serde_json::from_str::<serde_json::Value>(&text)
                .with_context(|| format!("{} is not valid JSON", mcp.display()))?;
        }
    }
    Ok(())
}

/// Write the project-root `mcp.json` and the `AGENTS.md` steering block.
pub fn apply(root: &Path) -> anyhow::Result<Vec<Change>> {
    let mcp = root.join("mcp.json");
    let existing = std::fs::read_to_string(&mcp).unwrap_or_default();
    let merged = edits::merge_mcp_server(&existing)?;
    edits::write_file(&mcp, &merged)?;

    let agents = root.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents).unwrap_or_default();
    let updated = edits::upsert_managed_block(&existing, hook::RULES_BLOCK_GENERAL);
    edits::write_file(&agents, &updated)?;

    Ok(vec![
        Change::new(mcp, "mcp server written"),
        Change::new(agents, "rules block written"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn apply_writes_mcp_json_and_agents_md() {
        let tmp = tempdir().unwrap();
        let changes = apply(tmp.path()).unwrap();
        assert_eq!(changes.len(), 2);

        let mcp = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        assert_eq!(v["mcpServers"]["rover"]["command"], "rover");

        let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("prefer Rover"));
        assert!(agents.contains(crate::meta::edits::BEGIN_MARKER));
    }

    #[test]
    fn apply_is_idempotent() {
        let tmp = tempdir().unwrap();
        apply(tmp.path()).unwrap();
        let mcp1 = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let agents1 = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        apply(tmp.path()).unwrap();
        let mcp2 = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let agents2 = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert_eq!(mcp1, mcp2);
        assert_eq!(agents1, agents2);
        assert_eq!(agents2.matches(crate::meta::edits::BEGIN_MARKER).count(), 1);
    }

    #[test]
    fn preflight_rejects_malformed_mcp_json() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("mcp.json"), "{ broken").unwrap();
        assert!(preflight(tmp.path()).is_err());
    }
}

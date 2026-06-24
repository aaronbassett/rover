//! Shared, pure file-edit utilities for harness wiring.

use std::path::Path;

use anyhow::Context;

pub const BEGIN_MARKER: &str =
    "<!-- rover:begin — managed by `rover meta use`; edit outside these markers -->";
pub const END_MARKER: &str = "<!-- rover:end -->";

/// Insert or replace Rover's managed block in `contents`.
///
/// If both markers are present (in order) the inner content is replaced;
/// otherwise the block is appended. Content outside the markers is untouched.
/// `body` is the block's inner Markdown (no markers).
pub fn upsert_managed_block(contents: &str, body: &str) -> String {
    let body = body.trim_end_matches('\n');
    let block = format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}");

    if let Some(start) = contents.find(BEGIN_MARKER)
        && let Some(rel) = contents[start..].find(END_MARKER)
    {
        let end_full = start + rel + END_MARKER.len();
        let mut out = String::with_capacity(contents.len() + block.len());
        out.push_str(&contents[..start]);
        out.push_str(&block);
        out.push_str(&contents[end_full..]);
        return out;
    }

    let mut out = contents.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n'); // blank line before the block
    }
    out.push_str(&block);
    out.push('\n');
    out
}

/// Write `contents` to `path`, creating parent directories as needed.
pub fn write_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Upsert the `rover` server into an `mcp.json` document.
pub fn merge_mcp_server(json_text: &str) -> anyhow::Result<String> {
    let mut root: serde_json::Value = if json_text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(json_text).context("parsing mcp.json")?
    };
    let obj = root
        .as_object_mut()
        .context("mcp.json root is not a JSON object")?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .context("mcp.json `mcpServers` is not a JSON object")?;
    servers.insert(
        "rover".to_string(),
        serde_json::json!({ "command": "rover", "args": ["mcp"] }),
    );

    let mut out = serde_json::to_string_pretty(&root)?;
    out.push('\n');
    Ok(out)
}

/// Add Rover's `SessionStart` and `PreToolUse(WebFetch)` hooks to a
/// `settings.json` document, idempotently (keyed on `hook_command`).
pub fn merge_hooks(json_text: &str, hook_command: &str) -> anyhow::Result<String> {
    let mut root: serde_json::Value = if json_text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(json_text).context("parsing settings.json")?
    };
    let obj = root
        .as_object_mut()
        .context("settings.json root is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| serde_json::json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("settings.json `hooks` is not a JSON object")?;

    add_event_hook(hooks, "SessionStart", None, hook_command)?;
    add_event_hook(hooks, "PreToolUse", Some("WebFetch"), hook_command)?;

    let mut out = serde_json::to_string_pretty(&root)?;
    out.push('\n');
    Ok(out)
}

fn add_event_hook(
    hooks: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    matcher: Option<&str>,
    command: &str,
) -> anyhow::Result<()> {
    let arr = hooks.entry(event).or_insert_with(|| serde_json::json!([]));
    let arr = arr
        .as_array_mut()
        .with_context(|| format!("settings.json `hooks.{event}` is not a JSON array"))?;

    let already = arr.iter().any(|group| {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hs| {
                hs.iter()
                    .any(|hk| hk.get("command").and_then(|c| c.as_str()) == Some(command))
            })
    });
    if already {
        return Ok(());
    }

    let mut group = serde_json::Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".to_string(), serde_json::json!(m));
    }
    group.insert(
        "hooks".to_string(),
        serde_json::json!([{ "type": "command", "command": command }]),
    );
    arr.push(serde_json::Value::Object(group));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOK_CMD: &str = "rover meta hook claude";

    #[test]
    fn hooks_fresh_document_adds_both_events() {
        let out = merge_hooks("", HOOK_CMD).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let ss = &v["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(ss["type"], "command");
        assert_eq!(ss["command"], HOOK_CMD);
        let pt = &v["hooks"]["PreToolUse"][0];
        assert_eq!(pt["matcher"], "WebFetch");
        assert_eq!(pt["hooks"][0]["command"], HOOK_CMD);
    }

    #[test]
    fn hooks_preserve_unrelated_and_are_idempotent() {
        let existing = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#;
        let once = merge_hooks(existing, HOOK_CMD).unwrap();
        let v: serde_json::Value = serde_json::from_str(&once).unwrap();
        // Unrelated Bash hook preserved.
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(pre.iter().any(|g| g["matcher"] == "Bash"));
        assert!(pre.iter().any(|g| g["matcher"] == "WebFetch"));
        // Re-running adds nothing.
        let twice = merge_hooks(&once, HOOK_CMD).unwrap();
        assert_eq!(twice, once);
        let v2: serde_json::Value = serde_json::from_str(&twice).unwrap();
        assert_eq!(v2["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn hooks_malformed_is_error() {
        assert!(merge_hooks("{ broken", HOOK_CMD).is_err());
    }

    #[test]
    fn inserts_block_into_empty() {
        let out = upsert_managed_block("", "BODY");
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains("BODY"));
        assert!(out.contains(END_MARKER));
    }

    #[test]
    fn appends_block_preserving_existing() {
        let out = upsert_managed_block("# My notes\n", "BODY");
        assert!(out.starts_with("# My notes\n"));
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains(END_MARKER));
    }

    #[test]
    fn replaces_existing_block_and_is_idempotent() {
        let once = upsert_managed_block("# Notes\n", "FIRST");
        let twice = upsert_managed_block(&once, "SECOND");
        // Exactly one managed block survives.
        assert_eq!(twice.matches(BEGIN_MARKER).count(), 1);
        assert!(twice.contains("SECOND"));
        assert!(!twice.contains("FIRST"));
        // Surrounding content preserved.
        assert!(twice.starts_with("# Notes\n"));
        // Re-applying the same body is a fixed point.
        let thrice = upsert_managed_block(&twice, "SECOND");
        assert_eq!(thrice, twice);
    }

    #[test]
    fn mcp_fresh_document_adds_rover() {
        let out = merge_mcp_server("").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["rover"]["command"], "rover");
        assert_eq!(v["mcpServers"]["rover"]["args"][0], "mcp");
    }

    #[test]
    fn mcp_preserves_other_servers_and_is_idempotent() {
        let existing = r#"{"mcpServers":{"other":{"command":"x"}}}"#;
        let once = merge_mcp_server(existing).unwrap();
        let v: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["rover"]["command"], "rover");
        let twice = merge_mcp_server(&once).unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn mcp_malformed_is_error() {
        assert!(merge_mcp_server("{ not json").is_err());
    }
}

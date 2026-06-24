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
    let block = format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}");

    if let (Some(start), Some(end)) = (contents.find(BEGIN_MARKER), contents.find(END_MARKER))
        && start < end
    {
        let end_full = end + END_MARKER.len();
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

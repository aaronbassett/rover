//! `rover config show` (and `set` in Task 13).
//!
//! Renders the effective configuration as a TOML-flavoured listing where
//! every leaf carries an inline `# <dotted-key>: from <source>` comment so
//! `grep ssrf.level` against the output works.

use anyhow::Context;

use crate::config::{Config, default_config_path, provenance, resolve_existing_config_path};

pub struct ShowArgs {
    /// Optional config path. `None` resolves the active config file
    /// (`ROVER_CONFIG`, platform config dir, then `./rover.toml`), falling back
    /// to the canonical default path for display when none exists.
    pub config_path: Option<std::path::PathBuf>,
}

pub fn show(args: ShowArgs) -> anyhow::Result<i32> {
    // Read the same file the runtime would load. When none exists, fall back to
    // the canonical default path so the header still points at where a config
    // would live; the read below then yields an empty (defaults) view.
    let path = args
        .config_path
        .or_else(resolve_existing_config_path)
        .unwrap_or_else(default_config_path);
    let file_text = std::fs::read_to_string(&path).unwrap_or_default();

    // Validate the file parses cleanly. `show` shouldn't run against a broken
    // file — surface the parse error to the user rather than printing garbage.
    let cfg: Config = if file_text.is_empty() {
        Config::default()
    } else {
        toml::from_str(&file_text).with_context(|| format!("parsing {}", path.display()))?
    };
    let rows = provenance::provenance_for(&file_text);
    let effective = effective_values(&cfg);

    println!("# rover effective configuration");
    println!("# defaults | file ({}) | env", path.display());
    println!();

    // Group by top-level section.
    let mut by_section: std::collections::BTreeMap<&str, Vec<&provenance::ProvenanceRow>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        let section = r.dotted.split_once('.').map(|(a, _)| a).unwrap_or("");
        by_section.entry(section).or_default().push(r);
    }
    for (section, section_rows) in by_section {
        if !section.is_empty() {
            println!("[{section}]");
        }
        for r in section_rows {
            let leaf = r
                .dotted
                .rsplit_once('.')
                .map(|(_, b)| b)
                .unwrap_or(&r.dotted);
            let value = effective
                .get(r.dotted.as_str())
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            let source = match r.source {
                provenance::Source::Default => "defaults",
                provenance::Source::File => "file",
                provenance::Source::Env => "env",
            };
            // Include the full dotted key in the comment so a `grep <dotted>`
            // against the output matches the right line.
            println!(
                "{leaf} = {value}  # from: {source} ({dotted})",
                dotted = r.dotted,
            );
        }
        println!();
    }
    Ok(0)
}

pub struct SetArgs {
    pub config_path: Option<std::path::PathBuf>,
    pub key: String,
    pub value: String,
}

pub fn set(args: SetArgs) -> anyhow::Result<i32> {
    // Modify the active config file when one already exists (so a set lands in
    // the file the runtime reads); otherwise create the canonical default.
    let path = args
        .config_path
        .or_else(resolve_existing_config_path)
        .unwrap_or_else(default_config_path);
    // Ensure parent dir exists so a first-time set creates the file cleanly.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
    }
    // Touch the file if missing so apply_set's read succeeds.
    if !path.exists() {
        std::fs::write(&path, "").with_context(|| format!("creating {}", path.display()))?;
    }
    match crate::config::edit::apply_set(&path, &args.key, &args.value) {
        Ok(()) => {
            eprintln!(
                "✓ {} = {}  (wrote {})",
                args.key,
                args.value,
                path.display()
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Ok(1)
        }
    }
}

/// Build a `dotted-key → TOML-formatted-string` map of effective values by
/// serializing the loaded Config to a generic `toml::Value` and indexing each
/// known leaf.
fn effective_values(cfg: &Config) -> std::collections::HashMap<&'static str, String> {
    let v = toml::Value::try_from(cfg).unwrap_or_else(|_| toml::Value::Table(Default::default()));
    let mut out = std::collections::HashMap::new();
    for dotted in provenance::known_leaves() {
        if let Some(val) = lookup_dotted(&v, dotted) {
            out.insert(*dotted, render_toml_value(&val));
        }
    }
    out
}

fn lookup_dotted(v: &toml::Value, dotted: &str) -> Option<toml::Value> {
    let mut cur = v.clone();
    for part in dotted.split('.') {
        let toml::Value::Table(t) = cur else {
            return None;
        };
        cur = t.get(part)?.clone();
    }
    Some(cur)
}

fn render_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{s}\""),
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) => {
            toml::to_string(v).unwrap_or_default().trim().to_string()
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}

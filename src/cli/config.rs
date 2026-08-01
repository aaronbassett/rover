//! `rover config show` (and `set` in Task 13).
//!
//! Renders the effective configuration as a TOML-flavoured listing where
//! every leaf carries an inline `# <dotted-key>: from <source>` comment so
//! `grep ssrf.level` against the output works.

use anyhow::Context;

use crate::config::{
    Config, ConfigLocation, default_config_path, provenance, resolve_config_location,
};

pub struct ShowArgs {
    /// Optional config path. `None` resolves the active config file
    /// (`ROVER_CONFIG`, platform config dir, then `./rover.toml`), falling back
    /// to the canonical default path for display when none exists.
    pub config_path: Option<std::path::PathBuf>,
}

pub fn show(args: ShowArgs) -> anyhow::Result<i32> {
    // Read the same file `load_resolved` would load, and honour the same
    // contract: an explicit `--config`/`ROVER_CONFIG` redirect that fails to
    // read is a loud error, never a silent fallback to defaults. Getting
    // this wrong is worse here than anywhere else — `show`'s entire job is
    // reporting provenance, so silently defaulting while still printing
    // `file (<path>)` in the header claims a file was consulted when it
    // never was.
    let (path, file_text) = match resolve_config_location(args.config_path.as_deref()) {
        ConfigLocation::Explicit { path, source } => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {} (from {source})", path.display()))?;
            (path, text)
        }
        ConfigLocation::Found(path) => {
            // Existed at resolution time; a failure to read it now (TOCTOU)
            // is still surfaced loudly rather than silently defaulted — the
            // header would otherwise name a file it never actually read.
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            (path, text)
        }
        ConfigLocation::None => {
            // Nothing configured anywhere: built-in defaults are the
            // correct, expected outcome. Report against the canonical
            // default path so the header still shows where a config file
            // would be created — this is the one case where the header
            // names a path with no file behind it, and that's by design,
            // not a claim that one was read.
            (default_config_path(), String::new())
        }
    };

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
    // An explicit `--config`/`ROVER_CONFIG` redirect is held to the same
    // contract as `show` and `load_resolved`: it must already exist.
    // Silently creating a file at a path the operator explicitly pointed
    // at — most likely a typo, or a container's bind-mount source that
    // doesn't exist on the host — would hide the mistake instead of
    // failing loudly, and would write into a location the runtime may not
    // even be able to find again the same way (a relative `ROVER_CONFIG`
    // resolved against a different cwd next run, for instance).
    let path = match resolve_config_location(args.config_path.as_deref()) {
        ConfigLocation::Explicit { path, source } => {
            if !path.is_file() {
                anyhow::bail!(
                    "config file {} does not exist (from {source}); refusing to create one — \
                     point {source} at an existing file, or unset it to use the platform \
                     default config path (created automatically on first `config set`)",
                    path.display()
                );
            }
            path
        }
        ConfigLocation::Found(path) => path,
        ConfigLocation::None => default_config_path(),
    };
    // Ensure parent dir exists so a first-time set (only the `None` branch
    // above can reach here with a missing file — `Explicit` is already
    // confirmed to exist, and `Found` only ever names an existing file)
    // creates the file cleanly.
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
        // `toml::to_string` only serialises tables — a bare array errors with
        // UnsupportedType and `unwrap_or_default()` silently rendered "".
        toml::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(render_toml_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        toml::Value::Table(_) => toml::to_string(v).unwrap_or_default().trim().to_string(),
        toml::Value::Datetime(d) => d.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_arrays_inline() {
        let empty = toml::Value::Array(vec![]);
        assert_eq!(render_toml_value(&empty), "[]");

        let list = toml::Value::Array(vec![
            toml::Value::String("localhost".into()),
            toml::Value::String("127.0.0.1".into()),
        ]);
        assert_eq!(render_toml_value(&list), "[\"localhost\", \"127.0.0.1\"]");
    }

    /// Pins the defect a re-review caught: `show` used to call
    /// `resolve_existing_config_path` directly, bypassing the
    /// fail-loudly-on-an-explicit-redirect contract `load_resolved`
    /// enforces — so `ROVER_CONFIG` pointed at a path that doesn't exist
    /// would silently print `# defaults | file (<path>)`, claiming to have
    /// read a file it never touched. `show`'s entire job is reporting
    /// provenance, so that was actively misleading, not just a missing
    /// check. This asserts `show` now returns `Err` instead of `Ok`.
    #[test]
    fn show_fails_loudly_when_rover_config_env_is_unreadable() {
        let _guard = crate::config::ROVER_CONFIG_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.toml");

        // SAFETY: `_guard` (the shared `ROVER_CONFIG_ENV_LOCK`) serialises
        // this against every other test in the `--lib` binary that touches
        // `ROVER_CONFIG`, including `src/config/mod.rs`'s tests.
        unsafe { std::env::set_var("ROVER_CONFIG", &missing) };
        let result = show(ShowArgs { config_path: None });
        unsafe { std::env::remove_var("ROVER_CONFIG") };

        assert!(
            result.is_err(),
            "show() must fail loudly when ROVER_CONFIG names a file that \
             doesn't exist, not silently print defaults labelled with that \
             path — got: {result:?}"
        );
    }

    /// Same contract on the `config set` side. Before this fix, `set` would
    /// `std::fs::write` an empty file at whatever path it resolved —
    /// including a mistyped `ROVER_CONFIG` — before ever checking whether
    /// that path came from an explicit redirect the operator expects to
    /// already exist. Refusing loudly beats silently creating a file at an
    /// address that was probably a typo.
    #[test]
    fn set_refuses_to_create_a_file_at_an_unreadable_explicit_rover_config() {
        let _guard = crate::config::ROVER_CONFIG_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.toml");

        // SAFETY: see `show_fails_loudly_when_rover_config_env_is_unreadable`.
        unsafe { std::env::set_var("ROVER_CONFIG", &missing) };
        let result = set(SetArgs {
            config_path: None,
            key: "fetch.timeout_secs".to_string(),
            value: "5".to_string(),
        });
        unsafe { std::env::remove_var("ROVER_CONFIG") };

        assert!(
            result.is_err(),
            "set() must fail loudly when ROVER_CONFIG names a file that \
             doesn't exist, got: {result:?}"
        );
        assert!(
            !missing.exists(),
            "set() must not silently create a file at an explicit, \
             nonexistent ROVER_CONFIG path"
        );
    }
}

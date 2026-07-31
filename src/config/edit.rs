//! `rover config set` — settable-key whitelist + value parsers + writer.

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SetError {
    #[error("io error reading {path:?}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("io error writing {path:?}: {source}")]
    Write {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("could not parse existing file at {path:?}: {source}")]
    ParseExistingFile {
        path: std::path::PathBuf,
        source: toml_edit::TomlError,
    },

    #[error("invalid value for {key}: expected {expected}, got {value}")]
    Parse {
        key: String,
        value: String,
        expected: String,
    },

    #[error("key `{key}` is not settable via `rover config set`; edit the file directly")]
    Unsettable { key: String },

    #[error("validation failed after writing {key} = {value}: {message}")]
    Validation {
        key: String,
        value: String,
        message: String,
    },
}

struct SettableSpec {
    key: &'static str,
    parser: fn(&str) -> Result<toml_edit::Item, String>,
    expected: &'static str,
}

fn settable() -> &'static [SettableSpec] {
    &[
        SettableSpec {
            key: "ssrf.level",
            parser: parse_ssrf_level,
            expected: "one of: strict, loopback, project, lan, none",
        },
        SettableSpec {
            key: "ssrf.project_root",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "fetch.user_agent",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "fetch.timeout_secs",
            parser: parse_int,
            expected: "integer (seconds)",
        },
        SettableSpec {
            key: "cache.default_ttl",
            parser: parse_string,
            expected: "humantime string (e.g. \"1h\")",
        },
        SettableSpec {
            key: "cache.min_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "cache.max_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "cache.override_no_store",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "cache.store_raw_html",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "robots.respect",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "robots.default_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "robots.failure_ttl",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "rate_limit.requests_per_minute_per_domain",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "rate_limit.per_domain_concurrency",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "rate_limit.global_concurrency",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "rate_limit.max_retries",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "tokenizer.default",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "output.dir",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "summarization.default_backend",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "summarization.default_mode",
            parser: parse_summarization_mode,
            expected: "one of: abstractive, extractive, headlines",
        },
        SettableSpec {
            key: "summarization.default_style",
            parser: parse_summarization_style,
            expected: "one of: bullet, prose, executive",
        },
        SettableSpec {
            key: "summarization.fallback_to_extractive",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "summarization.tables.target_tokens",
            parser: parse_int,
            expected: "integer",
        },
        SettableSpec {
            key: "summarization.tables.focus",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "debug.har_path",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "debug.har_body_cap",
            parser: parse_string,
            expected: "humansize string or integer",
        },
        SettableSpec {
            key: "debug.log_level",
            parser: parse_log_level,
            expected: "one of: trace, debug, info, warn, error",
        },
        SettableSpec {
            key: "headless.max_concurrent",
            parser: parse_usize,
            expected: "integer",
        },
        SettableSpec {
            key: "headless.chrome_executable",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "image_captions.default",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "image_captions.max_tokens",
            parser: parse_usize,
            expected: "integer",
        },
        SettableSpec {
            key: "image_captions.max_per_page",
            parser: parse_usize,
            expected: "integer",
        },
        SettableSpec {
            key: "image_captions.min_width",
            parser: parse_u32,
            expected: "integer",
        },
        SettableSpec {
            key: "image_captions.min_height",
            parser: parse_u32,
            expected: "integer",
        },
        SettableSpec {
            key: "image_captions.max_bytes",
            parser: parse_human_bytes_v,
            expected: "humansize string or integer (e.g. \"10MiB\")",
        },
        SettableSpec {
            key: "image_captions.cache.enabled",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "image_captions.cache.ttl",
            parser: parse_string,
            expected: "humantime string (e.g. \"1h\") or empty string for inherit",
        },
        SettableSpec {
            key: "image_captions.cache.restrict_to",
            parser: parse_cache_restrict,
            expected: "one of: none, host, page",
        },
        SettableSpec {
            key: "image_captions.cache.store_raw_image",
            parser: parse_bool,
            expected: "bool",
        },
        SettableSpec {
            key: "mcp.heartbeat_interval",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "mcp.reap_threshold",
            parser: parse_string,
            expected: "humantime string",
        },
        SettableSpec {
            key: "http.bind",
            parser: parse_string,
            expected: "string",
        },
        SettableSpec {
            key: "http.allow_server_paths",
            parser: parse_bool,
            expected: "bool",
        },
    ]
}

/// The dotted keys `rover config set` accepts. Exposed so tests can assert
/// this list, `provenance::known_leaves()`, and the docs stay in agreement.
#[must_use]
pub fn settable_keys() -> Vec<&'static str> {
    settable().iter().map(|s| s.key).collect()
}

fn parse_string(s: &str) -> Result<toml_edit::Item, String> {
    Ok(toml_edit::value(s.to_string()))
}

fn parse_int(s: &str) -> Result<toml_edit::Item, String> {
    let n: i64 = s.parse().map_err(|_| format!("not an integer: {s}"))?;
    Ok(toml_edit::value(n))
}

fn parse_usize(s: &str) -> Result<toml_edit::Item, String> {
    let n: i64 = s
        .parse::<usize>()
        .map(|u| u as i64)
        .map_err(|_| format!("not a non-negative integer: {s}"))?;
    Ok(toml_edit::value(n))
}

fn parse_u32(s: &str) -> Result<toml_edit::Item, String> {
    let n: i64 = s
        .parse::<u32>()
        .map(|u| u as i64)
        .map_err(|_| format!("not a non-negative 32-bit integer: {s}"))?;
    Ok(toml_edit::value(n))
}

fn parse_human_bytes_v(v: &str) -> Result<toml_edit::Item, String> {
    // Validate that the string parses correctly, but persist it as a string
    // so that the config file stays human-readable (e.g. "10MiB").
    crate::config::parse_human_bytes(v)?;
    Ok(toml_edit::value(v.to_string()))
}

fn parse_bool(s: &str) -> Result<toml_edit::Item, String> {
    let b = match s {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => return Err(format!("not a bool: {s}")),
    };
    Ok(toml_edit::value(b))
}

fn parse_ssrf_level(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "strict" | "loopback" | "project" | "lan" | "none" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid ssrf level: {s}")),
    }
}

fn parse_summarization_mode(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "abstractive" | "extractive" | "headlines" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid summarization mode: {s}")),
    }
}

fn parse_summarization_style(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "bullet" | "prose" | "executive" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid summarization style: {s}")),
    }
}

fn parse_log_level(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "trace" | "debug" | "info" | "warn" | "error" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid log level: {s}")),
    }
}

fn parse_cache_restrict(s: &str) -> Result<toml_edit::Item, String> {
    match s {
        "none" | "host" | "page" => Ok(toml_edit::value(s.to_string())),
        _ => Err(format!("not a valid cache restrict value: {s}")),
    }
}

pub fn apply_set(path: &Path, key: &str, value: &str) -> Result<(), SetError> {
    let spec = settable()
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| SetError::Unsettable {
            key: key.to_string(),
        })?;
    let item = (spec.parser)(value).map_err(|_e| SetError::Parse {
        key: key.to_string(),
        value: value.to_string(),
        expected: spec.expected.to_string(),
    })?;

    let original = std::fs::read_to_string(path).map_err(|source| SetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut doc: toml_edit::DocumentMut =
        original
            .parse()
            .map_err(|source| SetError::ParseExistingFile {
                path: path.to_path_buf(),
                source,
            })?;

    // Walk to the parent table, creating intermediates as needed.
    let parts: Vec<&str> = key.split('.').collect();
    let (leaf, parents) = parts.split_last().expect("non-empty key");
    let mut cursor: &mut toml_edit::Table = doc.as_table_mut();
    for p in parents {
        if !cursor.contains_key(p) {
            cursor.insert(p, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        cursor = cursor
            .get_mut(p)
            .and_then(|i| i.as_table_mut())
            .ok_or_else(|| SetError::Parse {
                key: key.to_string(),
                value: value.to_string(),
                expected: format!("parent `{p}` is not a table"),
            })?;
    }
    // Preserve any existing decor (leading/trailing comments and whitespace)
    // on the value being overwritten.
    let mut new_item = item;
    if let Some(existing) = cursor.get(leaf)
        && let (Some(existing_val), Some(new_val)) = (existing.as_value(), new_item.as_value_mut())
    {
        let old_decor = existing_val.decor().clone();
        *new_val.decor_mut() = old_decor;
    }
    cursor.insert(leaf, new_item);

    // Serialize, validate by round-trip through Config::deserialize.
    let new_text = doc.to_string();
    let _: crate::config::Config =
        toml::from_str(&new_text).map_err(|source| SetError::Validation {
            key: key.to_string(),
            value: value.to_string(),
            message: source.to_string(),
        })?;

    // Write. Original is preserved by not touching the file on the failure path.
    std::fs::write(path, new_text).map_err(|source| SetError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn unknown_key_is_rejected() {
        let r = apply_set(std::path::Path::new("/dev/null"), "bogus.key", "x");
        assert!(matches!(r, Err(SetError::Unsettable { .. })));
    }

    #[test]
    fn set_writes_value_and_preserves_comments() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        std::fs::write(
            &p,
            "# header comment\n[ssrf]\nlevel = \"strict\" # was strict\n",
        )
        .unwrap();
        apply_set(&p, "ssrf.level", "loopback").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(
            after.contains("# header comment"),
            "header dropped: {after}"
        );
        assert!(
            after.contains("level = \"loopback\""),
            "value not updated: {after}"
        );
        assert!(
            after.contains("# was strict"),
            "trailing comment dropped: {after}"
        );
    }

    #[test]
    fn set_invalid_value_does_not_modify_file() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        let original = "[ssrf]\nlevel = \"strict\"\n";
        std::fs::write(&p, original).unwrap();
        let r = apply_set(&p, "ssrf.level", "bogus");
        assert!(matches!(r, Err(SetError::Parse { .. })), "{r:?}");
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, original, "file modified despite parse failure");
    }

    #[test]
    fn set_creates_missing_section() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        std::fs::write(&p, "").unwrap();
        apply_set(&p, "ssrf.level", "loopback").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("[ssrf]"));
        assert!(after.contains("level = \"loopback\""));
    }

    #[test]
    fn set_bool_value_parses_common_forms() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("rover.toml");
        std::fs::write(&p, "").unwrap();
        apply_set(&p, "robots.respect", "false").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("respect = false"));
    }
}

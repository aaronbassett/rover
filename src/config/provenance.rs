//! Provenance tracking for `rover config show`.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    Env,
}

#[derive(Debug, Clone)]
pub struct ProvenanceRow {
    pub dotted: String,
    pub source: Source,
}

/// Compute provenance by parsing the file as a generic toml::Value and
/// walking known leaf keys. Any leaf present in the file is marked `File`;
/// the rest default to `Default`.
pub fn provenance_for(file_toml: &str) -> Vec<ProvenanceRow> {
    provenance_for_with_env(file_toml, env_overrides())
}

pub fn provenance_for_with_env(
    file_toml: &str,
    env_table: &[(&'static str, &'static str)],
) -> Vec<ProvenanceRow> {
    let v: toml::Value =
        toml::from_str(file_toml).unwrap_or(toml::Value::Table(Default::default()));
    let mut file_leaves: HashSet<String> = HashSet::new();
    walk_leaves(&v, "", &mut file_leaves);

    let env_leaves: HashSet<String> = env_table
        .iter()
        .filter(|(_, var)| std::env::var(var).map(|s| !s.is_empty()).unwrap_or(false))
        .map(|(key, _)| (*key).to_string())
        .collect();

    let mut rows = Vec::new();
    for dotted in known_leaves() {
        let source = if env_leaves.contains(*dotted) {
            Source::Env
        } else if file_leaves.contains(*dotted) {
            Source::File
        } else {
            Source::Default
        };
        rows.push(ProvenanceRow {
            dotted: (*dotted).to_string(),
            source,
        });
    }
    rows
}

fn walk_leaves(v: &toml::Value, prefix: &str, out: &mut HashSet<String>) {
    if let toml::Value::Table(t) = v {
        for (k, child) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match child {
                toml::Value::Table(_) => walk_leaves(child, &key, out),
                _ => {
                    out.insert(key);
                }
            }
        }
    }
}

/// The list of leaf keys `rover config show` reports on. Kept in sync
/// with `Config`'s struct fields by hand — schemars-style introspection
/// is out of scope for M8.
pub fn known_leaves() -> &'static [&'static str] {
    &[
        "fetch.user_agent",
        "fetch.timeout_secs",
        "ssrf.level",
        "ssrf.project_root",
        "cache.default_ttl",
        "cache.min_ttl",
        "cache.max_ttl",
        "cache.override_no_store",
        "cache.store_raw_html",
        "robots.respect",
        "robots.default_ttl",
        "rate_limit.requests_per_minute_per_domain",
        "rate_limit.per_domain_concurrency",
        "rate_limit.global_concurrency",
        "tokenizer.default",
        "output.dir",
        "summarization.default_backend",
        "summarization.default_mode",
        "summarization.default_style",
        "summarization.fallback_to_extractive",
        "summarization.tables.target_tokens",
        "summarization.tables.focus",
        "debug.har_path",
        "debug.har_body_cap",
        "debug.log_level",
        "http.bind",
        "http.allowed_hosts",
        "http.allowed_origins",
        "http.allow_server_paths",
        "mcp.heartbeat_interval",
        "mcp.reap_threshold",
        "rate_limit.max_retries",
        "robots.failure_ttl",
        "headless.max_concurrent",
        "headless.chrome_executable",
        "image_captions.default",
        "image_captions.max_tokens",
        "image_captions.max_per_page",
        "image_captions.min_width",
        "image_captions.min_height",
        "image_captions.max_bytes",
        "image_captions.cache.enabled",
        "image_captions.cache.ttl",
        "image_captions.cache.restrict_to",
        "image_captions.cache.store_raw_image",
    ]
}

/// Map of leaf key → env var that overrides it.
pub fn env_overrides() -> &'static [(&'static str, &'static str)] {
    &[
        ("debug.log_level", "ROVER_LOG_LEVEL"),
        ("output.dir", "ROVER_OUTPUT_DIR"),
        ("http.bind", "ROVER_HTTP_BIND"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_marks_defaults_vs_file() {
        let toml = r#"
[ssrf]
level = "loopback"
"#;
        let provenance = provenance_for(toml);
        let level_row = provenance
            .iter()
            .find(|r| r.dotted == "ssrf.level")
            .expect("ssrf.level present");
        assert_eq!(level_row.source, Source::File);
        let default_row = provenance
            .iter()
            .find(|r| r.dotted == "ssrf.project_root")
            .expect("ssrf.project_root present");
        assert_eq!(default_row.source, Source::Default);
    }

    #[test]
    fn provenance_recognizes_env_override() {
        let toml = "";
        // SAFETY: serialized by virtue of using a probe-specific env var
        // unlikely to collide with parallel tests.
        unsafe { std::env::set_var("ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE", "debug") };
        let rows = provenance_for_with_env(
            toml,
            &[("debug.log_level", "ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE")],
        );
        let r = rows.iter().find(|r| r.dotted == "debug.log_level").unwrap();
        assert_eq!(r.source, Source::Env);
        unsafe { std::env::remove_var("ROVER_LOG_LEVEL_TEST_OVERRIDE_PROBE") };
    }
}

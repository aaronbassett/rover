//! Subprocess tests for `rover config show` and `rover config set`.
#![cfg(feature = "test-loopback")]

use std::process::Command;
use tempfile::tempdir;

fn rover_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rover"))
}

#[test]
fn config_show_marks_provenance() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"loopback\"\n").unwrap();
    let out = Command::new(rover_bin())
        .arg("--config")
        .arg(&cfg)
        .arg("config")
        .arg("show")
        .output()
        .expect("rover config show");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    assert!(
        stdout.contains("ssrf.level")
            && stdout.contains("loopback")
            && stdout.contains("# from: file"),
        "expected ssrf.level marked file; got:\n{stdout}",
    );
    assert!(
        stdout.contains("ssrf.project_root") && stdout.contains("# from: defaults"),
        "expected project_root marked defaults; got:\n{stdout}",
    );
}

#[test]
fn config_set_writes_value() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"strict\"\n").unwrap();
    let out = Command::new(rover_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["config", "set", "ssrf.level", "loopback"])
        .output()
        .expect("rover config set");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("level = \"loopback\""), "file:\n{after}");
}

#[test]
fn config_set_rejects_unknown_key() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "").unwrap();
    let out = Command::new(rover_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["config", "set", "bogus.field", "x"])
        .output()
        .expect("rover config set");
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not settable") || stderr.contains("Unsettable"),
        "stderr:\n{stderr}",
    );
}

#[test]
fn config_set_rejects_invalid_enum_value() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[ssrf]\nlevel = \"strict\"\n").unwrap();
    let out = Command::new(rover_bin())
        .args(["--config"])
        .arg(&cfg)
        .args(["config", "set", "ssrf.level", "bogus"])
        .output()
        .expect("rover config set");
    assert!(!out.status.success(), "expected non-zero exit");
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("strict"), "file modified: {after}");
}

/// Every key `rover config set` accepts must be a known provenance leaf, and
/// every key documented in `site/docs/cli.md`'s settable list must be
/// settable. Nothing tied these three lists together before, and they had
/// already drifted — `mcp.*`, `headless.*`, `image_captions.*`,
/// `rate_limit.max_retries`, and `robots.failure_ttl` were documented and
/// settable but absent from `known_leaves()`.
#[test]
fn settable_keys_docs_and_provenance_agree() {
    let leaves: std::collections::HashSet<&str> = rover::config::provenance::known_leaves()
        .iter()
        .copied()
        .collect();

    // 1. Every settable key is a known provenance leaf.
    for key in rover::config::edit::settable_keys() {
        assert!(
            leaves.contains(key),
            "`{key}` is settable via `rover config set` but missing from known_leaves()"
        );
    }

    // 2. Every key in cli.md's settable list is actually settable.
    let cli_md = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("site/docs/cli.md"),
    )
    .expect("read site/docs/cli.md");
    let settable: std::collections::HashSet<&str> = rover::config::edit::settable_keys()
        .iter()
        .copied()
        .collect();

    let documented = documented_settable_keys(&cli_md);
    assert!(
        !documented.is_empty(),
        "parsed zero settable keys from cli.md — the parser or the docs heading changed"
    );
    for key in documented {
        assert!(
            settable.contains(key.as_str()),
            "`{key}` is documented in cli.md as settable but `settable()` rejects it"
        );
    }
}

/// Pull backtick-quoted dotted keys out of the bullet list that follows the
/// `rover config set` settable-keys heading in cli.md.
fn documented_settable_keys(cli_md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in cli_md.lines() {
        if line.contains("Settable keys") {
            in_list = true;
            continue;
        }
        if in_list {
            if line.starts_with("- ") {
                for chunk in line.split('`').skip(1).step_by(2) {
                    if chunk.contains('.') && !chunk.contains(' ') {
                        out.push(chunk.to_string());
                    }
                }
            } else if !line.trim().is_empty() {
                break;
            }
        }
    }
    out
}

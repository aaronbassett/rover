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

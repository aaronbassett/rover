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

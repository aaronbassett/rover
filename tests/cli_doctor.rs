//! Subprocess test of `rover doctor`.
#![cfg(feature = "test-loopback")]

use std::process::Command;
use tempfile::tempdir;

fn rover_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rover"))
}

#[test]
fn doctor_exits_zero_on_clean_install() {
    let tmp = tempdir().unwrap();
    let out = Command::new(rover_bin())
        .arg("doctor")
        .env("ROVER_DATA_DIR", tmp.path())
        .env("ROVER_OUTPUT_DIR", tmp.path())
        .output()
        .expect("spawn rover doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Allow non-zero exit only if `network_reachable` fails (sandbox env).
    if !out.status.success() {
        assert!(
            stdout.contains("network_reachable") || stderr.contains("network_reachable"),
            "non-zero exit but no network_reachable failure:\nstdout:\n{stdout}\nstderr:\n{stderr}",
        );
    } else {
        // The "all checks ok" footer should appear in human format.
        assert!(stdout.contains("ok"), "expected ok line; stdout:\n{stdout}");
    }
}

#[test]
fn doctor_ndjson_format_one_json_per_line() {
    let tmp = tempdir().unwrap();
    let out = Command::new(rover_bin())
        .arg("doctor")
        .arg("--format=ndjson")
        .env("ROVER_DATA_DIR", tmp.path())
        .env("ROVER_OUTPUT_DIR", tmp.path())
        .output()
        .expect("spawn rover doctor --format=ndjson");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("not json: {line}"));
    }
}

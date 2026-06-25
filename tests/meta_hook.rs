//! Subprocess tests for `rover meta hook claude` (stdin → stdout JSON).

use assert_cmd::Command;

#[test]
fn session_start_emits_context() {
    let assert = Command::new(env!("CARGO_BIN_EXE_rover"))
        .args(["meta", "hook", "claude"])
        .write_stdin(r#"{"hook_event_name":"SessionStart"}"#)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("SessionStart"), "stdout: {stdout}");
    assert!(stdout.contains("mcp__rover__fetch"), "stdout: {stdout}");
}

#[test]
fn pretooluse_reminder_has_no_permission_decision() {
    let assert = Command::new(env!("CARGO_BIN_EXE_rover"))
        .args(["meta", "hook", "claude"])
        .write_stdin(r#"{"hook_event_name":"PreToolUse","tool_name":"WebFetch"}"#)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("PreToolUse"), "stdout: {stdout}");
    assert!(!stdout.contains("permissionDecision"), "stdout: {stdout}");
}

#[test]
fn unknown_event_prints_nothing() {
    let assert = Command::new(env!("CARGO_BIN_EXE_rover"))
        .args(["meta", "hook", "claude"])
        .write_stdin(r#"{"hook_event_name":"Stop"}"#)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.trim().is_empty(), "stdout: {stdout}");
}

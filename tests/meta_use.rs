//! Subprocess tests for `rover meta use`.

use std::process::Command;

use tempfile::tempdir;

fn rover_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rover"))
}

#[test]
fn general_writes_mcp_and_agents_idempotently() {
    let tmp = tempdir().unwrap();
    let run = || {
        Command::new(rover_bin())
            .current_dir(tmp.path())
            .args(["meta", "use", "general"])
            .output()
            .expect("rover meta use general")
    };

    let out = run();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mcp = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
    assert!(
        mcp.contains("mcpServers") && mcp.contains("rover"),
        "mcp.json:\n{mcp}"
    );
    let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("prefer Rover"), "AGENTS.md:\n{agents}");

    // Re-run: no duplication.
    assert!(run().status.success());
    let agents2 = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert_eq!(
        agents2.matches("rover:begin").count(),
        1,
        "AGENTS.md:\n{agents2}"
    );
}

#[test]
fn claude_aborts_cleanly_when_binary_missing() {
    let tmp = tempdir().unwrap();
    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", "/nonexistent/claude-xyz-rover-test")
        .args(["meta", "use", "claude"])
        .output()
        .expect("rover meta use claude");
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Assert the specific preflight abort message, not merely the word "claude"
    // (which also appears in the bogus binary path we set above).
    assert!(
        stderr.contains("was not found on PATH"),
        "stderr:\n{stderr}"
    );
    // Validate-then-apply: nothing was written.
    assert!(!tmp.path().join(".claude").exists());
    assert!(!tmp.path().join("CLAUDE.md").exists());
}

#[cfg(unix)]
#[test]
fn claude_happy_path_with_stub_binary() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().unwrap();
    // A stub `claude`: --version succeeds; `mcp get` reports "not registered"
    // (exit 1); `mcp add` succeeds.
    let stub = tmp.path().join("claude-stub.sh");
    std::fs::write(
        &stub,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'claude-stub 0.0.0'; exit 0; fi\n\
         if [ \"$1\" = \"mcp\" ] && [ \"$2\" = \"get\" ]; then exit 1; fi\n\
         if [ \"$1\" = \"mcp\" ] && [ \"$2\" = \"add\" ]; then exit 0; fi\n\
         exit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", &stub)
        .args(["meta", "use", "claude", "--scope", "project"])
        .output()
        .expect("rover meta use claude");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Hooks landed in .claude/settings.json and parse.
    let settings =
        std::fs::read_to_string(tmp.path().join(".claude").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "WebFetch");
    assert_eq!(
        v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "rover meta hook claude"
    );
    // CLAUDE.md block written at project scope.
    let md = std::fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap();
    assert!(md.contains("prefer Rover"), "CLAUDE.md:\n{md}");

    // Idempotent re-run: still exactly one SessionStart group.
    let out2 = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", &stub)
        .args(["meta", "use", "claude", "--scope", "project"])
        .output()
        .expect("rover meta use claude (re-run)");
    assert!(out2.status.success());
    let settings2 =
        std::fs::read_to_string(tmp.path().join(".claude").join("settings.json")).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&settings2).unwrap();
    assert_eq!(v2["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
}

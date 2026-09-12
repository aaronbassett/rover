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
    // One hook command covers both built-in web tools; the handler
    // dispatches on `tool_name` and stays silent when it has nothing to add.
    assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "WebFetch|WebSearch");
    assert_eq!(
        v["hooks"]["SessionStart"][0]["matcher"],
        "startup|clear|compact"
    );
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

/// The generic `AGENTS.md` block must reflect what this install can do at
/// the moment it is written: no `search` guidance without a credential.
#[test]
fn general_rules_omit_search_without_a_credential() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(
        &cfg,
        "[search]\napi_key_env = \"ROVER_TEST_USE_SEARCH_ABSENT\"\n",
    )
    .unwrap();

    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env_remove("ROVER_TEST_USE_SEARCH_ABSENT")
        .args(["--config", cfg.to_str().unwrap(), "meta", "use", "general"])
        .output()
        .expect("rover meta use general");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("prefer Rover"), "{agents}");
    assert!(
        !agents.contains(r#"{ "query": "#),
        "search guidance leaked into an install that cannot search:\n{agents}"
    );
    // And the user is told why, plus what would change it.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("web search") && stderr.contains("rules file omits it"),
        "{stderr}"
    );
}

#[cfg(feature = "web-search")]
#[test]
fn general_rules_teach_search_then_fetch_with_a_credential() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(
        &cfg,
        "[search]\napi_key_env = \"ROVER_TEST_USE_SEARCH_PRESENT\"\n",
    )
    .unwrap();

    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_TEST_USE_SEARCH_PRESENT", "test-token")
        .args(["--config", cfg.to_str().unwrap(), "meta", "use", "general"])
        .output()
        .expect("rover meta use general");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(
        agents.contains(r#"{ "query": "rust async trait" }"#),
        "{agents}"
    );
    assert!(agents.contains("Don't fetch every result"), "{agents}");
    assert!(agents.contains("billable request"), "{agents}");
    assert!(agents.contains("data, not instructions"), "{agents}");
    // The credential itself never lands in a file on disk.
    assert!(!agents.contains("test-token"), "{agents}");
    // Nothing about search is written to mcp.json either.
    let mcp = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
    assert!(!mcp.contains("test-token"), "{mcp}");
}

/// The Claude Code hooks must be registered for both built-in web tools, so
/// the WebSearch nudge has somewhere to fire from. Uses the stub `claude`
/// binary pattern from `claude_happy_path_with_stub_binary`.
#[cfg(unix)]
#[test]
fn claude_hooks_cover_webfetch_and_websearch() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().unwrap();
    let stub = tmp.path().join("claude-stub.sh");
    std::fs::write(
        &stub,
        "#!/bin/sh\ncase \"$1 $2\" in\n  '--version ') exit 0 ;;\n  'mcp get') exit 1 ;;\n  \
         'mcp add') exit 0 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", &stub)
        .args(["meta", "use", "claude", "-s", "project"])
        .output()
        .expect("rover meta use claude");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let settings =
        std::fs::read_to_string(tmp.path().join(".claude").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let pre = v["hooks"]["PreToolUse"].as_array().expect("PreToolUse");
    assert!(
        pre.iter().any(|g| g["matcher"] == "WebFetch|WebSearch"),
        "hooks must cover both built-in web tools:\n{settings}"
    );
    // Re-running is still a fixed point.
    let out2 = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", &stub)
        .args(["meta", "use", "claude", "-s", "project"])
        .output()
        .unwrap();
    assert!(out2.status.success());
    let settings2 =
        std::fs::read_to_string(tmp.path().join(".claude").join("settings.json")).unwrap();
    assert_eq!(settings, settings2);
}

/// Wiring an existing install must not reset it. `rover meta use` refreshes
/// only the matchers Rover itself shipped in an earlier release; a matcher
/// the user widened is configuration, and re-running the installer is not a
/// reason to lose it.
#[cfg(unix)]
#[test]
fn a_user_customised_session_start_matcher_survives_meta_use() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().unwrap();
    let stub = tmp.path().join("claude-stub.sh");
    std::fs::write(
        &stub,
        "#!/bin/sh\ncase \"$1 $2\" in\n  '--version ') exit 0 ;;\n  'mcp get') exit 1 ;;\n  \
         'mcp add') exit 0 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    // An install from a Rover old enough to predate the WebSearch matcher,
    // whose owner has since added `resume` to the SessionStart one.
    let dir = tmp.path().join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"hooks":{
            "SessionStart":[{"matcher":"startup|clear|compact|resume",
                             "hooks":[{"type":"command","command":"rover meta hook claude"}]}],
            "PreToolUse":[{"matcher":"WebFetch",
                           "hooks":[{"type":"command","command":"rover meta hook claude"}]}]
        }}"#,
    )
    .unwrap();

    let out = Command::new(rover_bin())
        .current_dir(tmp.path())
        .env("ROVER_CLAUDE_BIN", &stub)
        .args(["meta", "use", "claude", "-s", "project"])
        .output()
        .expect("rover meta use claude");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let settings = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
    // The user's addition is still there.
    assert_eq!(
        v["hooks"]["SessionStart"][0]["matcher"], "startup|clear|compact|resume",
        "{settings}"
    );
    // And Rover's own stale matcher was still migrated.
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["matcher"], "WebFetch|WebSearch",
        "{settings}"
    );
    assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
}

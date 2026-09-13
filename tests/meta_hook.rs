//! Subprocess tests for `rover meta hook claude` (stdin → stdout JSON).
//!
//! These run the real binary, so they assert the steering an agent actually
//! receives — including the capability gating that keeps `search` out of the
//! steering in a build (or an install) that cannot run it.

use assert_cmd::Command;

const KEY: &str = "test-subscription-token-do-not-use";

/// Run the hook with a config file that names `env_var` as the search
/// credential variable, optionally exporting a value for it.
fn hook_with(event_json: &str, env_var: &str, key: Option<&str>) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, format!("[search]\napi_key_env = \"{env_var}\"\n")).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rover"));
    cmd.args(["--config", cfg.to_str().unwrap(), "meta", "hook", "claude"]);
    match key {
        Some(k) => cmd.env(env_var, k),
        None => cmd.env_remove(env_var),
    };
    let assert = cmd.write_stdin(event_json.to_string()).assert().success();
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn context_of(stdout: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(stdout).expect("hook output is JSON");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext")
        .to_string()
}

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

/// The steering must not advertise `search` when this install cannot run
/// it. Regression guard for the exact failure this feature could introduce:
/// a confident recommendation for a tool call that can only fail.
#[test]
fn session_start_omits_search_when_it_is_unavailable() {
    let out = hook_with(
        r#"{"hook_event_name":"SessionStart"}"#,
        "ROVER_TEST_HOOK_SEARCH_ABSENT",
        None,
    );
    let ctx = context_of(&out);
    assert!(
        !ctx.contains("mcp__rover__search_tool"),
        "advertised an unavailable tool:\n{ctx}"
    );
    // The deferred-tool list must not name it either.
    assert!(
        ctx.contains("ToolSearch  select:mcp__rover__fetch_tool"),
        "{ctx}"
    );
    // The pre-search guidance survives untouched.
    assert!(
        ctx.contains("Keep using WebSearch to discover URLs"),
        "{ctx}"
    );
    // Fetch steering is unaffected.
    assert!(ctx.contains("mcp__rover__fetch_tool"), "{ctx}");
}

/// And it must advertise `search` — with the search → fetch workflow — as
/// soon as the install can actually run it. Hooks re-evaluate per session,
/// so exporting the key is enough; no `rover meta use` re-run needed.
#[cfg(feature = "web-search")]
#[test]
fn session_start_teaches_search_then_fetch_once_a_key_is_present() {
    let out = hook_with(
        r#"{"hook_event_name":"SessionStart"}"#,
        "ROVER_TEST_HOOK_SEARCH_PRESENT",
        Some(KEY),
    );
    let ctx = context_of(&out);
    assert!(ctx.contains("mcp__rover__search_tool"), "{ctx}");
    assert!(
        ctx.contains("ToolSearch  select:mcp__rover__search_tool,mcp__rover__fetch_tool"),
        "{ctx}"
    );
    assert!(ctx.contains(r#"{ "query": "rust async trait" }"#), "{ctx}");
    // The workflow, and the rule that stops search becoming a scrape.
    assert!(ctx.contains("Do NOT fetch every result"), "{ctx}");
    // Prefer Rover's search over the built-in one.
    assert!(
        ctx.contains("Prefer mcp__rover__search_tool over the built-in WebSearch"),
        "{ctx}"
    );
    // Snippets are named as untrusted third-party content.
    assert!(ctx.contains("3rd-party web content"), "{ctx}");
    // The credential never appears in steering.
    assert!(
        !ctx.contains(KEY),
        "credential leaked into steering:\n{ctx}"
    );
}

/// The WebSearch PreToolUse nudge is capability-gated: silent when Rover
/// search cannot run, so the built-in proceeds untouched.
#[test]
fn websearch_nudge_is_silent_when_rover_search_is_unavailable() {
    let out = hook_with(
        r#"{"hook_event_name":"PreToolUse","tool_name":"WebSearch"}"#,
        "ROVER_TEST_HOOK_SEARCH_ABSENT_2",
        None,
    );
    assert!(out.trim().is_empty(), "expected no output, got: {out}");
}

#[cfg(feature = "web-search")]
#[test]
fn websearch_nudge_points_at_rover_search_and_never_blocks() {
    let out = hook_with(
        r#"{"hook_event_name":"PreToolUse","tool_name":"WebSearch"}"#,
        "ROVER_TEST_HOOK_SEARCH_PRESENT_2",
        Some(KEY),
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    // Rover nudges; it never auto-allows or blocks.
    assert!(
        v["hookSpecificOutput"].get("permissionDecision").is_none(),
        "{v}"
    );
    let ctx = context_of(&out);
    assert!(ctx.contains("mcp__rover__search_tool"), "{ctx}");
    assert!(ctx.contains("mcp__rover__fetch_tool"), "{ctx}");
    assert!(ctx.contains("search does not fetch anything"), "{ctx}");
    assert!(ctx.contains("Proceeding with WebSearch"), "{ctx}");
}

/// The WebFetch nudge is unconditional and unchanged by this feature.
#[test]
fn webfetch_nudge_is_unchanged_in_every_capability_state() {
    for (var, key) in [
        ("ROVER_TEST_HOOK_WEBFETCH_ABSENT", None),
        ("ROVER_TEST_HOOK_WEBFETCH_PRESENT", Some(KEY)),
    ] {
        let out = hook_with(
            r#"{"hook_event_name":"PreToolUse","tool_name":"WebFetch"}"#,
            var,
            key,
        );
        let ctx = context_of(&out);
        assert!(ctx.contains("mcp__rover__fetch_tool"), "{ctx}");
        assert!(ctx.contains("Proceeding with WebFetch"), "{ctx}");
    }
}

/// A broken `rover.toml` must not break the user's Claude Code session:
/// the hook degrades to default capabilities and still emits steering.
#[test]
fn a_broken_config_degrades_the_hook_instead_of_failing_the_session() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("rover.toml");
    std::fs::write(&cfg, "[search]\ncount = 999\n").unwrap();

    let assert = Command::new(env!("CARGO_BIN_EXE_rover"))
        .args(["--config", cfg.to_str().unwrap(), "meta", "hook", "claude"])
        .write_stdin(r#"{"hook_event_name":"SessionStart"}"#)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let ctx = context_of(&stdout);
    assert!(ctx.contains("mcp__rover__fetch_tool"), "{ctx}");
}

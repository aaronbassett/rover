//! Runtime hook handler and steering-text constants.

/// Injected at SessionStart to steer the agent toward Rover for reads.
pub const SESSION_START_CONTEXT: &str = "Rover is wired in as an MCP server and is the \
preferred way to read web pages. When you need the contents of a URL, use Rover instead of the \
built-in WebFetch: `mcp__rover__fetch` (one URL → clean, token-budgeted, prompt-injection-guarded \
Markdown, cached), `mcp__rover__batch_fetch` (many URLs), plus `mcp__rover__summarize`, \
`mcp__rover__get_metadata`, and `mcp__rover__count_tokens`. WebFetch returns a lossy per-prompt \
answer; Rover returns a reusable, guarded document. Keep using WebSearch to discover URLs, then \
fetch them with Rover rather than WebFetch. Use WebFetch only when Rover is unavailable.";

/// Injected before a WebFetch call as a non-blocking nudge.
pub const PRETOOL_REMINDER: &str = "Rover is available: `mcp__rover__fetch` returns cleaner, \
token-budgeted, prompt-injection-guarded Markdown than WebFetch and caches it \
(`mcp__rover__batch_fetch` for many URLs). Consider using Rover instead. Proceeding with WebFetch.";

/// Managed rules-block body for Claude Code (`CLAUDE.md`). Markers added by `upsert_managed_block`.
pub const RULES_BLOCK_CLAUDE: &str = "## Web fetching: prefer Rover\n\n\
Rover is wired in as an MCP server. When you need to **read a web page**, prefer Rover over the \
built-in `WebFetch`:\n\n\
- `mcp__rover__fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)\n\
- `mcp__rover__batch_fetch` — many URLs concurrently\n\
- `mcp__rover__summarize`, `mcp__rover__get_metadata`, `mcp__rover__count_tokens`\n\n\
`WebFetch` returns a lossy, per-prompt answer; Rover returns a reusable, guarded document. Keep \
using `WebSearch` to *find* URLs — then fetch them with Rover, not `WebFetch`. Use `WebFetch` only \
when Rover is unavailable.";

/// Managed rules-block body for the generic `AGENTS.md` (tool names harness-prefixed).
pub const RULES_BLOCK_GENERAL: &str = "## Web fetching: prefer Rover\n\n\
A `rover` MCP server is configured in `mcp.json`. When you need to **read a web page**, prefer its \
tools over any built-in web-fetch tool:\n\n\
- `fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)\n\
- `batch_fetch` — many URLs concurrently\n\
- `summarize`, `get_metadata`, `count_tokens`\n\n\
Tool names may be prefixed by your harness (e.g. `rover.fetch` or `mcp__rover__fetch`). A built-in \
fetch returns a lossy per-prompt answer; Rover returns a reusable, guarded document. If your \
harness doesn't auto-load `mcp.json`, register the `rover` server from it manually.";

/// Handle a Claude Code hook payload (stdin JSON) and return the response JSON
/// to print on stdout, or `""` for events we don't handle / unparseable input.
pub fn handle_claude_hook(stdin_json: &str) -> String {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin_json) else {
        return String::new();
    };
    let event = payload
        .get("hook_event_name")
        .and_then(|e| e.as_str())
        .unwrap_or_default();

    let response = match event {
        "SessionStart" => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": SESSION_START_CONTEXT,
            }
        }),
        "PreToolUse" => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": PRETOOL_REMINDER,
            }
        }),
        _ => return String::new(),
    };

    response.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_emits_additional_context() {
        let out = handle_claude_hook(r#"{"hook_event_name":"SessionStart"}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("mcp__rover__fetch")
        );
    }

    #[test]
    fn pretooluse_reminder_has_no_permission_decision() {
        let out = handle_claude_hook(r#"{"hook_event_name":"PreToolUse","tool_name":"WebFetch"}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(v["hookSpecificOutput"]["additionalContext"].is_string());
        // Critical: never auto-allow / block — no permissionDecision field.
        assert!(v["hookSpecificOutput"].get("permissionDecision").is_none());
    }

    #[test]
    fn unknown_event_is_empty() {
        assert_eq!(handle_claude_hook(r#"{"hook_event_name":"Stop"}"#), "");
    }

    #[test]
    fn unparseable_input_is_empty() {
        assert_eq!(handle_claude_hook("not json"), "");
    }

    #[test]
    fn steering_is_webfetch_only() {
        for s in [SESSION_START_CONTEXT, PRETOOL_REMINDER, RULES_BLOCK_CLAUDE] {
            assert!(s.contains("WebFetch"));
            assert!(!s.contains("WebSearch") || s.contains("Keep using"));
        }
    }
}

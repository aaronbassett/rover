---
id: quickstart
title: Quickstart
---

# Quickstart

Wire Rover into your agent, then fetch a page. This assumes `rover` is installed; if it isn't, start with [Installation](/docs/install).

`rover meta use` does the wiring for you — MCP registration, steering hooks, and a rules-file block — in one command. Use it for Claude Code or any harness that reads `AGENTS.md` + `mcp.json`. [Manual install](#manual-install) covers the same pieces by hand.

## Automatic install in Claude Code

```sh
rover meta use claude
```

It validates first — aborting without writing anything if the `claude` binary isn't on `PATH` or a target file is malformed JSON — then:

1. **Registers the MCP server** by running `claude mcp add rover -s <scope> -- rover mcp` (skipped if `rover` is already registered).
2. **Installs two hooks** in the scope's settings file: a `SessionStart` hook and a `PreToolUse` hook matched to the built-in `WebFetch` tool. Both run `rover meta hook claude`, which emits the steering text at runtime. The `PreToolUse` hook is a non-blocking reminder — it never denies the `WebFetch` call.
3. **Writes a rules block** into `CLAUDE.md` (project and user scope) between `<!-- rover:begin … -->` markers.

`-s, --scope` mirrors the Claude CLI and decides where each piece lands:

| Scope | MCP registration | Hooks file | Rules block |
| --- | --- | --- | --- |
| `local` *(default)* | `claude mcp add -s local` | `.claude/settings.local.json` | — |
| `project` | `claude mcp add -s project` | `.claude/settings.json` | `./CLAUDE.md` |
| `user` | `claude mcp add -s user` | `~/.claude/settings.json` | `~/.claude/CLAUDE.md` |

At `local` scope there is no committed `CLAUDE.md` to write to, so the steering rides entirely on the `SessionStart` hook in `settings.local.json`. The command is idempotent: re-running updates the managed block and skips registration or hooks that already exist. Restart the session to load the server.

## Automatic install for other harnesses

```sh
rover meta use general
```

For a harness that isn't Claude Code, `general` writes two files at the project root and installs no hooks (there is no portable hook standard):

1. **`mcp.json`** — the conventional `{"mcpServers": {…}}` config, with a `rover` entry added. Any servers already present are preserved.
2. **`AGENTS.md`** — a rules block (between `<!-- rover:begin … -->` markers) telling the agent to prefer Rover for reading pages. Surrounding content is preserved.

`general` is project-root only; `--scope` is accepted but always writes to the project root. If your harness doesn't read `mcp.json` automatically, register the `rover` server from it yourself ([MCP server](#mcp-server) below). Both files are updated in place on re-run.

## Manual install

`rover meta use` is a convenience over three independent pieces. Wire any of them by hand for full control, or for a harness Rover doesn't special-case.

### MCP server

Most agent CLIs share the registration form `<cli> mcp add rover -- rover mcp`:

```sh
claude mcp add rover -- rover mcp      # Claude Code
codex mcp add rover -- rover mcp       # Codex CLI
copilot mcp add rover -- rover mcp     # GitHub Copilot CLI
devin mcp add rover -- rover mcp       # Devin CLI
```

Any other MCP client just needs to be pointed at `rover mcp` over stdio. Add this to its server config — for example a project-root `mcp.json`:

```json
{
  "mcpServers": {
    "rover": {
      "command": "rover",
      "args": ["mcp"]
    }
  }
}
```

Restart the session to pick up the server. The agent then has five tools — `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens` — documented at [MCP tools](/docs/mcp-tools).

### Rules file

A note in the agent's rules file (`CLAUDE.md`, `AGENTS.md`, …) keeps it reaching for Rover instead of a built-in fetch. Paste this block; `rover meta use` writes the same content between markers so it can update it later:

```markdown
## Web fetching: prefer Rover

Rover is wired in as an MCP server. When you need to **read a web page**, prefer Rover over the built-in `WebFetch`:

- `mcp__rover__fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)
- `mcp__rover__batch_fetch` — many URLs concurrently
- `mcp__rover__summarize`, `mcp__rover__get_metadata`, `mcp__rover__count_tokens`

`WebFetch` returns a lossy, per-prompt answer; Rover returns a reusable, guarded document. Keep using `WebSearch` to *find* URLs — then fetch them with Rover, not `WebFetch`. Use `WebFetch` only when Rover is unavailable.
```

`mcp__rover__*` is how Claude Code names the tools. Other harnesses prefix them differently (`rover.fetch`, or bare `fetch`) — use whatever names your harness shows, or the generic block `rover meta use general` writes to `AGENTS.md`:

```markdown
## Web fetching: prefer Rover

A `rover` MCP server is configured in `mcp.json`. When you need to **read a web page**, prefer its tools over any built-in web-fetch tool:

- `fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)
- `batch_fetch` — many URLs concurrently
- `summarize`, `get_metadata`, `count_tokens`

Tool names may be prefixed by your harness (e.g. `rover.fetch` or `mcp__rover__fetch`). A built-in fetch returns a lossy per-prompt answer; Rover returns a reusable, guarded document. If your harness doesn't auto-load `mcp.json`, register the `rover` server from it manually.
```

### Hooks (Claude Code)

Hooks reinforce the rules file at runtime: one fires at session start, the other every time the agent reaches for `WebFetch`. Claude Code hooks live in a settings file — `.claude/settings.json` (project), `.claude/settings.local.json` (private project copy), or `~/.claude/settings.json` (all projects). Add both under `hooks`:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "rover meta hook claude" }] }
    ],
    "PreToolUse": [
      {
        "matcher": "WebFetch",
        "hooks": [{ "type": "command", "command": "rover meta hook claude" }]
      }
    ]
  }
}
```

`rover meta hook claude` is the handler. It reads the payload Claude Code sends on stdin, branches on `hook_event_name`, and prints the response JSON — so one command serves both events and the steering text stays versioned with the binary. The `PreToolUse` entry matches only `WebFetch`; the handler returns a reminder with no `permissionDecision`, so the call proceeds normally.

To wire the steering as **static content** instead — no `rover` call at hook time, or for a harness with a different hook system — emit the JSON yourself. `SessionStart`:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "Rover is wired in as an MCP server and is the preferred way to read web pages. When you need the contents of a URL, use Rover instead of the built-in WebFetch: `mcp__rover__fetch` (one URL → clean, token-budgeted, prompt-injection-guarded Markdown, cached), `mcp__rover__batch_fetch` (many URLs), plus `mcp__rover__summarize`, `mcp__rover__get_metadata`, and `mcp__rover__count_tokens`. WebFetch returns a lossy per-prompt answer; Rover returns a reusable, guarded document. Keep using WebSearch to discover URLs, then fetch them with Rover rather than WebFetch. Use WebFetch only when Rover is unavailable."
  }
}
```

`PreToolUse` (fired before a `WebFetch` call):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": "Rover is available: `mcp__rover__fetch` returns cleaner, token-budgeted, prompt-injection-guarded Markdown than WebFetch and caches it (`mcp__rover__batch_fetch` for many URLs). Consider using Rover instead. Proceeding with WebFetch."
  }
}
```

Leaving out `permissionDecision` is deliberate: the reminder must neither auto-allow nor block the call. Full flag reference: [`rover meta`](/docs/cli#rover-meta).

## From the shell

```sh
rover fetch https://example.com/article            # clean Markdown → stdout
rover fetch --max-tokens 4000 https://example.com  # summarise to fit a budget
rover cache stats                                  # entry count, size, expired
rover doctor                                       # check the install
```

`rover --help` lists every subcommand, and each subcommand has its own `--help`.

## Next

- [Anatomy of a Rover document](/docs/output) covers what a fetch returns, field by field.
- [Managing token budgets](/docs/token-budgets) covers counting and capping token cost.
- [Trust & prompt injection](/docs/trust) explains why the body comes back fenced as untrusted.

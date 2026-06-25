---
id: quickstart
title: Quickstart
---

# Quickstart

Wire Rover into your agent, then fetch a page. This assumes `rover` is installed; if it isn't, start with [Installation](/docs/install).

`rover meta use` does the wiring for you: MCP registration, steering hooks, and a rules-file block, in one command. Use it for Claude Code or any harness that reads `AGENTS.md` and `mcp.json`. To set the same pieces up by hand, see [Manual install](#manual-install).

## Automatic install in Claude Code

```sh
rover meta use claude
```

This validates before it touches anything (it stops without writing if the `claude` binary is missing or a target file is malformed JSON), then registers the MCP server, installs two hooks, and writes a rules block. The `-s, --scope` flag (default `local`) decides where each piece lands; the paths below are for the default `local` scope.

### Registers the MCP server

Runs `claude mcp add rover -s local -- rover mcp`, and skips it if `rover` is already registered. At `local` scope the Claude CLI records the server in `~/.claude.json`, under the current project's entry:

```json
{
  "projects": {
    "/path/to/your/project": {
      "mcpServers": {
        "rover": {
          "type": "stdio",
          "command": "rover",
          "args": ["mcp"]
        }
      }
    }
  }
}
```

`-s project` writes a top-level `mcpServers.rover` to a project-root `.mcp.json` instead, and `-s user` writes one to the top level of `~/.claude.json`. The server object is the same in each.

### Installs two hooks

Adds a `SessionStart` hook and a `PreToolUse` hook (matched to the built-in `WebFetch` tool) to the scope's settings file, which at `local` scope is `.claude/settings.local.json`:

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

Both entries run `rover meta hook claude`, which prints the steering text for whichever event fired. The `PreToolUse` hook only reminds; its output carries no `permissionDecision`, so the `WebFetch` call still runs. It prints (shown formatted here; the hook emits a single line):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": "Rover is available: `mcp__rover__fetch` returns cleaner, token-budgeted, prompt-injection-guarded Markdown than WebFetch and caches it (`mcp__rover__batch_fetch` for many URLs). Consider using Rover instead. Proceeding with WebFetch."
  }
}
```

### Writes a rules block

At `project` and `user` scope, writes a steering block to `CLAUDE.md` (`./CLAUDE.md` and `~/.claude/CLAUDE.md` respectively), wrapped in markers so re-runs update it in place:

```markdown
<!-- rover:begin — managed by `rover meta use`; edit outside these markers -->
## Web fetching: prefer Rover

Rover is wired in as an MCP server. When you need to **read a web page**, prefer Rover over the built-in `WebFetch`:

- `mcp__rover__fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)
- `mcp__rover__batch_fetch` — many URLs concurrently
- `mcp__rover__summarize`, `mcp__rover__get_metadata`, `mcp__rover__count_tokens`

`WebFetch` returns a lossy, per-prompt answer; Rover returns a reusable, guarded document. Keep using `WebSearch` to *find* URLs — then fetch them with Rover, not `WebFetch`. Use `WebFetch` only when Rover is unavailable.
<!-- rover:end -->
```

The default `local` scope skips this step. There is no committed `CLAUDE.md` to write a private choice into, so at `local` scope the steering rides on the `SessionStart` hook in `settings.local.json` instead.

Re-running `rover meta use claude` is safe at any scope: the managed block is updated in place, and registration or hooks that already exist are left alone. Restart the session to load the server.

## Automatic install for other harnesses

```sh
rover meta use general
```

For a harness that isn't Claude Code, `general` writes two files at the project root and installs no hooks (there is no portable hook standard):

- `mcp.json` gets a `rover` server added to the conventional `{"mcpServers": { ... }}` config; any servers already there are preserved.
- `AGENTS.md` gets a rules block, wrapped in `<!-- rover:begin ... -->` markers, telling the agent to prefer Rover for reading pages; surrounding content is preserved.

`general` is project-root only; `--scope` is accepted but always writes to the project root. If your harness doesn't read `mcp.json` automatically, register the `rover` server from it yourself ([MCP server](#mcp-server) below). Both files are updated in place on re-run.

## Manual install

`rover meta use` is a convenience over three independent pieces. Set up any of them by hand for full control, or for a harness Rover doesn't special-case.

### MCP server

Most agent CLIs share the registration form `<cli> mcp add rover -- rover mcp`:

```sh
claude mcp add rover -- rover mcp      # Claude Code
codex mcp add rover -- rover mcp       # Codex CLI
copilot mcp add rover -- rover mcp     # GitHub Copilot CLI
devin mcp add rover -- rover mcp       # Devin CLI
```

Any other MCP client just needs to be pointed at `rover mcp` over stdio. Add this to its server config, for example a project-root `mcp.json`:

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

Restart the session to pick up the server. The agent then has five tools (`fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens`), documented at [MCP tools](/docs/mcp-tools).

### Rules file

A note in the agent's rules file (`CLAUDE.md`, `AGENTS.md`, ...) keeps it reaching for Rover instead of a built-in fetch. `rover meta use` writes this block between markers so it can update it later; paste it yourself for a harness it doesn't cover. The Claude Code variant, with the `mcp__rover__*` tool names, is shown under [Writes a rules block](#writes-a-rules-block) above. The generic version, for any harness:

```markdown
## Web fetching: prefer Rover

A `rover` MCP server is configured in `mcp.json`. When you need to **read a web page**, prefer its tools over any built-in web-fetch tool:

- `fetch` — one URL → clean, token-budgeted, prompt-injection-guarded Markdown (cached)
- `batch_fetch` — many URLs concurrently
- `summarize`, `get_metadata`, `count_tokens`

Tool names may be prefixed by your harness (e.g. `rover.fetch` or `mcp__rover__fetch`). A built-in fetch returns a lossy per-prompt answer; Rover returns a reusable, guarded document. If your harness doesn't auto-load `mcp.json`, register the `rover` server from it manually.
```

### Hooks (Claude Code)

Hooks reinforce the rules file at runtime: one fires at session start, the other before each `WebFetch`. They live in a Claude Code settings file: `.claude/settings.json` (project), `.claude/settings.local.json` (private project copy), or `~/.claude/settings.json` (all projects). To add them by hand, use the two entries shown under [Installs two hooks](#installs-two-hooks) above, both pointing at `rover meta hook claude`.

To wire the steering as static content instead (no `rover` call at hook time, or for a harness with a different hook system), emit the response JSON yourself. The `PreToolUse` payload is the reminder shown above; the `SessionStart` payload:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "Rover is wired in as an MCP server and is the preferred way to read web pages. When you need the contents of a URL, use Rover instead of the built-in WebFetch: `mcp__rover__fetch` (one URL → clean, token-budgeted, prompt-injection-guarded Markdown, cached), `mcp__rover__batch_fetch` (many URLs), plus `mcp__rover__summarize`, `mcp__rover__get_metadata`, and `mcp__rover__count_tokens`. WebFetch returns a lossy per-prompt answer; Rover returns a reusable, guarded document. Keep using WebSearch to discover URLs, then fetch them with Rover rather than WebFetch. Use WebFetch only when Rover is unavailable."
  }
}
```

Leave out `permissionDecision` so the reminder neither auto-allows nor blocks the call. Full flag reference: [`rover meta`](/docs/cli#rover-meta).

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

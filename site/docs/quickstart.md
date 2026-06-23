---
id: quickstart
title: Quickstart
---

# Quickstart

Wire Rover into your agent, then fetch a page. Assumes `rover` is installed — see [Installation](/docs/install).

## Add the MCP server

Most agent CLIs register an MCP server the same way — `<cli> mcp add rover -- rover mcp`:

```sh
claude mcp add rover -- rover mcp      # Claude Code
codex mcp add rover -- rover mcp       # Codex CLI
copilot mcp add rover -- rover mcp     # GitHub Copilot CLI
devin mcp add rover -- rover mcp       # Devin CLI
```

Devin saves to the project scope by default; add `-s user` for a user-wide entry. For any other MCP client, point it at `rover mcp` over stdio:

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

Restart the session. Your agent now has five tools — `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens` — documented in full at [MCP tools](/docs/mcp-tools).

## From the shell

```sh
rover fetch https://example.com/article            # clean Markdown → stdout
rover fetch --max-tokens 4000 https://example.com  # summarise to fit a budget
rover cache stats                                  # entry count, size, expired
rover doctor                                       # check the install
```

`rover --help` lists every subcommand; each has its own `--help`.

## Next

- [Anatomy of a Rover document](/docs/output) — what a fetch returns, field by field.
- [Managing token budgets](/docs/token-budgets) — counting and capping token cost.
- [Trust & prompt injection](/docs/trust) — why the body comes back fenced as untrusted.

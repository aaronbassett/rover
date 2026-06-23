---
id: quickstart
title: Quickstart
---

# Quickstart

**Wire Rover into your agent, then fetch a page.** Assumes `rover` is installed — see [Installation](/docs/install).

## Claude Code

```sh
claude mcp add rover -- rover mcp
```

## Other MCP clients

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

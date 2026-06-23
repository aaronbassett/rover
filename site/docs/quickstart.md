---
id: quickstart
title: Quickstart
---

# Quickstart

Register Rover with your agent, then fetch a page. This assumes `rover` is installed; if it isn't, start with [Installation](/docs/install).

## Add the MCP server

Most agent CLIs use the same registration form, `<cli> mcp add rover -- rover mcp`:

```sh
claude mcp add rover -- rover mcp      # Claude Code
codex mcp add rover -- rover mcp       # Codex CLI
copilot mcp add rover -- rover mcp     # GitHub Copilot CLI
devin mcp add rover -- rover mcp       # Devin CLI
```

Devin writes to the project scope by default. Pass `-s user` for a user-wide entry instead. Any other MCP client just needs to be pointed at `rover mcp` over stdio:

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

Restart the session to pick up the new server. The agent then has five tools: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, and `count_tokens`. Each is documented at [MCP tools](/docs/mcp-tools).

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

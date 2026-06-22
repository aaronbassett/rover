---
id: intro
title: Getting started
slug: /intro
---

# Get Rover

Rover is an MCP server that turns the web into clean, token-efficient Markdown your agent can trust.

## Wire it into Claude Code

```sh
claude mcp add rover -- rover mcp
```

## Or use the CLI

```sh
rover fetch https://example.com/article
rover fetch --max-tokens 4000 https://example.com
```

See the [CLI reference](/docs/cli) and [MCP tools](/docs/mcp-tools) for the full surface.

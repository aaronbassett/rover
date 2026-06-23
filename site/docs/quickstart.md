---
id: quickstart
title: Quickstart
---

# Quickstart

**The fastest path from installed binary to a page in your agent's context.** Rover runs two ways from the same binary: a long-lived MCP server your agent talks to, and a one-shot CLI you can run from a shell. Wire up one of them, ask for a URL, and read what comes back. A couple of minutes, start to finish.

## Before you start

This page assumes `rover` is on your `PATH`. If it isn't, start with [Installation](/docs/install) and come back.

## Wire it into Claude Code

One command registers Rover as an MCP server in Claude Code:

```sh
claude mcp add rover -- rover mcp
```

That's the whole setup. `rover mcp` is the long-running server; the `claude mcp add` line tells Claude Code how to launch it. Restart the session and the tools are live.

## Other MCP clients

Any client that takes a JSON config wants the same shape — a `command` plus the `mcp` arg:

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

Point the client at `rover mcp`, however that client spells it. The server speaks standard MCP over stdio; there's nothing Rover-specific in the wiring.

## Your first fetch

Once Rover is wired in, the agent has five tools. You don't call them by hand — you ask for a URL and the agent picks the right one:

| Tool | What it does |
| --- | --- |
| `fetch` | Single URL to cleaned Markdown. Caching, headless rendering, image modes, token budgeting, inline summarisation. |
| `batch_fetch` | Fetch N URLs concurrently with per-domain rate limiting. Returns a `task_id`; stream progress with `rover batch <id> --monitor`. |
| `summarize` | Compact a cached or fresh page via extractive (offline) or cloud backends. Steer it with `focus`, `preserve`, and `target_tokens`. |
| `get_metadata` | Pull Schema.org, Open Graph, and Twitter Card metadata without the full body. |
| `count_tokens` | Estimate a URL's token cost across `cl100k` / `o200k` / `claude` / `llama3` / `qwen3` before you pay it. |

To see it work, just ask the agent to fetch something — "fetch https://example.com/article and summarise it." It calls `fetch`, and you get back a clean, trust-wrapped Markdown document. That's the whole loop.

## …or from the shell

Every capability is also a one-shot CLI command — handy for scripts, CI, and trying things out before you wire anything in:

```sh
rover fetch https://example.com/article            # clean Markdown → stdout
rover fetch --max-tokens 4000 https://example.com  # summarise to fit a token budget
rover cache stats                                  # entry count, size, expired
rover doctor                                       # sanity-check the install
```

`rover --help` prints the full subcommand surface, and every subcommand has its own `--help`. If something isn't behaving, `rover doctor` is the first thing to run.

## What you just got back

The body of every fetch is fenced as untrusted data, not instructions. Rover wraps it behind a trusted preamble and a per-response nonce delimiter that marks the page content as third-party input the model should read, never obey — see [Anatomy of a Rover document](/docs/output) for the document shape and [Trust & prompt injection](/docs/trust) for why the fence holds.

## Where to next

You have a working fetch. Pick the thread that matches what you're building:

- [Managing token budgets](/docs/token-budgets) — counting cost and fitting pages to a budget.
- [Summarising pages](/docs/summarizing) — extractive and cloud backends, steered with `focus` and `target_tokens`.
- [Caching & freshness](/docs/caching) — TTLs, revalidation, and not fetching the same URL twice.
- [MCP tools](/docs/mcp-tools) — full arguments and wire contracts for all five tools.
- [CLI](/docs/cli) — the complete one-shot command surface.

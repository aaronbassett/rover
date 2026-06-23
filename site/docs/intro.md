---
id: intro
title: Introduction
slug: /intro
---

# Introduction

**Rover is an MCP server that turns the web into clean, token-efficient Markdown your LLM agent can actually trust.** Point it at a URL and it fetches the page, strips the ads, nav, and chrome, extracts the real content with the battle-tested [`readabilityrs`](https://crates.io/crates/readabilityrs) crate, normalises it to Markdown, counts the tokens, and optionally summarises to a budget. The same binary runs as a long-lived MCP server for Claude Code and other agent harnesses, and as a one-shot CLI. It is built for **single-user-local** deployment — one server beside your IDE or agent, not a multi-tenant gateway.

## What your agent gets back

Rover hands your agent a clean Markdown **document**, not a model's lossy answer about the page. The body carries YAML frontmatter — canonical URL, content hash, token estimate, language, and an extraction-quality score — and the whole thing sits behind a trusted preamble and a per-response nonce delimiter that marks the body as untrusted data. Fetch the same page twice and you re-read the cached document; you don't re-run a model. Reach for a tool that returns raw HTML bytes and you've handed your agent a page full of nav bars and a token bill to match.

```yaml
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
content_hash: "sha256:b3e9…"
estimated_tokens: 14823
tokenizer: "o200k"
language: "en"
extraction_quality: 0.98
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language…
```

Full schema and the trust wrapper: [Anatomy of a Rover document](/docs/output).

## The four walls, and how Rover gets past them

Agents that browse the live web hit the same four walls every time. Rover has a fix for each.

| Wall | What it costs your agent | Rover's fix |
| --- | --- | --- |
| Boilerplate, ads, and chrome drown the content | Token budget vanishes into nav menus and cookie banners | Readability extraction to clean Markdown |
| JavaScript-rendered pages return an empty root `<div>` | Non-browsers reason over a shell, not the content | Optional headless rendering (the `headless` Cargo feature) |
| Repeated fetches waste tokens, time, and money | Wasted spend, and politeness rules ignored | HTTP-aware caching, per-domain rate limiting, `robots.txt` |
| Fetched web content is untrusted | "Ignore your instructions and…" lands straight in context | A layered prompt-injection guard, always on |

That last one is the difference. Most fetch tools hand the page to the model raw and hope for the best — Rover treats every page as input to read, never instructions to act on. The boundary holds by construction. See [Trust & prompt injection](/docs/trust) for how the nonce fence works.

On top of extraction, Rover layers HTTP-aware caching (TTL, ETag, Last-Modified, stale-while-revalidate), per-domain rate limiting and `robots.txt`, charset detection, configurable SSRF protection, the prompt-injection guard, optional headless rendering, extractive and cloud-LLM summarisation, inline image captioning, and a long-running task model with NDJSON-streamed progress.

## Five tools on day one

Wire Rover into your agent and it gets these five MCP tools.

| Tool | What it does |
| --- | --- |
| `fetch` | Single URL to cleaned Markdown. Handles caching, headless rendering, image modes, token budgeting, and inline summarisation. |
| `batch_fetch` | Fetch many URLs concurrently with per-domain rate limiting. Returns a task ID and streams progress as NDJSON. |
| `summarize` | Compact a cached or fresh page via the extractive or cloud backend. Steerable with `focus`, `preserve`, and `target_tokens`. |
| `get_metadata` | Pull Schema.org, Open Graph, and Twitter Card metadata without fetching the full body. |
| `count_tokens` | Estimate a URL's token cost across `cl100k`, `o200k`, `claude`, `llama3`, and `qwen3` tokenisers without paying it. |

Full schemas, arguments, and wire contracts: [MCP tools](/docs/mcp-tools).

:::note
Rover isn't a web crawler. To recursively mirror or crawl a whole site, reach for `wget` or `httrack`. Rover preps *individual* pages for an agent to reason over, not bulk downloads.
:::

## Start here

Rover is pre-1.0 (`0.1.0`); the build-from-source path works today. Two pages get you running:

- **[Installation](/docs/install)** — build from source, or grab a packaged binary once channels come online.
- **[Quickstart](/docs/quickstart)** — wire Rover into Claude Code in one command and fetch your first page.

From there, [Anatomy of a Rover document](/docs/output) explains exactly what your agent receives, and [Trust & prompt injection](/docs/trust) covers why fetched content can't talk your agent into anything.

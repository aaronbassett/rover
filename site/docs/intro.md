---
id: intro
title: Introduction
slug: /intro
---

# Introduction

**Rover fetches a URL and returns clean, token-counted Markdown your agent can treat as untrusted data.** It strips ads, nav, and chrome with [`readabilityrs`](https://crates.io/crates/readabilityrs), normalises to Markdown, counts the tokens, optionally summarises to a budget, and wraps the body so the model reads the page as data, not instructions. Rover is an MCP server and a CLI.

## What you get back

A Markdown document with YAML frontmatter — content hash, token count, language, extraction-quality score — behind a per-response nonce fence that marks the body as untrusted. The hash and token count make it reusable: cache it, re-read it on the next prompt, diff a later fetch against it.

```yaml
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
content_hash: "sha256:b3e9…"
estimated_tokens: 14823
tokenizer: "o200k"
extraction_quality: 0.98
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language…
```

Field reference: [Anatomy of a Rover document](/docs/output).

## What it handles

| Problem | Rover's answer |
| --- | --- |
| Ads, nav, and chrome inflate token cost | Readability extraction to clean Markdown |
| JavaScript-rendered pages return an empty shell | Optional headless rendering (the `headless` feature) |
| Refetching the same URL wastes tokens and money | HTTP-aware caching, per-domain rate limiting, `robots.txt` |
| A page can smuggle "ignore your instructions" into context | A layered [prompt-injection guard](/docs/trust) |

It also does extractive and cloud summarisation, inline image captioning, and batch fetches with streamed progress.

## Tools

| Tool | Returns |
| --- | --- |
| `fetch` | A URL as cleaned Markdown. |
| `batch_fetch` | Many URLs concurrently; streams progress. |
| `summarize` | A page compacted via an extractive or cloud backend. |
| `get_metadata` | Schema.org / Open Graph / Twitter Card metadata, no body. |
| `count_tokens` | A URL's token cost across five tokenisers. |

Full schemas: [MCP tools](/docs/mcp-tools).

## Start here

- [Installation](/docs/install) — Homebrew, prebuilt binary, or build from source.
- [Quickstart](/docs/quickstart) — wire Rover in and make a first fetch.

---
id: intro
title: Introduction
slug: /intro
---

# Introduction

Rover fetches a URL and hands back clean, token-counted Markdown your agent can treat as untrusted data. It strips ads, nav, and chrome with [`readabilityrs`](https://crates.io/crates/readabilityrs), normalises the page to Markdown, counts the tokens, optionally summarises to a budget, and wraps the body so the model reads the page as data, not instructions. Rover runs as an MCP server and a CLI.

## What you get back

Every fetch returns a Markdown document with YAML frontmatter: content hash, token count, language, and an extraction-quality score. The body sits behind a per-response nonce fence that marks it as untrusted. The hash and token count are what make the result reusable. Cache it, re-read it on the next prompt, or diff a later fetch against it.

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

Every field is documented in [Anatomy of a Rover document](/docs/output).

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

Full schemas live in [MCP tools](/docs/mcp-tools).

## Start here

- [Installation](/docs/install) covers Homebrew, the prebuilt binary, and building from source.
- [Quickstart](/docs/quickstart) wires Rover in and runs a first fetch.

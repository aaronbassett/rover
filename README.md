# Rover

An MCP (Model Context Protocol) server that fetches web pages and turns them
into clean, token-efficient Markdown for LLM agents.

> **Status:** early development. Milestone M1 (single-URL fetch path) is
> currently being implemented. See `docs/superpowers/prd/2026-05-07-rover-prd.md`
> for the product spec and `docs/superpowers/specs/2026-05-07-rover-design.md`
> for architectural decisions.

## Build

```sh
cargo build --release
```

The release binary lands at `target/release/rover`.

## Try it

```sh
cargo run --release -- fetch https://en.wikipedia.org/wiki/Rust_(programming_language)
```

Output is YAML-frontmattered Markdown printed to stdout:

```yaml
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
fetched_at: "2026-05-07T12:34:56Z"
content_hash: "sha256:..."
estimated_tokens: 14823
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language ...
```

## Subcommands

`rover fetch <url>` is implemented in M1. The full subcommand surface
(`rover mcp`, `rover batch`, `rover task`, `rover cache`, `rover doctor`,
`rover config`) ships across milestones M2–M8 — see the PRD.

## License

MIT or Apache-2.0, at your option.

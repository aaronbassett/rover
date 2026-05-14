# Rover

An MCP (Model Context Protocol) server that fetches web pages and turns them
into clean, token-efficient Markdown for LLM agents.

> **Status:** early development. Milestones M1 (single-URL fetch path),
> M2 (caching & storage), M3 (MCP server mode), and M4 (metadata, tables,
> images, links) are complete. M5 (rate limiting & robots) is next. See
> `docs/superpowers/prd/2026-05-07-rover-prd.md` for the product spec and
> `docs/superpowers/specs/2026-05-07-rover-design.md` for architectural
> decisions.

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

## Cache

`rover` keeps a local SQLite cache at `$XDG_DATA_HOME/rover/rover.db` (or
`~/.local/share/rover/rover.db` by default; override with `ROVER_DATA_DIR`).

```sh
rover cache list                 # paginated URL listing
rover cache get <url>            # print cached Markdown for a URL
rover cache purge 'https://x/*'  # delete cache entries by glob
rover cache stats                # entry count, total size, expired count
```

`rover fetch <url>` writes through the cache. `rover fetch --force-refresh <url>`
bypasses it.

## Subcommands

`rover fetch <url>`, `rover cache ...`, and `rover mcp` are implemented in
M1–M3. The remaining subcommand surface (`rover batch`, `rover task`,
`rover doctor`, `rover config`) ships across milestones M6–M8 — see the PRD.

## License

MIT or Apache-2.0, at your option.

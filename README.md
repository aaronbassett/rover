# Rover

An MCP (Model Context Protocol) server that fetches web pages and turns them
into clean, token-efficient Markdown for LLM agents.

> **Status:** early development. Milestones M1 (single-URL fetch path),
> M2 (caching & storage), M3 (MCP server mode), M4 (metadata, tables,
> images, links), M5 (rate limiting & robots), M6 (long-running tasks
> & batching — `batch_fetch`, `rover batch <id>`, `rover task <id>`),
> and M7 (summarization — `summarize` MCP tool, extractive + cloud
> backends, summary cache) are complete. M8 (SSRF level matrix,
> diagnostics, `rover doctor`) is next. See
> `docs/superpowers/prd/2026-05-07-rover-prd.md` for the product spec,
> `docs/superpowers/specs/2026-05-07-rover-design.md` for architectural
> decisions, and `docs/security.md` for known v1 security boundaries.

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

`rover fetch <url>`, `rover cache ...`, `rover mcp`, `rover batch <id>`,
and `rover task <id>` are implemented in M1–M6. The remaining subcommand
surface (`rover doctor`, `rover config`) ships in M8 — see the PRD.

## Background tasks

`batch_fetch` (MCP tool) and `rover batch <id>` / `rover task <id>` (CLI)
schedule long-running work and stream NDJSON event logs:

```sh
rover task <id>                # snapshot: progress, ETA, last event
rover batch <id> --monitor     # stream task_started/item_done/.../task_completed
rover task <id> --cancel       # request cooperative cancellation
rover batch <id> --format=ndjson      # snapshot as a single JSON line
rover task <id> --monitor --from-event <id>   # resume an interrupted stream
```

## Summarization (M7)

The `summarize` MCP tool compacts a cached (or freshly fetched) page using
either the offline extractive backend (TextRank) or a cloud backend via
`genai`. Backends are declared per-config and addressable by name:

```toml
[summarization]
default_backend = "default"
fallback_to_extractive = true

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
```

Then from an MCP client:

```jsonc
{
  "name": "summarize",
  "args": {
    "url": "https://example.com/article",
    "mode": "abstractive",
    "backend": "fast",
    "target_tokens": 500
  }
}
```

When the requested backend fails (auth, rate-limit, network), Rover
transparently falls back to the extractive backend and tags the response
with `summarizer_fallback: { from, reason }`. Set
`[summarization] fallback_to_extractive = false` for strict errors.

The MCP `fetch` tool also accepts `max_tokens` (auto-summarize when the
extracted markdown exceeds the budget) and an inline `summarize` arg
shaped like the `summarize` tool's. The CLI `rover fetch` exposes
matching `--max-tokens` and `--summarize <JSON>` flags for forward
compatibility, but the canonical summarization surface in v1 is MCP.

## License

MIT or Apache-2.0, at your option.

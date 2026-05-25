# Rover

An MCP (Model Context Protocol) server that fetches web pages and turns them
into clean, token-efficient Markdown for LLM agents.

> **Status:** early development. See
> `docs/superpowers/prd/2026-05-07-rover-prd.md` for the product spec,
> `docs/superpowers/specs/2026-05-07-rover-design.md` for architectural
> decisions, and `docs/security.md` for known v1 security boundaries.

## Milestones

| Milestone | Theme | Status | Date |
|-----------|-------|--------|------|
| M1 | Single-URL fetch path | ✅ | 2026-05-08 |
| M2 | Caching & storage | ✅ | 2026-05-11 |
| M3 | MCP server mode | ✅ | 2026-05-14 |
| M4 | Metadata, tables, images, links | ✅ | 2026-05-17 |
| M5 | Rate limiting & robots | ✅ | 2026-05-19 |
| M6 | Long-running tasks & batching | ✅ | 2026-05-21 |
| M7 | Summarization | ✅ | 2026-05-22 |
| M8 | SSRF Levels, Diagnostics, Polish | ✅ | 2026-05-23 |

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

### Diagnostics & Configuration (M8)

`rover doctor` runs a battery of health checks (SQLite open, WAL mode, schema version, network reachability, output dir, configured cloud backends, extractive synthesis):

```bash
rover doctor
rover doctor --format=ndjson    # one JSON object per check, for scripting
```

`rover config show` prints the merged effective config with per-leaf provenance comments:

```bash
rover config show
```

`rover config set <dotted.key> <value>` mutates the config file in place (preserves comments via `toml_edit`, then round-trip validates):

```bash
rover config set ssrf.level loopback
```

HAR debug recording — set `[debug] har_path` in `rover.toml`:

```toml
[debug]
har_path = "./rover-debug.har"
har_body_cap = "64KiB"
```

The resulting file imports into Chrome DevTools' Network panel (Import HAR).

SSRF protection ships with a five-level matrix (`strict | loopback | project | lan | none`) — see `docs/security.md` for the always-floor rules and the v2 DNS-rebinding limitation.

Five reference docs land alongside this milestone: `docs/configuration.md`, `docs/cli.md`, `docs/mcp-tools.md`, `docs/security.md`, `docs/backends.md`.

## License

MIT or Apache-2.0, at your option.

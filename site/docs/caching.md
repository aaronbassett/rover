---
id: caching
title: Caching & freshness
---

# Caching & freshness

**Rover caches every fetch and bounds how stale a cached copy can get.** A single SQLite database in WAL mode backs the cache, keyed by a hash of the canonical URL. Cache decisions follow upstream HTTP semantics: an origin that says "fresh for an hour" gets an hour, and one that says "don't store this" is honoured. The default TTL is short on purpose.

## How the cache decides

Rover reads the freshness rules off the response. `Cache-Control` (`max-age`, `s-maxage`, `no-store`, `no-cache`, `must-revalidate`), `Expires`, `ETag`, and `Last-Modified` all feed the decision. When an entry expires but carries a validator, the next fetch is a conditional GET reusing the stored `ETag` / `Last-Modified` — and a `304 Not Modified` refreshes the entry's expiry and counts as a hit. No re-download, no re-extraction, no token cost.

TTL is derived from the upstream headers, then clamped to `[min_ttl, max_ttl]`. When upstream sends no `max-age`, `default_ttl` applies. The defaults:

```toml
[cache]
default_ttl = "15m"   # used when upstream sends no max-age
min_ttl = "5m"        # floor: TTLs shorter than this are raised to it
max_ttl = "7d"        # ceiling: TTLs longer than this are capped to it
```

An origin can ask for a longer life through its own `Cache-Control`, up to `max_ttl`. The clamp is the outer boundary; the origin chooses within it. Tune these in the [Configuration](/docs/configuration) reference.

## Why the default TTL is short

Absent an explicit `max-age`, a cached page that's been poisoned or quietly changed has a small blast radius before the next revalidation. Set the default to an hour and a bad cache write rides along for an hour. Fifteen minutes keeps the window narrow. Origins that want longer caching say so in their headers — and get it, clamped to `max_ttl`. Cache poisoning is covered on the [Security & threat model](/docs/security) page.

`no-store` is honoured: the response isn't cached. Two escape hatches exist for when you control the origin — `override_no_store = true` flips it globally, and `override_no_store_domains` flips it per host:

```toml
[cache]
override_no_store = true
override_no_store_domains = ["docs.example.com"]
```

## Stale-while-revalidate

An entry that expired recently is served immediately while a fresh copy is fetched in the background. The window is `stale_while_revalidate_window` (default 5 minutes). Inside it, the just-expired entry comes back as `cache_status: "stale"`, a background `revalidate` task refreshes the row, and the response carries a `revalidation` block you can monitor. You get an answer now; the next caller gets the updated one.

Beyond that window, the entry is a miss and refetched synchronously. The stale fast-path only ever serves content that expired in the last few minutes.

The background path needs the MCP server's scheduler. The one-shot CLI (`rover fetch`) has no in-process scheduler, so it can't queue a background `revalidate` — `rover fetch` **always** revalidates synchronously, regardless of the window. Worth knowing if you're comparing `cache_status` between `rover fetch` and a long-running `rover mcp`.

Every fetch reports its `cache_status`, one of three values:

| `cache_status` | Meaning |
| --- | --- |
| `hit` | Served from cache; still fresh (or a `304` refreshed it). |
| `miss` | Not cached, expired past the SWR window, or bypassed — fetched from the origin. |
| `stale` | Expired within the SWR window; served now, refreshed in the background (MCP only). |

## Bypassing the cache

Skip the cache for a single request with `force_refresh` (MCP) or `--force-refresh` (CLI). It hits the origin, ignores whatever's stored, and writes the fresh result back — so the next caller gets a hit:

```sh
rover fetch --force-refresh https://example.com/page
```

It's a per-request override, not a config change — cache rules stay as they were for every other URL.

## Managing the cache

The `rover cache` subcommands inspect and prune the local store. See the [CLI](/docs/cli) reference for full flag details and exit codes.

| Command | What it does |
| --- | --- |
| `rover cache list [--limit N] [--offset N]` | List entries, most recent first. `--limit` defaults to 20, `--offset` to 0. |
| `rover cache get <url>` | Print the cached Markdown body for a URL. |
| `rover cache purge <pattern> [--all]` | Delete entries matching a glob (`*`, `?`). |
| `rover cache stats` | Report size, entry count, and expired count. |

`rover cache purge 'https://example.com/*'` clears one origin. The bare pattern `*` matches everything — Rover refuses it unless you also pass `--all`. That interlock is the difference between purging a domain and emptying the whole cache by typo.

## Where it lives

The database is a single file at `$XDG_DATA_HOME/rover/rover.db` — typically `~/.local/share/rover/rover.db`. It holds the cache, task state, and the event log, in WAL mode. Override the data directory with `ROVER_DATA_DIR` — a tmpfs for ephemeral runs, a project directory for isolation, a faster disk:

```sh
ROVER_DATA_DIR=/tmp/rover-cache rover fetch https://example.com
```

## Storing raw HTML

By default Rover caches the extracted Markdown and nothing else. Set `store_raw_html = true` and it also keeps the original HTML, zstd-compressed:

```toml
[cache]
store_raw_html = true
```

This populates the `raw_html` figure in `count_tokens` estimates — what the page would have cost unextracted. The cost is disk: two copies of every page. Leave it off unless you want that comparison. See [Managing token budgets](/docs/token-budgets) for what the estimate buys you, and the [Anatomy of a Rover document](/docs/output) for where `cache_status` and the rest of the envelope live in a response.

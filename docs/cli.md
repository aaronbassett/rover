# Rover CLI

Synopsis:

```text
rover [--config <path>] <subcommand> [args]
```

Global flags:

| Flag | Description |
| --- | --- |
| `--config <path>` | Override the config-file path. When absent, Rover falls back to `ROVER_CONFIG`, then the platform config dir (`~/.config/rover/config.toml` on Linux/macOS). |

Subcommands:

- `fetch <url>` — one-shot fetch, prints Markdown + frontmatter to stdout.
- `mcp` — start the MCP server over stdio.
- `cache <list|get|purge|stats>` — cache operations.
- `task <id>` — inspect/monitor a long-running task.
- `batch <id>` — inspect/monitor a `batch_fetch` task (alias for `task` with a kind check).
- `doctor` — run environment diagnostics.
- `config <show|set>` — inspect or update the config file.

Exit code `0` on success; `1` on any failure (config parse error, fetch error, doctor check failure, etc.).

## `rover fetch`

```text
rover fetch <url> [--force-refresh] [--ignore-robots]
             [--rate-limit-rpm <N>] [--per-host-concurrency <N>]
             [--global-concurrency <N>] [--max-retries <N>]
             [--max-tokens <N>] [--summarize <JSON>]
```

Fetches `<url>` through the cache-aware orchestrator (`fetch_with_cache`), runs the extraction pipeline, and prints a frontmatter-wrapped Markdown document to stdout.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `--force-refresh` | bool | off | Bypass the cache and re-fetch from origin. |
| `--ignore-robots` | bool | off | Skip the robots.txt gate for this fetch. CLI-only escape hatch. |
| `--rate-limit-rpm <N>` | u32 | from `[rate_limit]` | Override `requests_per_minute_per_domain`. |
| `--per-host-concurrency <N>` | u32 | from `[rate_limit]` | Override `per_domain_concurrency`. Clamped to `>= 1`. |
| `--global-concurrency <N>` | u32 | from `[rate_limit]` | Override `global_concurrency`. Clamped to `>= 1`. |
| `--max-retries <N>` | u8 | from `[rate_limit]` | Override `max_retries`. |
| `--max-tokens <N>` | usize | unset | Token budget. **v1 note:** parsed and validated; the canonical auto-summarize path is the MCP `fetch` tool. |
| `--summarize <JSON>` | string | unset | JSON blob with the same shape as the MCP `summarize` args minus `url`. **v1 note:** validated only; use the MCP `summarize` tool for the canonical surface. |

## `rover mcp`

```text
rover mcp [--ignore-robots]
          [--rate-limit-rpm <N>] [--per-host-concurrency <N>]
          [--global-concurrency <N>] [--max-retries <N>]
```

Starts the MCP server over stdio. Long-running. Same `--rate-limit-*` / `--ignore-robots` overrides as `fetch`, but applied for the lifetime of the server.

## `rover cache`

```text
rover cache list  [--limit <N>] [--offset <N>]
rover cache get   <url>
rover cache purge <pattern> [--all]
rover cache stats
```

| Subcommand | Description |
| --- | --- |
| `list` | List cached URLs, most recent first. `--limit` defaults to `20`, `--offset` to `0`. |
| `get <url>` | Print the cached Markdown body for `<url>`. |
| `purge <pattern>` | Delete cache entries whose URL matches the glob (`*`, `?`). The pattern `*` requires `--all` as a safety interlock. |
| `stats` | Print cache size, entry count, expired-entry count. |

## `rover task`

```text
rover task <id> [--monitor] [--cancel]
                [--format human|ndjson] [--from-event <N>]
```

Pure reader except for `--cancel`. Reads `tasks` + `task_events` from the cache database. No HTTP, no scheduler responsibilities.

| Flag | Default | Description |
| --- | --- | --- |
| `--monitor` | off | Stream task events as they're appended. Combine with `--from-event` to resume. |
| `--cancel` | off | Set the task's `cancellation_requested` flag (a single UPDATE). |
| `--format <fmt>` | `human` | `human` prints one line per event; `ndjson` emits one JSON object per line. |
| `--from-event <N>` | unset | Start streaming after this event id (use with `--monitor`). |

## `rover batch`

```text
rover batch <id> [--monitor] [--cancel]
                 [--format human|ndjson] [--from-event <N>]
```

Same flags and semantics as `rover task`, but the loaded task's `kind` must be `batch_fetch`. Returns an error if the id refers to a non-batch task.

## `rover doctor`

```text
rover doctor [--format human|ndjson]
```

Runs the built-in diagnostic battery sequentially:

1. **sqlite_open** — cache database opens cleanly.
2. **sqlite_wal_mode** — WAL journal mode active.
3. **sqlite_schema_version** — schema version matches the binary.
4. **output_dir_writable** — `[output] dir` (or its default) is writable.
5. **network_reachable** — `HEAD https://example.com` succeeds.
6. **extractive_synthesis** — the extractive backend produces output on a fixed input.
7. **backends_authenticate** — every cloud `[backends.*]` block authenticates.

| Flag | Default | Description |
| --- | --- | --- |
| `--format <fmt>` | `human` | `human`: one line per check with `✓` / `✗` / `-` markers and a summary footer (`all checks ok` / `one or more checks failed`). `ndjson`: one `{check, status, detail?}` JSON object per line. |

Exit code: `0` iff no check failed (`skip` is non-failing). `1` otherwise.

## `rover config show`

```text
rover config show
```

Prints the effective configuration as TOML, every leaf annotated with its source (`defaults`, `file`, or `env`). The full dotted key is included in each comment so `grep ssrf.level` against the output matches the right line.

Example output:

```toml
# rover effective configuration
# defaults | file (~/.config/rover/config.toml) | env

[cache]
default_ttl = "1h"  # from: defaults (cache.default_ttl)
min_ttl = "5m"      # from: defaults (cache.min_ttl)
...
```

## `rover config set`

```text
rover config set <dotted.key> <value>
```

In-place edit of the config file. Creates the parent directory and the file itself if missing. Preserves comments and key ordering for keys that already exist; appends new keys at the bottom of the appropriate `[section]`. Prints `✓ <key> = <value>  (wrote <path>)` on success.

Examples:

```bash
rover config set fetch.timeout_secs 30
rover config set ssrf.level project
rover config set ssrf.project_root /Users/me/code
rover config set cache.store_raw_html true
```

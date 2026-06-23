---
id: cli
title: CLI
---

# Rover CLI

`rover` runs as an MCP server over stdio, or as a one-shot CLI that fetches a URL and prints clean Markdown to stdout. The other subcommands inspect or maintain what those two produce: the cache, background tasks, config, and local models.

Synopsis:

```text
rover [--config <path>] <subcommand> [args]
```

Global flags:

| Flag | Description |
| --- | --- |
| `--config <path>` | Load this TOML file for the invocation. |

Pass `--config <path>` and Rover loads that file for the run. Leave it off and the `fetch`, `mcp`, `cache`, `task`, `batch`, and `doctor` subcommands run on built-in defaults — no file is read, no default path is searched. The exception is `rover config show` / `rover config set`, which resolve a default path when `--config` is absent: `ROVER_CONFIG`, then the platform config dir (`~/.config/rover/config.toml` on Linux/macOS), then `./rover.toml`. To apply a config to a running server, wire it explicitly:

```sh
rover mcp --config ~/.config/rover/config.toml
```

See [Configuration](/docs/configuration) for the full key reference.

Subcommands:

- `fetch <url>` — one-shot fetch; prints Markdown + frontmatter to stdout.
- `mcp` — start the MCP server over stdio.
- `cache <list|get|purge|stats>` — cache operations.
- `task <id>` — inspect or monitor a long-running task.
- `batch <id>` — inspect or monitor a `batch_fetch` task (alias for `task` with a kind check).
- `doctor` — run environment diagnostics.
- `config <show|set>` — inspect or update the config file.
- `model <download|list|remove|verify>` — manage the local model cache (feature-gated).

Exit code `0` on success; `1` on any failure — config parse error, fetch error, doctor check failure, and so on. `doctor` is the one subcommand whose exit code is a verdict; see its section below.

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
| `--max-tokens <N>` | usize | unset | Token budget. Auto-summarises the extracted body toward `N` when it runs over. |
| `--summarize <JSON>` | string | unset | Explicit summarise blob, applied before `--max-tokens`. |

`--max-tokens` is a target, not a ceiling. When the extracted body exceeds `N`, Rover auto-summarises it toward the budget through the configured `[summarization]` backend — the offline extractive backend by default. The result is best-effort: the extractive backend budgets by summing per-sentence token counts, so the joined summary can land a few tokens over `N`. It emits the summary anyway; it does not error. That is the difference from the MCP `fetch` tool's `max_tokens`, a single-shot hard ceiling that can return `max_tokens_exceeded`. See [Managing token budgets](/docs/token-budgets).

**`--summarize` runs first.** Pass a JSON blob with the same shape as the MCP `summarize` tool's args minus `url` — for example:

```sh
rover fetch https://example.com/long-report \
  --summarize '{"mode":"abstractive","target_tokens":500}'
```

The body is replaced with the summary, then `--max-tokens` applies on top if set. Both flags run the configured `[summarization]` backend. See [Summarising pages](/docs/summarizing).

## `rover mcp`

```text
rover mcp [--ignore-robots]
          [--rate-limit-rpm <N>] [--per-host-concurrency <N>]
          [--global-concurrency <N>] [--max-retries <N>]
```

Starts the MCP server over stdio. Same `--rate-limit-*` and `--ignore-robots` overrides as `fetch`, applied for the lifetime of the server rather than a single request. Reads no config unless you pass `--config`. See [MCP tools](/docs/mcp-tools) for the tool surface it exposes.

## `rover cache`

```text
rover cache list  [--limit <N>] [--offset <N>]
rover cache get   <url>
rover cache purge <pattern> [--all]
rover cache stats
```

Operations against the cache database. See [Caching & freshness](/docs/caching) for TTL and revalidation behaviour.

| Subcommand | Description |
| --- | --- |
| `list` | List cached URLs, most recent first. `--limit` defaults to `20`, `--offset` to `0`. |
| `get <url>` | Print the cached Markdown body for `<url>`. |
| `purge <pattern>` | Delete cache entries whose URL matches the glob (`*`, `?`). The pattern `*` requires `--all` as a safety interlock. |
| `stats` | Print cache size, entry count, and expired-entry count. |

A bare `rover cache purge '*'` wipes the entire cache, so it refuses to run without `--all`. Get the glob wrong without that interlock and you pay for every page again.

## `rover task`

```text
rover task <id> [--monitor] [--cancel]
                [--format human|ndjson] [--from-event <N>]
```

Reads `tasks` and `task_events` from the cache database. Read-only except for `--cancel`.

| Flag | Default | Description |
| --- | --- | --- |
| `--monitor` | off | Stream task events as they're appended. Combine with `--from-event` to resume. |
| `--cancel` | off | Set the task's `cancellation_requested` flag — a single UPDATE. |
| `--format <fmt>` | `human` | `human` prints one line per event; `ndjson` emits one JSON object per line. |
| `--from-event <N>` | unset | Start streaming after this event id; use with `--monitor`. |

## `rover batch`

```text
rover batch <id> [--monitor] [--cancel]
                 [--format human|ndjson] [--from-event <N>]
```

Same flags and semantics as `rover task`, with one guard: the loaded task's `kind` must be `batch_fetch`. Point it at a non-batch id and it errors rather than guessing. See [Batch & background tasks](/docs/batch).

## `rover doctor`

```text
rover doctor [--format human|ndjson]
```

Runs the diagnostic battery sequentially, cheap checks first. The always-run checks:

1. **sqlite_open** — cache database opens cleanly.
2. **sqlite_wal_mode** — WAL journal mode active.
3. **sqlite_schema_version** — schema version matches the binary.
4. **output_dir_writable** — `[output] dir` (or its default) is writable.
5. **network_reachable** — `HEAD https://example.com` succeeds.
6. **extractive_synthesis** — the extractive backend produces output on a fixed input.
7. **backends_authenticate** — every cloud `[backends.*]` block authenticates.
8. **captioners_authenticate** — every configured image captioner authenticates.

Feature-gated checks appear only when the matching feature is compiled in: the headless browser launch check (`headless`), and the local model cache and integrity checks (`local-inference` / `injection-model`). See [Optional features](/docs/features) for the feature matrix.

| Flag | Default | Description |
| --- | --- | --- |
| `--format <fmt>` | `human` | `human`: one line per check with `✓` / `✗` / `-` markers and a summary footer (`all checks ok` / `one or more checks failed`). `ndjson`: one `{check, status, detail?}` JSON object per line. |

The exit code is the verdict. `0` iff no check failed — a `skip` is non-failing, so a feature you didn't compile in won't fail the run. `1` otherwise. That makes `rover doctor` safe to drop straight into CI.

## `rover config show`

```text
rover config show
```

Prints the effective configuration as TOML, every leaf annotated with its source — `defaults`, `file`, or `env`. The full dotted key is included in each comment, so `grep ssrf.level` against the output matches the right line.

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

Edits the config file in place. Creates the parent directory and the file itself if missing. Preserves comments and key ordering for keys that already exist; appends new keys at the bottom of the appropriate `[section]`. Prints `✓ <key> = <value>  (wrote <path>)` on success.

Settable keys:

- `fetch.timeout_secs`
- `cache.default_ttl`, `cache.min_ttl`, `cache.max_ttl`, `cache.override_no_store`, `cache.store_raw_html`
- `ssrf.level`, `ssrf.project_root`
- `rate_limit.requests_per_minute_per_domain`, `rate_limit.per_domain_concurrency`, `rate_limit.global_concurrency`, `rate_limit.max_retries`
- `robots.respect`, `robots.default_ttl`, `robots.failure_ttl`
- `summarization.default_backend`, `summarization.default_mode`, `summarization.default_style`, `summarization.fallback_to_extractive`
- `summarization.tables.target_tokens`, `summarization.tables.focus`
- `tokenizer.default`
- `mcp.heartbeat_interval`, `mcp.reap_threshold`
- `debug.log_level`, `debug.har_path`, `debug.har_body_cap`
- `headless.max_concurrent`, `headless.chrome_executable`
- `image_captions.default`, `image_captions.max_tokens`, `image_captions.max_per_page`, `image_captions.min_width`, `image_captions.min_height`, `image_captions.max_bytes`, `image_captions.max_concurrent`

Examples:

```bash
rover config set fetch.timeout_secs 30
rover config set ssrf.level project
rover config set ssrf.project_root /Users/me/code
rover config set cache.store_raw_html true
rover config set image_captions.default cloud
rover config set headless.max_concurrent 8
```

## `rover model`

```text
rover model download <repo_id>
rover model list
rover model remove <repo_id>
rover model verify [<repo_id>]
```

Download, list, remove, and verify cached local models from HuggingFace Hub. Gated by the `local-inference` feature at compile time — without it, the subcommand is absent.

Models live under `$HF_HOME/hub/` (default `~/.cache/huggingface/hub/`).

### `rover model download`

```text
rover model download <repo_id>
```

Download a model ahead of time. Displays per-file progress to stderr; finishes with a confirmation line.

Example output:

```text
downloading Qwen/Qwen3.5-0.8B from HuggingFace…
  config.json                                                4 KB / 4 KB
  tokenizer.json                                         11 MB / 11 MB
  model.safetensors                                     1.6 GB / 1.6 GB
✓ cached at ~/.cache/huggingface/hub/models--Qwen--Qwen3.5-0.8B
```

### `rover model list`

```text
rover model list
```

List all cached models with their disk sizes.

Example output:

```text
~/.cache/huggingface/hub
  Qwen/Qwen3.5-0.8B   1.6 GB
  Qwen/Qwen3-4B       8.1 GB
```

### `rover model remove`

```text
rover model remove <repo_id>
```

Remove a cached model and free the disk it held. Returns a confirmation with the freed size.

Example output:

```text
removed ~/.cache/huggingface/hub/models--Qwen--Qwen3.5-0.8B (1.6 GB freed)
```

### `rover model verify`

```text
rover model verify [<repo_id>]
```

Re-hashes cached model files and compares them against the integrity manifest (`.rover-integrity.toml`) recorded at download time. With a `<repo_id>`, it verifies that one model; without, it verifies every cached model. Exits non-zero if any file has been modified or is missing. See [Security & threat model](/docs/security) §"Local model files" for the full integrity model.

Example output:

```text
OK    Qwen/Qwen3.5-0.8B  (4 files, revision a1b2c3d)
FAIL  Qwen/Qwen3-4B  (revision e4f5a6b)
        model.safetensors: modified (expected sha256:…, got sha256:…)
```

The same check runs automatically before any local model is loaded for inference, and the `local_model_integrity` check in `rover doctor` reports the same status. Bypass it with `--unsafe-disable-model-integrity-check` (or `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK=1`) — a security-sensitive escape hatch that logs a warning at startup.

:::note
Gated by `local-inference`. When it is not compiled in, `rover model --help` returns an unrecognized-subcommand error.
:::

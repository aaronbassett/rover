---
id: cli
title: CLI
---

# Rover CLI

`rover` runs as an MCP server over stdio, or as a one-shot CLI that fetches a URL and prints clean Markdown to stdout. The remaining subcommands inspect or maintain what those two produce: the cache, background tasks, config, and local models.

Synopsis:

```text
rover [--config <path>] <subcommand> [args]
```

Global flags:

| Flag | Description |
| --- | --- |
| `--config <path>` | Load this TOML file for the invocation. The file must exist. |

Every subcommand — `fetch`, `mcp`, `cache`, `task`, `batch`, `doctor`, and `config show` / `set` — resolves the same config file. With `--config <path>`, Rover loads that file and errors if it is missing. Without it, Rover loads the default config: the file named by `ROVER_CONFIG` if set, otherwise the platform config file (`~/.config/rover/rover.toml` on Linux/macOS) or a project-local `./rover.toml`, whichever exists first. If none exists, built-in defaults apply. A file written by `rover config set` is therefore picked up by `rover fetch` and `rover mcp` without passing `--config`.

```sh
rover config set ssrf.level loopback   # writes ~/.config/rover/rover.toml
rover fetch http://127.0.0.1:8080/      # honors it — no --config needed
```

See [Configuration](/docs/configuration) for the full key reference.

Subcommands:

- `fetch <url>` runs a one-shot fetch and prints Markdown plus frontmatter to stdout.
- `mcp` starts the MCP server over stdio.
- `cache <list|get|purge|stats>` runs cache operations.
- `task <id>` inspects or monitors a long-running task.
- `batch <id>` inspects or monitors a `batch_fetch` task (alias for `task` with a kind check).
- `doctor` runs environment diagnostics.
- `config <show|set>` inspects or updates the config file.
- `meta <use|hook>` wires Rover into an agent harness, or runs the hook handler that wiring installs.
- `model <download|list|remove|verify>` manages the local model cache (feature-gated).

Exit code is `0` on success and `1` on any failure: config parse error, fetch error, doctor check failure, and so on. `doctor` is the one subcommand whose exit code is a verdict; see its section below.

## `rover fetch`

```text
rover fetch <url> [--force-refresh] [--ignore-robots]
             [--user-agent <UA>] [--timeout-secs <N>]
             [--rate-limit-rpm <N>] [--per-host-concurrency <N>]
             [--global-concurrency <N>] [--max-retries <N>]
             [--max-tokens <N>] [--summarize <JSON>]
```

Fetches `<url>` through the cache-aware orchestrator (`fetch_with_cache`), runs the extraction pipeline, and prints a frontmatter-wrapped Markdown document to stdout.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `--force-refresh` | bool | off | Bypass the cache and re-fetch from origin. |
| `--ignore-robots` | bool | off | Skip the robots.txt gate for this fetch. CLI-only escape hatch. |
| `--user-agent <UA>` | string | from `[fetch]` | Override `user_agent` for this request (UA header, redirects, image sub-fetches, robots matching). |
| `--timeout-secs <N>` | u64 | from `[fetch]` | Override `timeout_secs` (per-request timeout). Must be `> 0`. |
| `--rate-limit-rpm <N>` | u32 | from `[rate_limit]` | Override `requests_per_minute_per_domain`. |
| `--per-host-concurrency <N>` | u32 | from `[rate_limit]` | Override `per_domain_concurrency`. Clamped to `>= 1`. |
| `--global-concurrency <N>` | u32 | from `[rate_limit]` | Override `global_concurrency`. Clamped to `>= 1`. |
| `--max-retries <N>` | u8 | from `[rate_limit]` | Override `max_retries`. |
| `--max-tokens <N>` | usize | unset | Token budget. Auto-summarises the extracted body toward `N` when it runs over. |
| `--summarize <JSON>` | string | unset | Explicit summarise blob, applied before `--max-tokens`. |

`--max-tokens` is a target, not a ceiling. When the extracted body runs over `N`, Rover summarises it toward the budget through the configured `[summarization]` backend, which defaults to the offline extractive one. The result is best-effort. The extractive backend budgets by summing per-sentence token counts, so the joined summary can land a few tokens over `N`. It emits the summary anyway and does not error. The MCP `fetch` tool's `max_tokens` behaves differently: it's a single-shot hard ceiling that can return `max_tokens_exceeded`. See [Managing token budgets](/docs/token-budgets).

`--summarize` runs first. Pass a JSON blob with the same shape as the MCP `summarize` tool's args minus `url`, for example:

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
          [--http [--bind <ADDR>]]
```

Starts the MCP server over stdio. Takes the same `--rate-limit-*` and `--ignore-robots` overrides as `fetch`, applied for the lifetime of the server rather than a single request. Loads the resolved default config (or `--config <path>`) at startup, like every other subcommand. See [MCP tools](/docs/mcp-tools) for the tool surface it exposes.

### `rover mcp --http`

Serve MCP over Streamable HTTP instead of stdio, so multiple agents can share one running instance and its cache.

| Flag | Default | Description |
| --- | --- | --- |
| `--http` | off | Selects the HTTP transport. Not feature-gated — every build has it. |
| `--bind <ADDR>` | `[http] bind` | Requires `--http`. Overrides `ROVER_HTTP_BIND` and `[http] bind`. |

Bind resolution: `--bind` → `ROVER_HTTP_BIND` → `[http] bind` → `127.0.0.1:7683`.

| Endpoint | Behaviour |
| --- | --- |
| `POST /mcp` | The JSON-RPC endpoint. Stateless, JSON responses only — Rover sends no server-initiated messages, so SSE framing has nothing to carry. |
| `GET /healthz` | Liveness. `200` with the running Rover version. |
| `GET /readyz` | Readiness. Opens the cache database; `200` if it answers, `503` if it doesn't. |

Neither health endpoint sits behind `ROVER_HTTP_TOKEN`. See [`[http]`](/docs/configuration#http) for the config block and [Deployment](/docs/deployment) for the container setup, request-size limits, and the security posture of running this exposed on a network.

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

A bare `rover cache purge '*'` would wipe the whole cache, so it refuses to run without `--all`. Get the glob wrong without that interlock and you pay to fetch every page again.

## `rover task`

```text
rover task <id> [--monitor] [--cancel]
                [--format human|ndjson] [--from-event <N>]
```

Reads `tasks` and `task_events` from the cache database. Read-only except for `--cancel`.

| Flag | Default | Description |
| --- | --- | --- |
| `--monitor` | off | Stream task events as they're appended. Combine with `--from-event` to resume. |
| `--cancel` | off | Set the task's `cancellation_requested` flag, a single UPDATE. |
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

1. `sqlite_open`: cache database opens cleanly.
2. `sqlite_wal_mode`: WAL journal mode active.
3. `sqlite_schema_version`: schema version matches the binary.
4. `output_dir_writable`: `output.dir` (or its default) is writable.
5. `network_reachable`: `HEAD https://example.com` succeeds.
6. `extractive_synthesis`: the extractive backend produces output on a fixed input.
7. `backends_authenticate`: every cloud `[backends.*]` block authenticates.
8. `captioners_authenticate`: every configured image captioner authenticates.

Feature-gated checks appear only when the matching feature is compiled in: the headless browser launch check (`headless`), and the local model cache and integrity checks (`local-inference` / `injection-model`). See [Optional features](/docs/features) for the feature matrix.

| Flag | Default | Description |
| --- | --- | --- |
| `--format <fmt>` | `human` | `human`: one line per check with `✓` / `✗` / `-` markers and a summary footer (`all checks ok` / `one or more checks failed`). `ndjson`: one `{check, status, detail?}` JSON object per line. |

The exit code is the verdict. It's `0` iff no check failed, and `1` otherwise. A `skip` is non-failing, so a feature you didn't compile in won't fail the run. That makes `rover doctor` safe to drop straight into CI.

## `rover config show`

```text
rover config show
```

Prints the effective configuration as TOML, every leaf annotated with its source: `defaults`, `file`, or `env`. Each comment includes the full dotted key, so `grep ssrf.level` against the output matches the right line.

Example output:

```toml
# rover effective configuration
# defaults | file (~/.config/rover/rover.toml) | env

[cache]
default_ttl = "1h"  # from: defaults (cache.default_ttl)
min_ttl = "5m"      # from: defaults (cache.min_ttl)
...
```

## `rover config set`

```text
rover config set <dotted.key> <value>
```

Edits the config file in place, creating the parent directory and the file itself if missing. For keys that already exist, it preserves comments and key ordering; new keys are appended at the bottom of the matching `[section]`. Prints `✓ <key> = <value>  (wrote <path>)` on success.

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
- `image_captions.default`, `image_captions.max_tokens`, `image_captions.max_per_page`, `image_captions.min_width`, `image_captions.min_height`, `image_captions.max_bytes`
- `image_captions.cache.enabled`, `image_captions.cache.ttl`, `image_captions.cache.restrict_to`, `image_captions.cache.store_raw_image`
- `http.bind`, `http.allow_server_paths`

Examples:

```bash
rover config set fetch.timeout_secs 30
rover config set ssrf.level project
rover config set ssrf.project_root /Users/me/code
rover config set cache.store_raw_html true
rover config set image_captions.default cloud
rover config set headless.max_concurrent 8
```

## `rover meta`

```text
rover meta use  <claude|general> [-s|--scope <local|user|project>]
rover meta hook <claude|general>
```

Wires Rover into an agent harness, or runs the hook handler that wiring installs. Unlike the other subcommands, `meta` does not read the Rover config file.

### `rover meta use`

```text
rover meta use <harness> [-s|--scope <local|user|project>]
```

Registers Rover with a harness in one step: MCP server, steering hooks (where supported), and a rules-file block. It is idempotent and validate-then-apply: it parses every file it would touch first, and aborts without writing anything if a prerequisite is missing or a target file is malformed JSON. A per-file summary prints to stderr on success.

| Argument | Values | Default | Description |
| --- | --- | --- | --- |
| `<harness>` | `claude`, `general` | — | Target harness. |
| `-s, --scope <scope>` | `local`, `user`, `project` | `local` | Where config is written. `general` ignores it and always uses the project root. |

`claude` delegates MCP registration to the `claude` CLI (`claude mcp add rover -s <scope> -- rover mcp`, skipped if already registered), installs a `SessionStart` hook (matched to `startup|clear|compact`, so it re-runs on every session entry) and a `WebFetch`-matched `PreToolUse` hook that both run `rover meta hook claude`, and writes a rules block into `CLAUDE.md`. It needs the `claude` binary on `PATH`. Scope decides the files:

| Scope | Hooks file | Rules block |
| --- | --- | --- |
| `local` | `.claude/settings.local.json` | none (`SessionStart` hook carries it) |
| `project` | `.claude/settings.json` | `./CLAUDE.md` |
| `user` | `~/.claude/settings.json` | `~/.claude/CLAUDE.md` |

`general` writes a project-root `mcp.json` (adding a `rover` server, preserving any others) and a rules block in `AGENTS.md`. It installs no hooks. Both files are updated in place on re-run.

See [Quickstart](/docs/quickstart) for the full walkthrough, the per-scope file mapping, and the exact content each piece writes.

### `rover meta hook`

```text
rover meta hook <harness>
```

The runtime hook handler that the `claude` wiring points at. It reads a hook payload on stdin, branches on `hook_event_name`, and prints response JSON for `SessionStart` (an `<EXTREMELY_IMPORTANT_TOOL_UPDATE>`-wrapped briefing with concrete tool-call examples) and `PreToolUse` (a non-blocking reminder, with no `permissionDecision`); any other event prints nothing. `rover meta hook general` errors, since `general` installs no hooks. You don't normally run this by hand; Claude Code invokes it per the `settings.json` entries that `rover meta use claude` writes.

## `rover model`

```text
rover model download <repo_id>
rover model list
rover model remove <repo_id>
rover model verify [<repo_id>]
```

Download, list, remove, and verify cached local models from HuggingFace Hub. Gated by the `local-inference` feature at compile time; without it, the subcommand is absent.

Models live under `$HF_HOME/hub/` (default `~/.cache/huggingface/hub/`).

### `rover model download`

```text
rover model download <repo_id>
```

Downloads a model ahead of time. Shows per-file progress to stderr and finishes with a confirmation line.

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

Lists all cached models with their disk sizes.

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

Removes a cached model and frees the disk it held. Returns a confirmation with the freed size.

Example output:

```text
removed ~/.cache/huggingface/hub/models--Qwen--Qwen3.5-0.8B (1.6 GB freed)
```

### `rover model verify`

```text
rover model verify [<repo_id>]
```

Re-hashes cached model files and compares them against the integrity manifest (`.rover-integrity.toml`) recorded at download time. Pass a `<repo_id>` to verify that one model; omit it to verify every cached model. Exits non-zero if any file has been modified or is missing. See [Security & threat model](/docs/security) §"Local model files" for the full integrity model.

Example output:

```text
OK    Qwen/Qwen3.5-0.8B  (4 files, revision a1b2c3d)
FAIL  Qwen/Qwen3-4B  (revision e4f5a6b)
        model.safetensors: modified (expected sha256:…, got sha256:…)
```

The same check runs automatically before any local model is loaded for inference, and the `local_model_integrity` check in `rover doctor` reports the same status. To bypass it, use `--unsafe-disable-model-integrity-check` (or `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK=1`), a security-sensitive escape hatch that logs a warning at startup.

:::note
Gated by `local-inference`. When it is not compiled in, `rover model --help` returns an unrecognized-subcommand error.
:::

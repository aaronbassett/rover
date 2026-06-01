# Rover Configuration

Rover reads a single TOML file. Default location: `$XDG_CONFIG_HOME/rover/config.toml` (typically `~/.config/rover/config.toml`). Override with `--config <path>` on every subcommand, or set `ROVER_CONFIG=/path/to/config.toml`.

All sections and keys are optional — defaults below apply when absent. Unknown keys are rejected at load time (`deny_unknown_fields`). Durations parse via `humantime` (e.g. `"1h"`, `"5m"`, `"7d"`, `"500ms"`).

Inspect the effective configuration with `rover config show`. Mutate a single setting with `rover config set <dotted.key> <value>` — see `cli.md`.

## `[fetch]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `user_agent` | string | `Rover/<version> (+https://github.com/aaronbassett/rover)` | UA header on every outbound HTTP request. |
| `timeout_secs` | integer | `15` | Per-request timeout in seconds. Must be `> 0`. |

## `[ssrf]` (M8)

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `level` | enum | `"strict"` | One of `strict`, `loopback`, `project`, `lan`, `none`. |
| `project_root` | path | `"."` | Used when `level = "project"`. The descendant root for `file://` URLs after `std::fs::canonicalize` resolves symlinks. |

### Level semantics

| Level | Allows |
| --- | --- |
| `strict` | Public IPs only; `http` / `https` only. |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs descendant of `project_root` after symlink resolution. |
| `lan` | Project + RFC1918 + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. The always-floor (link-local, multicast, broadcast, `0.0.0.0`, `::`) is still blocked. |

Unknown level strings are rejected the first time the SSRF policy is consulted (typed `SsrfError::UnknownLevel`).

## `[cache]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default_ttl` | duration | `"15m"` | TTL used when an upstream response has no `Cache-Control: max-age`. Kept short so cache-poisoned or stale content has a small blast radius; origins can opt into longer caching via response headers. |
| `min_ttl` | duration | `"5m"` | Floor for any TTL derived from an upstream header. Must be `<= default_ttl`. |
| `max_ttl` | duration | `"7d"` | Ceiling for any TTL. Must be `>= default_ttl`. |
| `stale_while_revalidate_window` | duration | `"5m"` | How long after `expires_at` an entry is still eligible for the stale-while-revalidate fast-path. Inside the window, `fetch` may return the stale row immediately and queue a background `revalidate` task. Beyond it, the row is treated as a cache miss and refetched synchronously, so callers never receive arbitrarily old content. |
| `override_no_store` | bool | `false` | When `true`, cache responses even if they sent `Cache-Control: no-store`. |
| `override_no_store_domains` | array<string> | `[]` | Per-domain allowlist for `override_no_store`. Lowercased on load. |
| `store_raw_html` | bool | `false` | When `true`, store the zstd-compressed raw HTML alongside the extracted Markdown. Enables the `raw_html` field in `count_tokens mode=estimates`. |

### Stale-while-revalidate behaviour

When a cache entry's `expires_at` has passed:

- **Within `stale_while_revalidate_window`:** the MCP server returns the stale row immediately and enqueues a background `revalidate` task. The agent sees `cache_status: "stale"` and a `revalidation_task_id` it can monitor.
- **Beyond `stale_while_revalidate_window`:** the row is treated as a miss; `fetch` refetches synchronously and writes through the cache.

The CLI (`rover fetch`) **always** revalidates synchronously regardless of the window — a one-shot CLI process has no in-process scheduler to drain the background task queue, so it cannot rely on SWR. Set `default_ttl` and `max_ttl` to reflect how fresh you actually need cached content; set `stale_while_revalidate_window` to bound how stale the SWR fast-path is allowed to serve.

## `[tokenizer]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default` | enum | `"o200k"` | Default tokenizer family. One of `o200k` (GPT-4o), `cl100k` (GPT-4), `claude`. |

## `[mcp]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `heartbeat_interval` | duration | `"5s"` | Per-task heartbeat write cadence. Must be `> 0`. |
| `reap_threshold` | duration | `"60s"` | If a task hasn't heartbeat within this window its owning process is considered dead and the task is marked `failed`. Must be `> 0`. |

## `[output]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `dir` | path | unset | Where extracted assets (downloaded images, table CSVs) are written. When unset, falls back to `ROVER_OUTPUT_DIR` if set, otherwise `${data_local_dir}/rover/output`. |

## `[rate_limit]`

All HTTP-bound code paths share a single `Pacer` built from this block at startup.

| Key | Type | Default | Constraints | Description |
| --- | --- | --- | --- | --- |
| `requests_per_minute_per_domain` | integer | `60` | `1..=6000` | Per-host RPM budget. |
| `per_domain_concurrency` | integer | `2` | `>= 1` | Max simultaneous in-flight requests per host. |
| `global_concurrency` | integer | `8` | `>= 1` | Max simultaneous in-flight requests across all hosts. |
| `max_retries` | integer | `3` | `<= 10` | Retries on transient failures (network, 5xx, 429). |
| `initial_backoff` | duration | `"500ms"` | `<= max_backoff` | First backoff after a transient failure. |
| `max_backoff` | duration | `"30s"` | | Backoff ceiling. |
| `retry_after_ceiling` | duration | `"5m"` | `> 0` | Maximum `Retry-After` value Rover will respect inline. |
| `jitter_seed` | integer or unset | unset | | Deterministic seed for backoff jitter — set in tests for reproducible timing; entropy otherwise. |
| `deferred_retry_threshold_secs` | integer | `30` | | Server-provided `Retry-After` above this threshold converts a synchronous fetch into a deferred `retry` task instead of sleeping in-line. |

## `[robots]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `respect` | bool | `true` | When `false`, the robots.txt gate is bypassed for every fetch. |
| `ignore_domains` | array<string> | `[]` | Hosts for which robots.txt is not fetched and rules are not enforced. Lowercased on load. |
| `default_ttl` | duration | `"24h"` | TTL used when the robots.txt response has no `Cache-Control: max-age`. |
| `failure_ttl` | duration | `"5m"` | TTL used when the robots.txt fetch fails (5xx, transport error) — fail-closed. Must be `<= default_ttl`. |

## `[summarization]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default_backend` | string | `"default"` | Name of a `[backends.<name>]` block to use when no `backend` arg is supplied. Must exist in `[backends]` (or be the implicit `default` extractive). |
| `default_mode` | string | `"abstractive"` | One of `extractive`, `abstractive`, `headlines`. |
| `default_style` | string | `"prose"` | One of `bullet`, `prose`, `executive`. |
| `fallback_to_extractive` | bool | `true` | When a cloud backend fails (auth, rate-limit, model error, invalid request), retry the request through an extractive backend. Requires at least one extractive backend to exist. |

## `[summarization.tables]`

Controls the per-table defaults consumed by the `tables: {mode: "summarize"}` hook in the MCP `fetch` tool.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `target_tokens` | integer | `150` | Target token count for each generated table summary. |
| `focus` | string | `"Describe what this table shows. Highlight any extreme values or notable rows."` | Focus prompt passed to the summarizer for every table. |

## `[image_captions]` (M9)

Configuration for image caption generation when `images.mode = "caption"` is used in the MCP `fetch` tool.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default` | string | `"default"` | Name of a configured `[captioners.<name>]` block to use when no `captioner` override is supplied in the MCP tool call. |
| `max_tokens` | integer | `100` | Maximum token count per caption. |
| `max_per_page` | integer | `10` | Maximum number of images to caption per page. Captions are generated for the first N images; the rest are dropped. |
| `min_width` | integer | `32` | Skip images narrower than this (pixels). |
| `min_height` | integer | `32` | Skip images shorter than this (pixels). |
| `max_bytes` | integer | `2097152` | Skip images larger than this (bytes; default 2 MiB). |
| `max_concurrent` | integer | `2` | Number of concurrent caption-generation tasks. |

Example:

```toml
[image_captions]
default = "cloud"
max_tokens = 80
max_per_page = 5
min_width = 64
min_height = 64
max_bytes = 1048576
max_concurrent = 4
```

## `[captioners.<name>]` (M9)

Free-form section: repeat for each named captioner. Mirrors the `[backends.<name>]` structure.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `cloud` or `local`. |
| `provider` | string | yes (cloud) | One of `openai`, `anthropic`, `gemini`, `xai`, `groq`, `deepseek`, `together`, `fireworks`, `openai_compat`. Ignored for `kind = "local"`. |
| `model` | string | yes | For cloud: model id (e.g. `gpt-4-vision`). For local: HuggingFace repo id (requires `local-vision` feature). |
| `base_url` | string | yes for `openai_compat`; unused otherwise | Custom endpoint. For `openai_compat`, auto-normalized to end in `/v1/`. |
| `api_key_env` | string | no | Env var holding the API key. When unset for cloud providers, the genai library falls back to its provider-default env var. Unused for `kind = "local"`. |

Example:

```toml
[captioners.cloud]
kind = "cloud"
provider = "openai"
model = "gpt-4-vision"
api_key_env = "OPENAI_API_KEY"

[captioners.local-vlm]
kind = "local"
model = "HuggingFaceTB/SmolVLM-256M-Instruct"
```

## `[headless]` (M9)

Configuration for the headless browser renderer when the `headless` feature is compiled in.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `auto_detect_spa` | bool | `true` | When true and MCP `fetch` does not specify `headless.mode`, auto-detect single-page apps via heuristics and render via headless. |
| `default_wait` | string | `"domcontentloaded"` | When to consider a render done. `domcontentloaded` returns as soon as the initial HTML is parsed. `networkidle0` additionally waits until the network fully settles — zero requests in flight for a continuous 500 ms (bounded by `timeout_secs`) — slower, but the right choice for SPAs that fetch their content over XHR after load (a single pending XHR still blocks completion). |
| `timeout` | duration | `"30s"` | Per-render timeout. |
| `max_concurrent` | integer | `4` | Number of concurrent headless render tasks. |
| `chrome_executable` | string | unset | Path to the Chrome/Chromium executable. When unset, attempts auto-detection (searches PATH, common install locations). |

The MCP `fetch` tool accepts a typed `headless` argument (see `docs/mcp-tools.md`) which overrides `auto_detect_spa`, `default_wait`, and `timeout` on a per-call basis.

## `[backends.<name>]`

Free-form section: repeat for each named backend. See `backends.md` for the full reference and worked examples.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `extractive`, `cloud`, or `local`. |
| `provider` | string | yes (cloud) | One of `openai`, `anthropic`, `gemini`, `xai`, `groq`, `deepseek`, `together`, `fireworks`, `openai_compat`. Ignored for `kind = "local"`. |
| `model` | string | yes | For cloud: literal model id (e.g. `gpt-4o-mini`). For local: HuggingFace repo id (requires `local-inference` feature). |
| `base_url` | string | yes for `openai_compat`; unused otherwise | Custom endpoint. For `openai_compat`, auto-normalized to end in `/v1/`. |
| `api_key_env` | string | no | Env var holding the API key. When unset for cloud providers, the genai library falls back to its provider-default env var (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.). Unused for `kind = "local"`. |

When the `[backends]` map is empty, Rover installs an implicit `default` extractive backend so a fresh install works offline without any configuration. Adding any explicit `[backends.*]` block disables that implicit injection.

## `[debug]` (M8)

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `har_path` | string | `""` (off) | Path to write a HAR-1.2 file capturing every request/response. Empty disables HAR recording. |
| `har_body_cap` | integer or humansize | `65536` (64 KiB) | Maximum response body bytes captured per entry. Accepts raw bytes (`65536`) or humansize strings (`"64KiB"`, `"1MiB"`, `"1GiB"`). |
| `log_level` | string | `"info"` | Default tracing filter. Overridden by `RUST_LOG` when set. |

## Environment overrides

| Env var | Overrides | Notes |
| --- | --- | --- |
| `ROVER_CONFIG` | config file path | Used by `rover config show` / `set` when no `--config` is passed. |
| `ROVER_DATA_DIR` | data dir (cache db, downloads) | |
| `ROVER_OUTPUT_DIR` | `output.dir` | |
| `ROVER_LOG_LEVEL` | `debug.log_level` | |
| `RUST_LOG` | tracing filter | Takes precedence over `debug.log_level`. |

`rover config show` annotates every leaf with its effective source (`defaults`, `file`, or `env`). Only the 25 leaves listed by `provenance::known_leaves()` are tracked — they cover every section above except the dynamic `[backends.*]` map and the rate-limit timing knobs (`initial_backoff`, `max_backoff`, `retry_after_ceiling`, `jitter_seed`, `deferred_retry_threshold_secs`, `max_retries`).

## Worked example

```toml
[fetch]
timeout_secs = 30

[ssrf]
level = "project"
project_root = "/Users/me/code"

[cache]
default_ttl = "6h"
store_raw_html = true

[rate_limit]
requests_per_minute_per_domain = 30
per_domain_concurrency = 2

[robots]
respect = true
ignore_domains = ["staging.internal"]

[summarization]
default_backend = "fast"
default_mode = "abstractive"

[summarization.tables]
target_tokens = 200

[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[backends.local]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234"      # auto-normalized to /v1/
model = "qwen2.5-0.5b-instruct"

[backends.default]
kind = "extractive"

[debug]
har_path = "/tmp/rover.har"
har_body_cap = "256KiB"
log_level = "info"
```

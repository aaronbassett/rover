---
id: configuration
title: Configuration
---

# Configuration

Every subcommand resolves the same config file. Pass `--config <path>` to load a specific file (it must exist). Without `--config`, Rover loads the default config: the file named by `ROVER_CONFIG` if set, otherwise the platform config file (`~/.config/rover/rover.toml` on Linux/macOS) or a project-local `./rover.toml`, whichever exists first. If none exists, built-in defaults apply.

This is uniform across `fetch`, `mcp`, `cache`, `task`, `batch`, `doctor`, and `config show` / `set` — so a file written by `rover config set` is read back by `rover fetch` and `rover mcp` without `--config`. `rover config show` prints the effective settings; `rover config set <dotted.key> <value>` changes one. See the [CLI reference](/docs/cli).

Every section and key is optional, and the defaults below apply when a key is absent. Unknown keys are rejected at load time (`deny_unknown_fields`), so a typo fails loudly instead of being silently ignored. Durations parse via `humantime`: `"1h"`, `"5m"`, `"7d"`, `"500ms"`.

## `[fetch]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `user_agent` | string | `Rover/<version> (+https://github.com/aaronbassett/rover)` | UA header on every outbound HTTP request. |
| `timeout_secs` | integer | `15` | Per-request timeout in seconds. Must be `> 0`. |

## `[ssrf]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `level` | enum | `"strict"` | One of `strict`, `loopback`, `project`, `lan`, `none`. |
| `project_root` | path | `"."` | Used when `level = "project"`. The descendant root for `file://` URLs after `std::fs::canonicalize` resolves symlinks. |

### Level semantics

Each level is a superset of the one above it.

| Level | Allows |
| --- | --- |
| `strict` | Public IPs only; `http` / `https` only. |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs descendant of `project_root` after symlink resolution. |
| `lan` | Project + RFC1918 + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. The always-floor (link-local, multicast, broadcast, `0.0.0.0`, `::`) is still blocked. |

Unknown level strings are rejected the first time the SSRF policy is consulted (typed `SsrfError::UnknownLevel`). The always-floor blocks the dangerous ranges at every level, `none` included. See [Security & threat model](/docs/security).

## `[prompt_injection]`

These keys tune the prompt-injection detectors for the content-returning MCP tools: `fetch`, `summarize`, `get_metadata`, and transitively `batch_fetch`. They act on output only. They do not control the nonce wrapper, which is always on and can't be turned off. See [Trust & prompt injection](/docs/trust) for the model behind the layers and [MCP tools](/docs/mcp-tools) for the wire contract.

```toml
[prompt_injection]
level = "moderate"      # strict | high | moderate | low | disabled  (output side)
model = "disabled"      # disabled | deberta-base | deberta-small | prompt-guard-2-86m | prompt-guard-2-22m | <hf-id>
model_threshold = 0.9   # model malicious-score threshold

[prompt_injection.allowlist]   # URL globs; a matching URL skips that method on OUTPUT
wrap = []                      # e.g. ["https://*.internal.example.com/*"]   ("*" disables entirely)
patterns = []
model = []

[prompt_injection.agent_overrides]   # grant the agent per-call `security` control (default: all deny)
wrap = false
patterns = false
model = false
level = false
```

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `level` | enum | `"moderate"` | Output action on a detector hit. One of `strict`, `high`, `moderate`, `low`, `disabled`. |
| `model` | string | `"disabled"` | Model detector. One of `disabled`, the presets `deberta-base`, `deberta-small`, `prompt-guard-2-86m`, `prompt-guard-2-22m`, or a custom HuggingFace `owner/repo` id. |
| `model_threshold` | float | `0.9` | Malicious-score threshold above which a model window is flagged. |

`[prompt_injection.allowlist]` holds per-method URL globs (`wrap`, `patterns`, `model`). A URL matching a method's list skips that method on output for that URL, and a bare `"*"` disables the method entirely.

`[prompt_injection.agent_overrides]` grants the agent per-call control through the MCP `security` arg. Each of `wrap`, `patterns`, `model`, `level` defaults to `false`. A `security` field is honored only when its grant here is `true`; otherwise it is ignored and recorded in the response's `prompt_injection.overrides_attempted`.

Both the `level` and `model` strings are validated at first use (`guard::GuardConfig::from_config`), which surfaces a typed error rather than a serde error. The TOML parser accepts unknown values; the guard rejects them when it's built.

The `model` detector needs Rover compiled with the `injection-model` feature, and it downloads an ONNX DeBERTa classifier from HuggingFace on first use. Verify the build with `rover doctor`. Without the feature, a configured `model` is ignored with a warning, but the structural wrapper and pattern detector are always compiled in, so coverage never drops to zero. Internal-inference hardening, which cleans the content Rover feeds to its own summarizer and caption models, is always on and can't be turned off. See [Optional features](/docs/features) for the feature-flag matrix.

## `[cache]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default_ttl` | duration | `"15m"` | TTL used when an upstream response has no `Cache-Control: max-age`. Kept short so cache-poisoned or stale content has a small blast radius; origins can opt into longer caching via response headers. |
| `min_ttl` | duration | `"5m"` | Floor for any TTL derived from an upstream header. Must be `<= default_ttl`. |
| `max_ttl` | duration | `"7d"` | Ceiling for any TTL. Must be `>= default_ttl`. |
| `stale_while_revalidate_window` | duration | `"5m"` | How long after `expires_at` an entry stays eligible for the stale-while-revalidate fast-path. Inside the window, `fetch` may return the stale row immediately and queue a background `revalidate` task. Beyond it, the row is treated as a cache miss and refetched synchronously, so callers never receive arbitrarily old content. |
| `override_no_store` | bool | `false` | When `true`, cache responses even if they sent `Cache-Control: no-store`. |
| `override_no_store_domains` | `array<string>` | `[]` | Per-domain allowlist for `override_no_store`. Lowercased on load. |
| `store_raw_html` | bool | `false` | When `true`, store the zstd-compressed raw HTML alongside the extracted Markdown. Enables the `raw_html` field in `count_tokens mode=estimates`. |

### Stale-while-revalidate behaviour

What happens when a cache entry's `expires_at` has passed depends on how far past it you are.

Within `stale_while_revalidate_window`, the MCP server returns the stale row immediately and enqueues a background `revalidate` task. The agent sees `cache_status: "stale"` and a `revalidation` block it can monitor.

Beyond `stale_while_revalidate_window`, the row is treated as a miss. `fetch` refetches synchronously and writes through the cache.

The CLI (`rover fetch`) always revalidates synchronously, regardless of the window. A one-shot CLI process has no in-process scheduler to drain the background task queue, so it can't rely on SWR at all. Set `default_ttl` and `max_ttl` to how fresh you need cached content; set `stale_while_revalidate_window` to bound how stale the SWR fast-path may serve. See [Caching & freshness](/docs/caching) for how these interact with upstream `Cache-Control`.

## `[tokenizer]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default` | enum | `"o200k"` | Default tokenizer family for token counting. One of `o200k` (GPT-4o, the default), `cl100k` (GPT-4), `claude`, `llama3`, `qwen3`. Pick the family that matches the model you actually pay for. Token counts differ between families, so the wrong tokenizer estimates the wrong budget. |

## `[mcp]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `heartbeat_interval` | duration | `"5s"` | Per-task heartbeat write cadence. Must be `> 0`. |
| `reap_threshold` | duration | `"60s"` | If a task hasn't heartbeat within this window, its owning process is considered dead and the task is marked `failed`. Must be `> 0`. |

## `[http]`

Consulted only by `rover mcp --http`; the stdio transport ignores it entirely. See [`rover mcp --http`](/docs/cli) for the flags and endpoints, and [Deployment](/docs/deployment) for running it in a container.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `bind` | string | `"127.0.0.1:7683"` | Listen address. Overridden by `ROVER_HTTP_BIND`, then `--bind`. |
| `allowed_hosts` | `array<string>` | `[]` | Inbound `Host` allow-list. `[]` derives from the bind: a loopback bind enforces `localhost` / `127.0.0.1` / `::1`; a non-loopback bind disables the check, since Host validation defends a browser-reachable localhost service against DNS rebinding, and a deliberately exposed port has nothing for it to protect. `["*"]` disables it explicitly regardless of bind. Any other list is used verbatim. |
| `allowed_origins` | `array<string>` | `[]` | Browser `Origin` allow-list. Empty disables Origin validation. |
| `allow_server_paths` | bool | `false` | Permit `tables.mode = "csv_file"` and `images.mode = "download"` over HTTP. Both write to Rover's own filesystem and return an absolute path a remote caller can't read — only correct when client and server share a volume at identical paths. |

The bearer token has no config key. Set `ROVER_HTTP_TOKEN` in the environment; unset means every request is accepted unauthenticated. Generate one with `openssl rand -hex 32` — see [Deployment](/docs/deployment) for why that matters more here than for most tokens.

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
| `jitter_seed` | integer or unset | unset | | Deterministic seed for backoff jitter. Set it in tests for reproducible timing; entropy otherwise. |
| `deferred_retry_threshold_secs` | integer | `30` | | A server-provided `Retry-After` above this threshold turns a synchronous fetch into a deferred `retry` task instead of sleeping in-line. |

## `[robots]`

Rover is an agent's browser, not a spider — it fetches the page that was explicitly requested, one at a time — so the robots.txt gate is **off by default**. robots.txt governs automated crawling; set `respect = true` to opt into enforcement.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `respect` | bool | `false` | When `true`, every fetch is gated on the host's robots.txt. Off by default (Rover is an agent browser, not a crawler). |
| `ignore_domains` | `array<string>` | `[]` | Hosts for which robots.txt is not fetched and rules are not enforced. Lowercased on load. |
| `default_ttl` | duration | `"24h"` | TTL used when the robots.txt response has no `Cache-Control: max-age`. |
| `failure_ttl` | duration | `"5m"` | TTL used when the robots.txt fetch fails (5xx, transport error). Fail-closed. Must be `<= default_ttl`. |

## `[summarization]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default_backend` | string | `"default"` | Name of a `[backends.<name>]` block to use when no `backend` arg is supplied. Must exist in `[backends]` (or be the implicit `default` extractive). |
| `default_mode` | string | `"abstractive"` | One of `extractive`, `abstractive`, `headlines`. |
| `default_style` | string | `"prose"` | One of `bullet`, `prose`, `executive`. |
| `fallback_to_extractive` | bool | `true` | When a cloud backend fails (auth, rate-limit, model error, invalid request), retry the request through an extractive backend. Requires at least one extractive backend to exist. |

## `[summarization.tables]`

Per-table defaults consumed by the `tables: {mode: "summarize"}` hook in the MCP `fetch` tool.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `target_tokens` | integer | `150` | Target token count for each generated table summary. |
| `focus` | string | `"Describe what this table shows. Highlight any extreme values or notable rows."` | Focus prompt passed to the summarizer for every table. |

## `[image_captions]`

Defaults for caption generation when `images.mode = "caption"` is set in the MCP `fetch` tool. See [Images & captioning](/docs/images).

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `default` | string | unset | Name of a configured `[captioners.<name>]` block to use when no `captioner` override is supplied in the MCP tool call. When unset and exactly one captioner is configured, that one is used, so a single-captioner setup needs no `default`. |
| `max_tokens` | integer | `128` | Maximum token count per caption. |
| `max_per_page` | integer | `10` | Maximum successful captions per page (cache hits count). The selection loop stops when this is reached. |
| `min_width` | integer | `200` | Skip images narrower than this (pixels). Filters out spacers, icons, and tracking pixels before they cost a caption. |
| `min_height` | integer | `200` | Skip images shorter than this (pixels). |
| `max_bytes` | integer | `10485760` | Skip images larger than this (bytes; default 10 MiB). Accepts raw bytes or a humansize string (`"10MiB"`). Downloads stream and abort at this limit. |

Example:

```toml
[image_captions]
default = "cloud"
max_tokens = 80
max_per_page = 5
min_width = 64
min_height = 64
max_bytes = "1MiB"
```

### `[image_captions.cache]`

Caption results are cached by image content hash. The image must be downloaded on a hit to derive the key; the cache saves the VLM call, not the download.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | When `false`, skips cache lookup and insert. |
| `ttl` | duration | `[cache].max_ttl` | Caption row TTL. Unset inherits `[cache].max_ttl`. |
| `restrict_to` | enum | `none` | Key scope: `none`, `host`, or `page`. |
| `store_raw_image` | bool | `false` | When `true`, stores the zstd-compressed image bytes alongside the caption. |

`restrict_to` values:

| Value | Key scope |
| --- | --- |
| `none` | Image bytes only — the same bytes reuse the caption anywhere. |
| `host` | Image bytes + image hostname. |
| `page` | Image bytes + image `src` URL. Scopes to the image URL, not the containing page. |

Changing `restrict_to` silently re-derives keys, invalidating prior rows.

## `[captioners.<name>]`

A free-form section: repeat it for each named captioner. It mirrors the `[backends.<name>]` structure.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `cloud`. (The former `local` kind, a native mistralrs vision backend, was removed; for local inference use `provider = "openai_compat"` pointed at a local server.) |
| `provider` | string | yes | One of `openai`, `anthropic`, `gemini`, `xai`, `groq`, `deepseek`, `together`, `fireworks`, `openai_compat`. |
| `model` | string | yes | Model id: a cloud model (e.g. `gpt-4o-mini`) or the model your `openai_compat` server hosts. |
| `base_url` | string | yes for `openai_compat`; unused otherwise | Custom endpoint. For `openai_compat`, auto-normalized to end in `/v1/`. |
| `api_key_env` | string | no | Env var holding the API key. When unset for cloud providers, the genai library falls back to its provider-default env var. Keyless local servers can omit it. |
| `max_concurrent` | integer | no | Max concurrent caption calls to this captioner per page. Default `2`. |
| `max_attempts` | integer | no | Hard cap on provider caption calls per page. Unset ⇒ `3 × image_captions.max_per_page`. Cache hits and pre-caption skips (dimension, size, and per-page budget checks) do not consume this budget. |

Example:

```toml
[captioners.cloud]
kind = "cloud"
provider = "openai"
model = "gpt-4-vision"
api_key_env = "OPENAI_API_KEY"
max_concurrent = 3        # concurrent VLM calls to this provider
# max_attempts = 30       # unset: defaults to 3 × image_captions.max_per_page

# Local inference via an OpenAI-compatible server (ollama, LM Studio, vLLM, ...):
[captioners.ollama]
kind = "cloud"
provider = "openai_compat"
model = "llama3.2-vision"
base_url = "http://localhost:11434/v1"
```

## `[headless]`

The headless browser renderer needs Rover compiled with the `headless` feature. Without it, these keys are inert and a requested headless render falls back to the static fetch. See [JavaScript & dynamic pages](/docs/dynamic-pages) for when a page actually needs a real browser.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `auto_detect_spa` | bool | `true` | When true and MCP `fetch` does not specify `headless.mode`, auto-detect single-page apps via heuristics and render via headless. |
| `launch_delay_secs` | integer | `2` | In Auto mode, seconds to wait after detecting a render is needed (an unrendered SPA, or a bot-protection challenge on the HTTP fetch) and before launching the browser — a breather between the lightweight fetch and the heavier browser hit. `0` disables it. Not applied in `On` mode, which has no detection step. |
| `default_wait` | string | `"domcontentloaded"` | When to consider a render done. `domcontentloaded` returns as soon as the initial HTML is parsed. `networkidle0` additionally waits until the network fully settles (zero requests in flight for a continuous 500 ms, bounded by `timeout_secs`). Slower, but the right choice for SPAs that fetch their content over XHR after load; a single pending XHR still blocks completion. |
| `timeout` | duration | `"30s"` | Per-render timeout. |
| `max_concurrent` | integer | `4` | Number of concurrent headless render tasks. |
| `chrome_executable` | string | unset | Path to the Chrome/Chromium executable. When unset, attempts auto-detection (searches PATH, common install locations). |

The MCP `fetch` tool accepts a typed `headless` argument (see [MCP tools](/docs/mcp-tools)) that overrides `auto_detect_spa`, `default_wait`, and `timeout` per call.

## `[backends.<name>]`

A free-form section: repeat it for each named backend. See [Summarisation backends](/docs/backends) for the full reference and worked examples, and [Summarising pages](/docs/summarizing) for how backends, modes, and styles combine at call time.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `extractive`, `cloud`, or `local`. The `local` kind runs an in-process model and needs Rover compiled with the `local-inference` feature. |
| `provider` | string | yes (cloud) | One of `openai`, `anthropic`, `gemini`, `xai`, `groq`, `deepseek`, `together`, `fireworks`, `openai_compat`. Ignored for `kind = "local"`. |
| `model` | string | yes | For cloud, a literal model id (e.g. `gpt-4o-mini`). For local, a HuggingFace repo id (requires the `local-inference` feature). |
| `base_url` | string | yes for `openai_compat`; unused otherwise | Custom endpoint. For `openai_compat`, auto-normalized to end in `/v1/`. |
| `api_key_env` | string | no | Env var holding the API key. When unset for cloud providers, the genai library falls back to its provider-default env var (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.). Unused for `kind = "local"`. |

When the `[backends]` map is empty, Rover installs an implicit `default` extractive backend, so a fresh install works offline without any configuration. Add any explicit `[backends.*]` block and that implicit injection is disabled.

## `[debug]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `har_path` | string | `""` (off) | Path to write a HAR-1.2 file capturing every request/response. Empty disables HAR recording. |
| `har_body_cap` | integer or humansize | `65536` (64 KiB) | Maximum response body bytes captured per entry. Accepts raw bytes (`65536`) or humansize strings (`"64KiB"`, `"1MiB"`, `"1GiB"`). |
| `log_level` | string | `"info"` | Default tracing filter. Overridden by `RUST_LOG` when set. |

## Environment overrides

| Env var | Overrides | Notes |
| --- | --- | --- |
| `ROVER_CONFIG` | config file path | Overrides the default config location for every subcommand when no `--config` is passed. |
| `ROVER_DATA_DIR` | data dir (cache db, downloads) | |
| `ROVER_OUTPUT_DIR` | `output.dir` | |
| `ROVER_HTTP_BIND` | `http.bind` | Only consulted by `rover mcp --http`; overridden in turn by `--bind`. |
| `ROVER_LOG_LEVEL` | `debug.log_level` | |
| `RUST_LOG` | tracing filter | Takes precedence over `debug.log_level`. |

`rover config show` annotates every leaf with its effective source: `defaults`, `file`, or `env`. Only the 45 leaves listed by `provenance::known_leaves()` are tracked, and coverage varies by section rather than being all-or-nothing. Two dynamic sections — the free-form `[backends.<name>]` and `[captioners.<name>]` maps — are entirely untracked, and so is all of `[prompt_injection]`. A few other sections track their primary knobs but omit some secondary ones: `cache` omits `stale_while_revalidate_window` and `override_no_store_domains`; `rate_limit` omits `initial_backoff`, `max_backoff`, `retry_after_ceiling`, `jitter_seed`, and `deferred_retry_threshold_secs`; `robots` omits `ignore_domains`. `headless` is the sparsest: it tracks only 2 of its 12 keys (`max_concurrent`, `chrome_executable`) and omits the other 10 — `auto_detect_spa`, `default_wait`, `timeout_secs`, `launch_delay_secs`, and all six `block_*` toggles (`block_images`, `block_fonts`, `block_media`, `block_css`, `block_third_party`, `block_service_workers`). Every other section above — `fetch`, `ssrf`, `tokenizer`, `mcp`, `http`, `output`, `summarization` (including `summarization.tables`), `image_captions` (including `image_captions.cache`), and `debug` — is tracked in full.

## Worked example

```toml
[fetch]
timeout_secs = 30

[ssrf]
level = "project"
project_root = "/Users/me/code"

[prompt_injection]
level = "moderate"
model = "disabled"          # set to e.g. "deberta-small" with --features injection-model

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

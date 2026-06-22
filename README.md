# Rover

**An MCP server that turns the web into clean, token-efficient Markdown for LLM agents.**

Rover sits between your agent and the open web. Give it a URL — it fetches, extracts the meaningful content, strips the chrome, normalises the markup, counts tokens, optionally summarises, and hands back a YAML-frontmattered Markdown document your agent can actually reason about. The same binary runs as a long-lived MCP server (for Claude Code and other agent harnesses) and as a one-shot CLI.

```sh
$ rover fetch https://en.wikipedia.org/wiki/Rust_(programming_language)
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
fetched_at: "2026-05-26T12:34:56Z"
content_hash: "sha256:b3e9…"
estimated_tokens: 14823
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language…
```

[Install](#install) · [Quick start](#quick-start) · [Use as an MCP server](#use-as-an-mcp-server) · [Features](#features) · [Configuration](#configuration) · [Documentation](#documentation)

---

## Why Rover

Agents that browse the web on the fly hit the same three walls every time:

- **Boilerplate, ads, and chrome** drown the actual content. Token budgets vanish into navigation menus.
- **JavaScript-rendered pages** return empty `<div id="root">` to anything that isn't a browser.
- **Repeated fetches** to the same URL waste tokens, time, and money — and ignore politeness rules (rate limits, `robots.txt`, server-side caching headers).

Rover fixes all three. The extraction layer is the battle-tested [`readabilityrs`](https://crates.io/crates/readabilityrs) crate (handles Prism/Shiki/rehype/WordPress/GitHub code blocks, MathJax/KaTeX, footnote dialects, lazy-loaded images, permalink anchors). On top of that, Rover adds caching with proper TTL handling, per-domain rate limiting, `robots.txt` honouring, charset detection, configurable SSRF protection, optional headless rendering for SPAs, extractive *and* cloud-LLM summarisation, image captioning, and a long-running task model with NDJSON-streamed progress.

> [!NOTE]
> Rover is built for single-user-local deployment — one MCP server alongside your IDE/agent, not a multi-tenant gateway. Distribute it as a binary, point your agent at it, get on with your work.

## Install

Pick whichever channel fits. All of them install a binary named `rover`.

**Homebrew:**

```sh
brew install aaronbassett/tap/rover
```

The `rover` formula ships the JavaScript-rendering (`headless`) build and
`depends_on "chromium"`. Other optional features are available from source via
`cargo install` (see the crates.io option below).

**Prebuilt binary (Linux & macOS):**

One-line installer (downloads the right tarball for your platform and installs `rover`):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/aaronbassett/rover/releases/latest/download/rover-fetch-installer.sh | sh
```

Or download a `.tar.xz` for your platform from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify its checksum, then extract it and move the `rover` binary onto your `PATH`:

```sh
tar xf rover-fetch-<target>.tar.xz   # then move the extracted `rover` onto your PATH
```

Targets: `x86_64`/`aarch64` Linux (gnu) and Intel/Apple-Silicon macOS. The prebuilt binary includes the `headless` feature (JavaScript-rendered pages).

**crates.io:**

```sh
cargo install rover-fetch --features headless   # crate is rover-fetch; binary is rover
```

> [!NOTE]
> The crate is published as `rover-fetch` because the `rover` name on crates.io is held by an unrelated project. The installed binary is still `rover`. `cargo install` builds from source with the crate's default (basic) features; add `--features headless` to match the prebuilt and Homebrew binary.

**From source (for hacking):**

```sh
git clone https://github.com/aaronbassett/rover
cd rover
cargo build --release
# binary lands at target/release/rover
```

The default build (~28 MiB) needs no model downloads, no Chrome, and no extra
runtime dependencies.

**Requirements:** Rust 1.96+ (edition 2024). See [`docs/versioning.md`](docs/versioning.md) for the stability and MSRV policy.

## Quick start

Fetch a page and print clean Markdown to stdout:

```sh
rover fetch https://example.com/article
```

Fetch and cap the output at 4,000 tokens (Rover summarises automatically when the extracted Markdown exceeds the budget):

```sh
rover fetch --max-tokens 4000 https://example.com/long-article
```

Inspect the cache:

```sh
rover cache stats         # entry count, total size, expired count
rover cache list          # paginated URL listing
rover cache get <url>     # print the cached Markdown for a URL
```

Sanity-check your installation:

```sh
rover doctor              # SQLite, schema, network, config, backends, output dir
```

> [!TIP]
> Run `rover --help` for the full subcommand surface. Every subcommand also supports `--help` for its specific options.

## Use as an MCP server

The CLI is convenient, but the canonical surface is the MCP server. Wire Rover into Claude Code (or any MCP-speaking agent harness) so the model can call it directly:

```sh
rover mcp
```

`rover mcp` is a long-running stdio MCP server. Register it with your agent — for Claude Code:

```sh
claude mcp add rover -- rover mcp
```

…or, for any MCP client that takes a JSON config, the standard shape is:

```json
{
  "mcpServers": {
    "rover": {
      "command": "rover",
      "args": ["mcp"]
    }
  }
}
```

The model now has these tools available:

| Tool | What it does |
| --- | --- |
| `fetch` | Single URL → cleaned Markdown. Supports caching, headless rendering, image modes, token budgeting, inline summarisation. |
| `batch_fetch` | Fetch N URLs concurrently with per-domain rate limiting. Returns a `task_id`; stream progress with `rover batch <id> --monitor`. |
| `summarize` | Compact a cached or fresh page using extractive (offline) or cloud (genai) backends. Steerable via `focus`, `preserve`, `target_tokens`. |
| `get_metadata` | Extract Schema.org, Open Graph, and Twitter Card metadata without pulling the full body. |
| `count_tokens` | Estimate the token cost of a URL across cl100k / o200k / claude / llama3 / qwen3 tokenisers without paying it. |

Full tool reference: [`docs/mcp-tools.md`](docs/mcp-tools.md).

## Features

### Output that respects your token budget

Every fetch returns YAML-frontmattered Markdown with cache provenance, content hash, and token estimates. Pass `max_tokens` (MCP) or `--max-tokens` (CLI) and Rover summarises to fit. Pass `count_only` / `--count-only` and Rover skips the body entirely and returns just the estimate.

### Caching, with care

A single SQLite database (WAL mode) backs the cache, task state, and event log. Cache decisions honour `Cache-Control`, `Expires`, `ETag`, `Last-Modified`, and stale-while-revalidate semantics; the cache also stores extracted Markdown, raw HTML (optional), and per-page metadata.

```sh
rover cache list
rover cache get <url>
rover cache purge 'https://example.com/*'
rover cache stats
rover fetch --force-refresh <url>   # bypass cache for this request
```

Cache location: `$XDG_DATA_HOME/rover/rover.db` (or `~/.local/share/rover/rover.db`). Override with `ROVER_DATA_DIR`.

### Background tasks with streaming progress

`batch_fetch` (MCP) and `rover batch <id>` / `rover task <id>` (CLI) schedule long-running work and stream NDJSON events you can pipe into the Monitor pattern:

```sh
rover batch <id> --monitor                       # live: item_started, item_done, …, task_completed
rover task <id>                                  # snapshot: progress, ETA, last event
rover task <id> --cancel                         # cooperative cancellation
rover batch <id> --format=ndjson                 # single JSON line, scripting-friendly
rover task <id> --monitor --from-event <id>      # resume an interrupted stream
```

Tasks survive `rover mcp` restarts. Batch jobs resume from persisted progress; summarisation jobs mark as `failed` with a clear reason so the agent can re-request.

### Summarisation

Two backends ship by default — and you can configure as many cloud backends as you want, each addressable by name:

```toml
[summarization]
default_backend = "default"
fallback_to_extractive = true

[backends.default]
kind = "extractive"          # offline TextRank; no API key, no network

[backends.fast]
kind = "cloud"
provider = "openai"          # openai, anthropic, gemini, openai_compat
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
```

`openai_compat` covers LM Studio, Ollama, vLLM, and anything else that speaks the OpenAI chat-completions dialect. Steering parameters (`focus`, `preserve`, `target_tokens`, `style`) work uniformly across all backends.

When a cloud backend fails (auth, rate limit, network), Rover transparently falls back to the extractive backend and tags the response with `summarizer_fallback: { from, reason }`. Set `fallback_to_extractive = false` for strict-error mode.

### Per-domain rate limiting & `robots.txt`

A per-host token bucket plus a global concurrency cap plus a respected `Crawl-Delay` floor — all configurable. The robots cache fails closed (cached `disallow_all` sentinel for the configured `failure_ttl`) so a flaky robots endpoint doesn't quietly let traffic through.

### Configurable SSRF protection

Five levels: `strict` · `loopback` · `project` · `lan` · `none`. Every outbound URL is validated twice — once by parsed scheme/host, once against every resolved address before the connection is opened — and a **dial-time SSRF resolver** re-applies the same policy at every connection attempt, closing the DNS-rebinding TOCTOU window for both the initial request and every redirect hop.

See [`docs/security.md`](docs/security.md) for the full level matrix, the always-blocked address floor, `file://` handling under `project`, and the documented residual limitations.

### HAR debug recording

Set `[debug] har_path` in `rover.toml` and every round-trip lands in a HAR file that imports cleanly into Chrome DevTools' Network panel:

```toml
[debug]
har_path = "./rover-debug.har"
har_body_cap = "64KiB"
```

Sub-requests (CSS, fonts, beacons) are deliberately excluded so the HAR file stays focused on what Rover actually returned.

### Optional features (Cargo feature flags)

Three independent features for users who want more than the default:

| Feature | Adds | Approx. size |
| --- | --- | --- |
| `local-inference` | Local LLM summarisation via [`mistral.rs`](https://github.com/EricLBuehler/mistral.rs) (default model: Qwen 3.5 0.8B) | ~80 MB |
| `local-vision` | Local image captioning via SmolVLM (shares `mistral.rs` with `local-inference`) | ~5 MB additional |
| `headless` | JavaScript-rendered SPA support via [`chromiumoxide`](https://github.com/mattsse/chromiumoxide) (uses system Chrome) | ~32 MB |

Install any combination from source via crates.io (the prebuilt binary and
Homebrew formula ship only the `headless` build):

```sh
cargo install rover-fetch --features local-inference
cargo install rover-fetch --features headless
cargo install rover-fetch --features local-inference,local-vision,headless
```

Local models are downloaded on first use (or ahead of time with `rover model download <repo_id>`) and live under `$HF_HOME/hub`. Manage the cache with `rover model {list,download,remove}`.

> [!WARNING]
> Cloud captioners (OpenAI, Anthropic, Gemini, OpenAI-compatible) are **always compiled in** — they don't need any feature flag. The `local-vision` feature only adds the option of a fully-offline captioner.

> [!IMPORTANT]
> The `headless` feature needs a Chrome/Chromium browser on the host. Rover auto-detects standard install paths on Linux/macOS/Windows; override with `[headless] chrome_executable`. `rover doctor` verifies the launch path.

Setup details, model recommendations, memory profiles, and binary-size matrix: [`docs/features.md`](docs/features.md).

## Configuration

Rover reads `rover.toml` from `$XDG_CONFIG_HOME/rover/rover.toml` (or `~/.config/rover/rover.toml`). Override the location with `ROVER_CONFIG`. Every key has a sensible default — the config file is optional.

Inspect the merged effective configuration with per-key provenance:

```sh
rover config show
```

Mutate values in place (comments preserved via `toml_edit`, round-trip validated):

```sh
rover config set ssrf.level loopback
rover config set cache.default_ttl 3600
rover config set summarization.default_backend fast
```

A minimal `rover.toml`:

```toml
[fetch]
user_agent = "my-agent/1.0"
timeout_secs = 30

[ssrf]
level = "strict"

[cache]
default_ttl = "1h"
max_ttl = "7d"

[rate_limit]
requests_per_minute_per_domain = 30
per_domain_concurrency = 2
global_concurrency = 8

[robots]
respect = true
failure_ttl = "5m"

[summarization]
default_backend = "default"
fallback_to_extractive = true

[backends.default]
kind = "extractive"
```

The full reference — every section, every key, every default — lives in [`docs/configuration.md`](docs/configuration.md).

## Documentation

| Doc | What's in it |
| --- | --- |
| [`docs/cli.md`](docs/cli.md) | Every subcommand, every flag, exit codes, NDJSON event shapes. |
| [`docs/mcp-tools.md`](docs/mcp-tools.md) | MCP tool schemas: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens`. |
| [`docs/configuration.md`](docs/configuration.md) | Every config section and key, with defaults, types, and worked examples. |
| [`docs/backends.md`](docs/backends.md) | Summarisation backend reference: extractive (TextRank) and cloud (genai) providers. |
| [`docs/features.md`](docs/features.md) | Cargo feature flags: `local-inference`, `local-vision`, `headless` — setup, models, sizes. |
| [`docs/security.md`](docs/security.md) | SSRF levels, address floor, DNS rebinding mitigation, secret redaction, cache poisoning, known limitations. |

## Subcommands at a glance

```text
rover fetch <url>                    one-shot fetch → Markdown on stdout
rover mcp                            long-running MCP server (stdio)
rover cache list|get|purge|stats     inspect / manage the local cache
rover batch <id>                     batch status; --monitor streams events
rover task <id>                      task status (any kind); --cancel, --monitor
rover doctor                         health checks; --format=ndjson for scripting
rover config show                    print merged config + per-key provenance
rover config set <key> <value>       mutate a config key in place
rover model download|list|remove     manage local model cache (feature-gated)
```

Full reference: [`docs/cli.md`](docs/cli.md).

## Security & privacy

Rover defaults to **strict SSRF** (public IPs only, `http`/`https` only) and a conservative rate-limit profile. The tracing layer scrubs both URL query-string secrets (`api_key`, `token`, `secret`, `password`) and HTTP `Authorization`-style credentials (`Bearer …` / `Basic …`, plus any field literally named `authorization`) before events hit any log destination.

> [!CAUTION]
> The HAR recorder, when enabled via `[debug] har_path`, writes request and response bodies to disk *unredacted by design* — HAR is opt-in debug instrumentation for inspecting raw traffic. Protect the HAR file with filesystem permissions and treat it as sensitive material. Full threat model: [`docs/security.md`](docs/security.md).

## License

MIT or Apache-2.0, at your option.

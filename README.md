<div align="center">

<img src="docs/assets/rover-hero.png" alt="Rover — turn the web into clean, token-efficient Markdown your agent can trust" width="100%">

# Rover

**An MCP server that turns the web into clean, token-efficient Markdown your LLM agent can actually trust.**

[![CI](https://github.com/aaronbassett/rover/actions/workflows/ci.yml/badge.svg)](https://github.com/aaronbassett/rover/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 1.96+](https://img.shields.io/badge/rustc-1.96+-orange.svg)](docs/versioning.md)
[![Status: alpha](https://img.shields.io/badge/status-alpha-yellow.svg)](#install)

[Quick start](#quick-start-wire-it-into-your-agent) · [Why Rover](#why-rover) · [How it compares](#how-your-agent-gets-the-web) · [MCP tools](#the-mcp-tools) · [Security](#security--trust) · [Features](#features) · [Docs](#documentation)

</div>

---

Point your agent at a URL and Rover fetches it, strips the ads/nav/chrome, extracts the real content, normalises the markup, counts the tokens, optionally summarises to a budget, and hands back a YAML-frontmattered Markdown document — wrapped so the model knows it's **untrusted third-party data, not instructions**. The same binary runs as a long-lived **MCP server** for Claude Code and other agent harnesses, and as a one-shot **CLI**.

<div align="center">

<img src="docs/assets/rover-demo.gif" alt="rover fetching a Wikipedia page and summarising it to a token budget" width="85%">

<sub><code>rover</code> fetching the Charlie Dog (a.k.a. <strong>Rover</strong> 🐕) page and summarising ~19.6k tokens down to ~330 — summarisation here runs through a configured cloud backend.</sub>

</div>

> [!NOTE]
> Rover is built for single-user-local deployment — one MCP server alongside your IDE/agent, not a multi-tenant gateway. Ship it as a binary, point your agent at it, get on with your work.

## Why Rover

Agents that browse the live web hit the same four walls every time:

- **🧹 Boilerplate, ads, and chrome drown the content.** Token budgets vanish into navigation menus and cookie banners.
- **🖼️ JavaScript-rendered pages return an empty `<div id="root">`** to anything that isn't a browser.
- **🔁 Repeated fetches waste tokens, time, and money** — and ignore politeness rules (rate limits, `robots.txt`, caching headers).
- **🛡️ Fetched web content is untrusted.** A page can carry "ignore your instructions and…" straight into your agent's context. Most fetch tools hand it over raw.

Rover fixes all four. Extraction is the battle-tested [`readabilityrs`](https://crates.io/crates/readabilityrs) crate (Prism/Shiki/rehype/WordPress/GitHub code blocks, MathJax/KaTeX, footnote dialects, lazy-loaded images, permalink anchors). On top of that Rover layers HTTP-aware caching, per-domain rate limiting + `robots.txt`, charset detection, configurable SSRF protection, a layered **prompt-injection guard**, optional headless rendering for SPAs, extractive *and* cloud-LLM summarisation, inline image captioning, and a long-running task model with NDJSON-streamed progress.

## How your agent gets the web

|  | **Rover** | **Claude Code `WebFetch`** | **`wget`** |
|---|---|---|---|
| What your agent gets back | Clean Markdown **document** + frontmatter, content hash, token count | A fast model's **answer** about the page (lossy, per-prompt) | Raw HTML / bytes |
| Strips nav/ads/chrome → Markdown | ✅ readability extraction | ✅ HTML→MD (non-optional) | ❌ |
| Reusable across calls (re-read, no re-run) | ✅ cached doc, stable hash | ❌ re-runs the model each prompt | ✅ (raw file) |
| Token budgeting & counts | ✅ estimate · `max_tokens` · summarise-to-fit · count-only | ❌ fixed truncation, no control | ❌ |
| HTTP-aware caching | ✅ TTL · ETag · Last-Modified · stale-while-revalidate | ◻️ flat 15-min cache | ◻️ timestamping (`-N`) only |
| JavaScript / SPA rendering | ◻️ optional (`headless` feature) | ❌ | ❌ |
| Batch fetch + per-domain rate limiting | ✅ `batch_fetch`, token-bucket, streaming progress | ❌ one URL per call | ◻️ recursive, no per-domain limit |
| SSRF / private-network protection | ✅ 5 levels + dial-time re-check (anti-DNS-rebinding) | ◻️ HTTP→HTTPS upgrade; private-IP stance undocumented | ❌ |
| Prompt-injection guard | ✅ layered: nonce wrapper + detectors + optional model | ❌ content goes straight to the model | — |
| Structured metadata (schema.org / OG / Twitter) | ✅ `get_metadata` | ❌ (must ask in the prompt) | ❌ |
| Inline image captioning | ✅ cloud VLMs (OpenAI / Anthropic / Gemini / compatible) | ❌ | ❌ |
| Works offline / no per-fetch API cost | ✅ extractive backend, no API key | ❌ model call per fetch | ✅ |

<sub>✅ full · ◻️ partial/optional · ❌ no · — n/a · `WebFetch` column per the [official Claude Code docs](https://docs.claude.com/en/docs/claude-code).</sub>

> **Rover isn't a web crawler.** To recursively mirror or crawl an entire site, reach for `wget` or `httrack` — Rover fetches and preps *individual* pages for an agent to reason over, not bulk downloads.

## Quick start: wire it into your agent

The canonical surface is the MCP server. Add it to **Claude Code** in one command:

```sh
claude mcp add rover -- rover mcp
```

For any other MCP client that takes a JSON config, the standard shape is:

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

Your agent now has these tools:

| Tool | What it does |
| --- | --- |
| `fetch` | Single URL → cleaned Markdown. Caching, headless rendering, image modes, token budgeting, inline summarisation. |
| `batch_fetch` | Fetch N URLs concurrently with per-domain rate limiting. Returns a `task_id`; stream progress with `rover batch <id> --monitor`. |
| `summarize` | Compact a cached or fresh page via extractive (offline) or cloud backends. Steerable with `focus`, `preserve`, `target_tokens`. |
| `get_metadata` | Extract Schema.org, Open Graph, and Twitter Card metadata without pulling the full body. |
| `count_tokens` | Estimate a URL's token cost across `cl100k` / `o200k` / `claude` / `llama3` / `qwen3` tokenisers without paying it. |

Full tool reference: [`docs/mcp-tools.md`](docs/mcp-tools.md).

### …or use it from the shell

Every capability is also a one-shot CLI command — handy for scripts, CI, and trying things out:

```sh
rover fetch https://example.com/article            # clean Markdown → stdout
rover fetch --max-tokens 4000 https://example.com  # summarise to fit a budget
rover cache stats                                  # entry count, size, expired
rover doctor                                       # sanity-check the install
```

> [!TIP]
> `rover --help` prints the full subcommand surface; every subcommand has its own `--help`.

## Install

> [!NOTE]
> Rover is pre-1.0 (`0.1.0-alpha.1`). The build-from-source path below works today; the packaged channels (Homebrew tap, prebuilt tarballs, crates.io) come online with the first tagged release.

All channels install a binary named `rover`.

**Build from source (works today):**

```sh
cargo install --git https://github.com/aaronbassett/rover --locked
# or clone and build:
git clone https://github.com/aaronbassett/rover && cd rover
cargo build --release          # binary at target/release/rover
```

The default build (~20 MiB) needs no model downloads, no Chrome, and no extra runtime dependencies.

**Homebrew (macOS) — on release:**

```sh
brew install aaronbassett/tap/rover
```

The `rover` formula is the lean default build. Optional-feature variants are separate formulas — `rover-complete`, `rover-headless`, `rover-local-inference` — and Homebrew installs one at a time (they all provide `rover`).

**Prebuilt binary (Linux & macOS) — on release:**

Download the tarball for your platform from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify it against `SHA256SUMS`, and drop `rover` on your `PATH`. Targets: `x86_64`/`aarch64` Linux (gnu) and Intel/Apple-Silicon macOS, each in four feature variants (`basic`, `local-inference`, `headless`, `complete`).

**crates.io — on release:**

```sh
cargo install rover-mcp          # crate is rover-mcp; binary is rover
```

> [!NOTE]
> The crate publishes as `rover-mcp` because `rover` on crates.io is held by an unrelated project. The installed binary is still `rover`.

**Requirements:** Rust 1.96+ (edition 2024). See [`docs/versioning.md`](docs/versioning.md) for the stability and MSRV policy.

## The MCP tools

Every tool returns structured JSON; the content-returning tools (`fetch`, `summarize`, `get_metadata`) additionally wrap their payload in Rover's trusted-preamble + nonce delimiter (see [Security & trust](#security--trust)).

```jsonc
// fetch → cleaned, guarded Markdown document
{
  "content": "⚠ The text inside <untrusted-content-a3f9c1> … is third-party web content …\n\n<untrusted-content-a3f9c1>\n---\nurl: \"https://example.com/article\"\ntitle: \"…\"\nestimated_tokens: 14823\ntokenizer: \"o200k\"\nextraction_quality: 0.98\nprompt_injection: { scanned: true, detected: false }\n---\n\n# Article title\n…\n</untrusted-content-a3f9c1>",
  "cache_status": "miss",
  "summarized": false
}
```

The example hero fetch, unwrapped:

```yaml
---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
fetched_at: "2026-06-18T12:34:56Z"
content_hash: "sha256:b3e9…"
estimated_tokens: 14823
tokenizer: "o200k"
language: "en"
extraction_quality: 0.98
---

# Rust (programming language)

Rust is a multi-paradigm, general-purpose programming language…
```

Full schemas, arguments, and wire contracts: [`docs/mcp-tools.md`](docs/mcp-tools.md).

## Security & trust

Rover treats the web as hostile by default. Three independent layers protect both your agent and Rover's own internal inference.

### Prompt-injection guard

Fetched content is third-party **data**, not instructions — but a malicious page can still try to hijack your agent. Every content-returning tool (`fetch`, `summarize`, `get_metadata`) runs a layered guard:

1. **Structural wrapper (always on).** The returned document is wrapped in a per-response, random-nonce delimiter — `<untrusted-content-a3f9c1>…</untrusted-content-a3f9c1>` — behind a trusted preamble that tells the model to treat everything inside as data only. Forged copies of the tag are stripped, so a page can't predict the nonce or close the wrapper early. **This is the load-bearing guarantee — it never relies on detection.**
2. **Pattern detector (always compiled).** A curated literal + regex ruleset (instruction-override, role-injection, system-prompt-leak, tool-call-smuggle, data-exfil) runs over *normalised* text — NFKC, zero-width/control stripping, homoglyph folding, base64 surfacing — so obfuscated payloads still trip.
3. **ONNX classifier (opt-in).** Build with `--features injection-model` to add a DeBERTa prompt-injection model (downloaded on first use) for novel phrasings the rules don't enumerate.

A configurable response level decides what happens on a hit:

| Level | Action |
| --- | --- |
| `strict` | Drop the body; return the warning only |
| `high` | Remove the matched spans / windows |
| `moderate` *(default)* | Quarantine matched spans in `<DANGER>…</DANGER>` + warn |
| `low` | Content intact; warn only |
| `disabled` | No detection (the wrapper still applies) |

Structured `prompt_injection` telemetry rides along on every response, and content Rover feeds to its **own** summariser/caption models is always independently cleaned at high strength — that hardening can't be disabled. Configure under [`[prompt_injection]`](docs/configuration.md#prompt_injection); full contract in [`docs/mcp-tools.md`](docs/mcp-tools.md#prompt-injection-guard).

### SSRF protection

Five levels: `strict` · `loopback` · `project` · `lan` · `none`. Every outbound URL is validated twice — once by parsed scheme/host, once against every resolved address before the socket opens — and a **dial-time SSRF resolver** re-applies the policy at each connection attempt, closing the DNS-rebinding TOCTOU window for both the initial request and every redirect hop. Default is `strict` (public IPs, `http`/`https` only). Full level matrix, the always-blocked address floor, and `file://` handling: [`docs/security.md`](docs/security.md).

### Secret redaction

The tracing layer scrubs URL query-string secrets (`api_key`, `token`, `secret`, `password`) **and** HTTP `Authorization`-style credentials (`Bearer …` / `Basic …`, plus any field literally named `authorization`) before events reach any log destination.

> [!CAUTION]
> The HAR recorder (`[debug] har_path`) writes request/response bodies to disk **unredacted by design** — it's opt-in debug instrumentation. Protect the file with filesystem permissions and treat it as sensitive. Full threat model: [`docs/security.md`](docs/security.md).

## Features

### Output that respects your token budget

Every fetch returns YAML-frontmattered Markdown with cache provenance, content hash, language, extraction-quality score, and a token estimate. Pass `max_tokens` (MCP) / `--max-tokens` (CLI) and Rover summarises to fit — the body is replaced with a budget-sized summary and the frontmatter gains `summarized: true`. The MCP `fetch` `count_only` arg (and the standalone `count_tokens` tool) returns just the estimate without the body. Token counts span five tokenisers (`cl100k`, `o200k`, `claude`, `llama3`, `qwen3`; default `o200k`).

### Caching, with care

A single SQLite database (WAL mode) backs the cache, task state, and event log. Cache decisions honour `Cache-Control`, `Expires`, `ETag`, `Last-Modified`, and stale-while-revalidate. The default TTL is **15 minutes** — deliberately short, so content that's been poisoned or quietly changed has a small blast radius before the next revalidation.

```sh
rover cache list
rover cache get <url>
rover cache purge 'https://example.com/*'
rover cache stats
rover fetch --force-refresh <url>   # bypass cache for this request
```

Cache location: `$XDG_DATA_HOME/rover/rover.db` (or `~/.local/share/rover/rover.db`). Override with `ROVER_DATA_DIR`.

### Background tasks with streaming progress

`batch_fetch` (MCP) and `rover batch <id>` / `rover task <id>` (CLI) schedule long-running work and stream NDJSON events:

```sh
rover batch <id> --monitor                       # live: item_started, item_done, …, task_completed
rover task <id>                                  # snapshot: progress, ETA, last event
rover task <id> --cancel                         # cooperative cancellation
rover batch <id> --format=ndjson                 # single JSON line, scripting-friendly
rover task <id> --monitor --from-event <id>      # resume an interrupted stream
```

Tasks survive `rover mcp` restarts: batch jobs resume from persisted progress; summarisation jobs mark `failed` with a clear reason so the agent can re-request.

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

`openai_compat` covers LM Studio, Ollama, vLLM, and anything else speaking the OpenAI chat-completions dialect. Steering parameters (`focus`, `preserve`, `target_tokens`, `style`) work uniformly across backends. When a cloud backend fails (auth, rate limit, network), Rover transparently falls back to extractive and tags the response with `summarizer_fallback: { from, reason }` — set `fallback_to_extractive = false` for strict-error mode.

### Inline image captioning

Set `images: caption` (MCP) and Rover replaces images with model-written alt-text inline in the Markdown. Captioning uses cloud vision models and is **always compiled in — no feature flag**:

```toml
[image_captions]
default = "openai"
max_per_page = 5

[captioners.openai]
provider = "openai"           # openai, anthropic, gemini, openai_compat
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
```

`openai_compat` works here too — point it at a local Ollama or LM Studio vision server (e.g. `llama3.2-vision`) for fully offline captioning with no API key.

### Per-domain rate limiting & `robots.txt`

A per-host token bucket, a global concurrency cap, and a respected `Crawl-Delay` floor — all configurable. The robots cache fails closed (a cached `disallow_all` sentinel for the configured `failure_ttl`), so a flaky robots endpoint doesn't quietly let traffic through.

### HAR debug recording

Set `[debug] har_path` and every round-trip lands in a HAR file that imports cleanly into Chrome DevTools' Network panel. Sub-requests (CSS, fonts, beacons) are excluded so the file stays focused on what Rover actually returned.

```toml
[debug]
har_path = "./rover-debug.har"
har_body_cap = "64KiB"
```

### Optional features (Cargo feature flags)

| Feature | Adds | Notes |
| --- | --- | --- |
| `headless` | JavaScript-rendered SPA support via [`chromiumoxide`](https://github.com/mattsse/chromiumoxide) | Uses system Chrome/Chromium (~32 MB) |
| `local-inference` | Local LLM summarisation via [`mistral.rs`](https://github.com/EricLBuehler/mistral.rs) (default model: Qwen 3.5 0.8B) | ~80 MB; model downloaded on first use |
| `injection-model` | ONNX DeBERTa prompt-injection classifier (guard method 3) | Native ONNX runtime; ~200 MB model downloaded on first use |

```sh
cargo build --release --features headless
cargo build --release --features local-inference,headless
cargo build --release --features injection-model
```

Local models download on first use (or ahead of time via `rover model download <repo_id>`) and live under `$HF_HOME/hub`; manage them with `rover model {list,download,remove}`.

> [!IMPORTANT]
> Cloud captioners (OpenAI, Anthropic, Gemini, OpenAI-compatible) are **always compiled in** — no feature flag. The `headless` feature needs a Chrome/Chromium browser on the host; Rover auto-detects standard install paths (override with `[headless] chrome_executable`), and `rover doctor` verifies the launch path.

Setup details, model recommendations, and memory profiles: [`docs/features.md`](docs/features.md).

## Configuration

Rover reads `rover.toml` from `$XDG_CONFIG_HOME/rover/rover.toml` (or `~/.config/rover/rover.toml`); override with `ROVER_CONFIG`. Every key has a sensible default — the file is optional.

```sh
rover config show                          # merged effective config + per-key provenance
rover config set ssrf.level loopback       # mutate in place (comments preserved, round-trip validated)
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
default_ttl = "15m"          # default; raise per-origin Cache-Control still wins
max_ttl = "7d"

[rate_limit]
requests_per_minute_per_domain = 30
per_domain_concurrency = 2
global_concurrency = 8

[summarization]
default_backend = "default"

[backends.default]
kind = "extractive"
```

The full reference — every section, key, and default — lives in [`docs/configuration.md`](docs/configuration.md).

## Subcommands at a glance

```text
rover fetch <url>                    one-shot fetch → Markdown on stdout
rover mcp                            long-running MCP server (stdio)
rover cache list|get|purge|stats     inspect / manage the local cache
rover batch <id>                     batch status; --monitor streams events
rover task <id>                      task status (any kind); --cancel, --monitor
rover doctor                         health checks; --format=ndjson for scripting
rover config show|set                inspect / mutate config (provenance-aware)
rover model download|list|remove     manage local model cache (feature-gated)
```

Full reference, exit codes, and NDJSON event shapes: [`docs/cli.md`](docs/cli.md).

## Documentation

| Doc | What's in it |
| --- | --- |
| [`docs/cli.md`](docs/cli.md) | Every subcommand, flag, exit code, and NDJSON event shape. |
| [`docs/mcp-tools.md`](docs/mcp-tools.md) | MCP tool schemas: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, `count_tokens`, and the prompt-injection wire contract. |
| [`docs/configuration.md`](docs/configuration.md) | Every config section and key, with defaults, types, and examples. |
| [`docs/backends.md`](docs/backends.md) | Summarisation backend reference: extractive (TextRank) and cloud providers. |
| [`docs/features.md`](docs/features.md) | Cargo feature flags: `headless`, `local-inference`, `injection-model` — setup, models, sizes. |
| [`docs/security.md`](docs/security.md) | SSRF levels, address floor, DNS-rebinding mitigation, secret redaction, prompt-injection guard, known limitations. |
| [`docs/versioning.md`](docs/versioning.md) | Stability and MSRV policy. |

Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md) · Security policy: [`SECURITY.md`](SECURITY.md) · Changelog: [`CHANGELOG.md`](CHANGELOG.md).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

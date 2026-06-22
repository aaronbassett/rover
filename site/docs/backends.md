---
id: backends
title: Backends
---

# Rover Summarizer Backends

Rover's summarizer is pluggable. Backends are declared as `[backends.<name>]` blocks in the config; the active default is selected by `[summarization] default_backend`. The MCP `summarize` tool (and the inline `summarize` sub-arg on `fetch`) accepts a per-call `backend` override.

Two kinds are supported:

- **`extractive`** — pure-Rust, CPU-only, offline. Selects sentences from the source via the TextRank-flavoured implementation in `summarizer::extractive`. Always available; no network, no API key.
- **`cloud`** — calls out to a hosted LLM via the [`genai`](https://crates.io/crates/genai) crate.

## Implicit default

When the `[backends]` map is empty, Rover installs an implicit `default` extractive backend. This means a fresh install works offline with zero configuration. Adding *any* explicit `[backends.*]` block disables that implicit injection — from that point on, validation rules apply strictly:

1. `[summarization] default_backend` must name an existing `[backends.*]` entry.
2. If `[summarization] fallback_to_extractive = true` (the default), at least one extractive backend must exist.

## Cloud providers

`provider` strings recognized by the cloud kind:

| `provider` | Notes |
| --- | --- |
| `openai` | Native OpenAI. Default env: `OPENAI_API_KEY`. |
| `anthropic` | Native Anthropic. Default env: `ANTHROPIC_API_KEY`. |
| `gemini` | Google Gemini. Default env: `GEMINI_API_KEY`. |
| `xai` | xAI Grok. Default env: `XAI_API_KEY`. |
| `groq` | Groq. Default env: `GROQ_API_KEY`. |
| `deepseek` | DeepSeek. Default env: `DEEPSEEK_API_KEY`. |
| `together` | Together AI. Default env: `TOGETHER_API_KEY`. |
| `fireworks` | Fireworks AI. Default env: `FIREWORKS_API_KEY`. |
| `openai_compat` | Any endpoint speaking the OpenAI Chat Completions shape. Requires `base_url`. |

For native providers, leave `api_key_env` unset to use `genai`'s default env-var resolution. For `openai_compat`, `api_key_env` is optional — Rover supplies a `"noop"` key to the client if none is configured (most local servers accept any key).

## `[backends.<name>]` fields

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `extractive` or `cloud`. |
| `provider` | string | yes for `cloud` | One of the provider strings above. |
| `model` | string | yes for `cloud` | Literal model id passed through to the provider (e.g. `gpt-4o-mini`, `claude-haiku-4-5`). |
| `base_url` | string | yes for `openai_compat` only | Custom endpoint URL. Ignored for native providers. |
| `api_key_env` | string | no | Env var holding the API key. When unset, see provider defaults above. |

### `openai_compat` base URL normalization

Rover auto-normalizes the `base_url` for `openai_compat` to ensure it ends in `/v1/`. This applies to both `[backends.*]` and `[captioners.*]`. The normalization is idempotent; if your URL already ends `/v1/`, it is unchanged.

| Input | Normalized |
| --- | --- |
| `http://localhost:1234` | `http://localhost:1234/v1/` |
| `http://localhost:1234/` | `http://localhost:1234/v1/` |
| `http://localhost:1234/v1` | `http://localhost:1234/v1/` |
| `http://localhost:1234/v1/` | unchanged |
| `https://api.example.com/custom/` | `https://api.example.com/custom/v1/` |
| `https://api.example.com/custom/v1/` | unchanged |

Whitespace around the URL is trimmed before normalization.

## Fallback selection

When `[summarization] fallback_to_extractive = true`, a failing cloud call (auth, rate limit, model error, invalid request) retries against an extractive backend. The chosen fallback is:

1. The backend named `default` if it is extractive.
2. Otherwise, the lexicographically first extractive backend.

The `fetch` / `summarize` responses report this via the `summarizer_fallback` envelope: `{from: "<original backend>", reason: "<stable code>"}`.

## Worked examples

### Offline only (the implicit default)

No config required.

### OpenAI

```toml
[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[backends.default]
kind = "extractive"

[summarization]
default_backend = "fast"
fallback_to_extractive = true
```

### Anthropic

```toml
[backends.claude]
kind = "cloud"
provider = "anthropic"
model = "claude-haiku-4-5"
api_key_env = "ANTHROPIC_API_KEY"

[backends.default]
kind = "extractive"

[summarization]
default_backend = "claude"
```

### LM Studio (local, OpenAI-compatible)

```toml
[backends.lm_studio]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234"        # auto-normalized to /v1/
model = "qwen2.5-0.5b-instruct"

[backends.default]
kind = "extractive"

[summarization]
default_backend = "lm_studio"
```

### Ollama (local, OpenAI-compatible)

```toml
[backends.ollama]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:11434"       # normalized to http://localhost:11434/v1/
model = "llama3.2:1b"

[backends.default]
kind = "extractive"

[summarization]
default_backend = "ollama"
```

### Multi-backend with named alternatives

```toml
[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"

[backends.deep]
kind = "cloud"
provider = "anthropic"
model = "claude-sonnet-4-5"

[backends.local]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234"
model = "qwen2.5-0.5b-instruct"

[backends.default]
kind = "extractive"

[summarization]
default_backend = "fast"
fallback_to_extractive = true
```

Per-call override from an MCP client:

```jsonc
{ "url": "https://...", "backend": "deep", "mode": "abstractive", "style": "executive" }
```

## Validating backend setup

```bash
rover doctor                # checks extractive synthesis + every cloud backend authenticates
rover doctor --format ndjson
```

The `extractive_synthesis` and `backends_authenticate` checks run as part of the default battery.

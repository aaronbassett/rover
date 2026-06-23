---
id: backends
title: Summarisation backends
---

# Summarisation backends

Declare backends as `[backends.<name>]` entries; pick the active one with `[summarization] default_backend`. The MCP `summarize` tool and the inline `summarize` sub-arg on `fetch` accept a per-call `backend` override, so a client can switch backends without touching the config. See [Summarising pages](/docs/summarizing) for how modes and styles flow through.

Two kinds are always available, no feature flag required:

- **`extractive`** — offline, CPU-only, pure Rust. Selects sentences from the source using the TextRank-flavoured implementation in `summarizer::extractive`. No network, no API key, no setup.
- **`cloud`** — calls a hosted LLM through the [`genai`](https://crates.io/crates/genai) crate. You bring the provider and the key.

A third kind, **`local`**, runs an LLM on your own machine and requires the `local-inference` Cargo feature; the stock binary doesn't have it. For `kind = "local"`, `model` is a HuggingFace repo id, not a provider model id, and `provider` is ignored. Setup, model sizes, and build flags live in [Optional features](/docs/features).

## The implicit default

**An empty `[backends]` map still gives you a working summariser.** With nothing declared, Rover installs an implicit `default` extractive backend, so a fresh install summarises offline with zero configuration.

Adding any explicit `[backends.*]` block disables that injection. From there the validation rules apply strictly:

1. `[summarization] default_backend` must name an existing `[backends.*]` entry.
2. If `[summarization] fallback_to_extractive = true` — the default — at least one extractive backend must exist.

Declare one `cloud` backend and you've opted out of the fallback unless you re-add an extractive one. Most configs keep a `[backends.default]` extractive block for that reason.

## Cloud providers

The `provider` string picks which hosted API the `cloud` kind talks to. Each native provider resolves its key from a default environment variable; `openai_compat` covers anything that speaks the OpenAI Chat Completions shape.

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

For native providers, leave `api_key_env` unset and `genai` resolves the default env var for you. For `openai_compat`, `api_key_env` is optional — Rover hands the client a `"noop"` key when none is configured, which is what most local servers want anyway.

## `[backends.<name>]` fields

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | string | yes | `extractive`, `cloud`, or `local`. `local` requires the `local-inference` feature. |
| `provider` | string | yes for `cloud` | One of the provider strings above. Ignored for `local`. |
| `model` | string | yes for `cloud` and `local` | For `cloud`, the literal provider model id (e.g. `gpt-4o-mini`, `claude-haiku-4-5`). For `local`, a HuggingFace repo id (e.g. `Qwen/Qwen3.5-0.8B`). |
| `base_url` | string | yes for `openai_compat` only | Custom endpoint URL. Ignored for native providers and for `local`. |
| `api_key_env` | string | no | Env var holding the API key. When unset, see the provider defaults above. |

The `model` split is the easy thing to get wrong. A `cloud` backend wants the string the provider's API expects; a `local` backend wants the repo path Rover passes to HuggingFace to download weights. Cross them and the backend either 404s at the provider or hunts for a repo that doesn't exist.

### `openai_compat` base URL normalisation

Rover normalises the `base_url` for `openai_compat` so it always ends in `/v1/`. This applies to both `[backends.*]` and `[captioners.*]`. The normalisation is idempotent — a URL that already ends `/v1/` is left alone.

| Input | Normalised |
| --- | --- |
| `http://localhost:1234` | `http://localhost:1234/v1/` |
| `http://localhost:1234/` | `http://localhost:1234/v1/` |
| `http://localhost:1234/v1` | `http://localhost:1234/v1/` |
| `http://localhost:1234/v1/` | unchanged |
| `https://api.example.com/custom/` | `https://api.example.com/custom/v1/` |
| `https://api.example.com/custom/v1/` | unchanged |

Whitespace around the URL is trimmed before normalisation, so a stray pasted space won't break the endpoint.

## Fallback selection

**A failing cloud call doesn't have to fail the request.** With `[summarization] fallback_to_extractive = true`, a cloud failure — auth, rate limit, model error, invalid request — retries against an extractive backend. The agent gets a summary, just a cheaper one.

The fallback backend is chosen deterministically:

1. The backend named `default`, if it is extractive.
2. Otherwise, the lexicographically first extractive backend.

The `fetch` and `summarize` responses report the swap through the `summarizer_fallback` envelope: `{from: "<original backend>", reason: "<stable code>"}`. The `reason` code is stable, so a client can branch on it without scraping prose.

## Worked examples

### Offline only (the implicit default)

No config required. The implicit `default` extractive backend handles everything.

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
base_url = "http://localhost:1234"        # auto-normalised to /v1/
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
base_url = "http://localhost:11434"       # normalised to http://localhost:11434/v1/
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

The full set of config keys and where this file lives is documented in [Configuration](/docs/configuration).

## Validating backend setup

Run `rover doctor` to confirm the wiring before an agent depends on it.

```bash
rover doctor                # checks extractive synthesis + every cloud backend authenticates
rover doctor --format ndjson
```

The `extractive_synthesis` and `backends_authenticate` checks run as part of the default battery. Build with `local-inference` and the battery adds the local-model checks, so a misconfigured `local` backend surfaces here rather than on the first agent call.

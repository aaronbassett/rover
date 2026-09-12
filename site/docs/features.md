---
id: features
title: Optional features
---

# Optional features

The default build does fetch-and-extract: no Cargo features, no model weights. Four opt-in features add capability. `web-search` adds web search, `headless` renders JavaScript pages, `local-inference` summarises on-device, and `injection-model` adds the ONNX prompt-injection classifier.

`web-search` is the odd one out: it pulls in no new dependency at all (Rover already has an HTTP client and a JSON parser) and needs nothing at runtime beyond an API key. The flag exists to gate the *surface* — the `search` MCP tool, `rover search`, the Brave client — so a build can deliberately opt out. The other three pull in extra dependencies, and two of them download a model the first time you use them.

## Enabling features

Pass `--features` at install time. The crate is `rover-fetch` (the name `rover` was taken on crates.io); the installed binary is still `rover`.

```sh
cargo install rover-fetch                                     # default build
cargo install rover-fetch --features web-search                # add search
cargo install rover-fetch --features headless,web-search       # the release set
cargo install rover-fetch --features headless,local-inference  # combine any features
```

| Feature | Adds | Needs on first use |
| --- | --- | --- |
| `web-search` | Web search: the `search` MCP tool and the `rover search` subcommand, backed by the Brave Search API | An API key in `BRAVE_SEARCH_API_KEY`. No new dependency, no download, no runtime binary |
| `headless` | JavaScript / SPA rendering via `chromiumoxide` over the Chrome DevTools Protocol | A system Chrome/Chromium browser (not bundled, except in the `runtime-headless` container target) |
| `local-inference` | Local LLM summarisation via `mistral.rs`: the `local` backend kind and the `rover model` subcommand | Model download (~1.6 GB) |
| `injection-model` | The ONNX DeBERTa prompt-injection classifier, the optional model layer of the guard | A native ONNX runtime; model download (~200 MB) |

### Distributions

| Distribution | `web-search` | `headless` |
| --- | --- | --- |
| Prebuilt binary (install script) | ✅ | ✅ |
| Homebrew formula | ✅ | ✅ |
| Container, default target | ✅ | ❌ — no Chromium in the distroless image |
| Container, `runtime-headless` target | ✅ | ✅ |
| `cargo install rover-fetch` | opt in | opt in |

`web-search` is in **every** official distribution, including the default
container target that deliberately omits `headless`. The two are excluded
for different reasons: `headless` needs a Chromium binary the distroless
image cannot carry, while `web-search` needs nothing at runtime, so leaving
it out of the container would mean a deployment silently missing a
capability every other channel has. See
[Deployment](/docs/deployment#spa-rendering) for the headless container
target and [Installation](/docs/install) for the packaged channels.

## `web-search`: finding URLs, not just reading them

Compile in `web-search` to add the `search` MCP tool and the `rover search`
subcommand. Rover then does both halves of the job: `search` discovers
candidate URLs, `fetch` reads the ones you pick. It never fetches a result
on your behalf.

Brave Search is the provider. The API key lives in an environment variable,
never in the config file:

```sh
export BRAVE_SEARCH_API_KEY=...
```

```toml
[search]
count = 5
country = "GB"
safe_search = "strict"
```

`rover doctor` reports one of three states — not compiled, compiled but not
configured, or configured — and neither unavailable state makes an install
unhealthy: Rover fetches perfectly well without search. Nothing pretends to
succeed either: without the feature or without a key, `search` returns
`search_feature_not_compiled` or `search_not_configured`, and the agent
steering `rover meta use` installs never mentions the tool at all. Full
usage, filters, result metadata, trust model and billing implications:
[Web search](/docs/web-search).

## `headless`: JavaScript and SPAs

Compile in `headless` when the pages you fetch render their content in JavaScript. Rover drives a system Chrome/Chromium over the DevTools Protocol (via `chromiumoxide`), grabs the rendered DOM, then runs it through the same extraction pipeline a static fetch uses.

The browser is not bundled, except in the `runtime-headless` container target (see [Deployment](/docs/deployment#spa-rendering)). Elsewhere, Rover expects one already on the host and auto-detects the standard install paths. Override the executable when yours lives somewhere non-standard:

```toml
[headless]
chrome_executable = "/opt/custom/chromium"
```

With the feature compiled in, `rover doctor` verifies the launch path, so a missing or misdetected browser surfaces before a fetch hits it. Full usage and configuration live in [JavaScript & dynamic pages](/docs/dynamic-pages).

## `local-inference`: on-device summarisation

Compile in `local-inference` to summarise without a network round-trip or an API key. It enables the `local` summariser backend kind, backed by `mistral.rs`. The default model is Qwen 3.5 0.8B (~1.6 GB), downloaded on first use.

```toml
[backends.offline]
kind = "local"
model = "Qwen/Qwen3.5-0.8B"     # any Hugging Face repo id

[summarization]
default_backend = "offline"
```

On macOS, Metal acceleration turns on automatically. The feature also brings in the `rover model` subcommand and model-integrity checking: Rover verifies cached files against their manifest before loading. For details on integrity checking, see [Security & threat model](/docs/security).

## `injection-model`: the ONNX classifier

Compile in `injection-model` to add the model layer of the prompt-injection guard. The structural wrapper and the pattern detector are always present. This feature adds an ONNX DeBERTa classifier that catches novel phrasings the rules don't enumerate. It pulls in a native ONNX runtime, and the classifier model (~200 MB) downloads on first use.

```toml
[prompt_injection]
model = "deberta-base"          # the classifier to load; "disabled" turns it off
```

`rover doctor` checks that the configured model is cached and valid. The wrapper holds either way: the classifier is an extra net, not the load-bearing guarantee. The full guard is documented in [Trust & prompt injection](/docs/trust).

## Image captioning needs no feature flag

Image captioning is always compiled in. It runs through cloud or OpenAI-compatible providers: OpenAI, Anthropic, Gemini, and anything that speaks the OpenAI chat-completions dialect.

```toml
[captioners.openai]
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[image_captions]
default = "openai"
```

There is no native local vision backend. To caption fully locally, point `provider = "openai_compat"` at a local vision server (Ollama, LM Studio, vLLM) running a vision-capable model. Full setup is in [Images & captioning](/docs/images).

## Model cache management

The `rover model` subcommand ships with `local-inference` and manages the cached model weights that feature depends on.

```sh
rover model download <repo_id>      # fetch a model into the cache ahead of time
rover model list                    # show cached models
rover model remove <repo_id>        # delete cached files
rover model verify                  # check cached files against their integrity manifest
```

Models download to `$HF_HOME/hub` (default `~/.cache/huggingface/hub`), shared with any other Hugging Face tooling on the host. Pre-download to avoid a cold first `summarize`, or let the first call fetch on demand.

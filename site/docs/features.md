---
id: features
title: Optional features
---

# Optional features

The default build does fetch-and-extract: no Cargo features, no model weights. Three opt-in features add capability. `headless` renders JavaScript pages, `local-inference` summarises on-device, and `injection-model` adds the ONNX prompt-injection classifier. Each one pulls in extra dependencies, and two of them download a model the first time you use them.

## Enabling features

Pass `--features` at install time. The crate is `rover-fetch` (the name `rover` was taken on crates.io); the installed binary is still `rover`.

```sh
cargo install rover-fetch                                       # default build
cargo install rover-fetch --features headless                   # one feature
cargo install rover-fetch --features headless,local-inference   # combine features
```

| Feature | Adds | Needs on first use |
| --- | --- | --- |
| `headless` | JavaScript / SPA rendering via `chromiumoxide` over the Chrome DevTools Protocol | A system Chrome/Chromium browser (not bundled) |
| `local-inference` | Local LLM summarisation via `mistral.rs`: the `local` backend kind and the `rover model` subcommand | Model download (~1.6 GB) |
| `injection-model` | The ONNX DeBERTa prompt-injection classifier, the optional model layer of the guard | A native ONNX runtime; model download (~200 MB) |

The prebuilt binary and the Homebrew formula already include `headless`. See [Installation](/docs/install) for the packaged channels.

## `headless`: JavaScript and SPAs

Compile in `headless` when the pages you fetch render their content in JavaScript. Rover drives a system Chrome/Chromium over the DevTools Protocol (via `chromiumoxide`), grabs the rendered DOM, then runs it through the same extraction pipeline a static fetch uses.

The browser is not bundled. Rover expects one already on the host and auto-detects the standard install paths. Override the executable when yours lives somewhere non-standard:

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

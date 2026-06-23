---
id: features
title: Optional features
---

# Optional features

**The default build is the lean fetch-and-extract core — no Cargo features, no extra runtime dependencies.** It clocks in around 20 MiB, needs no Chrome, and downloads no model weights. Three opt-in features add capability when you want it: `headless` for JavaScript-rendered pages, `local-inference` for on-device LLM summarisation, and `injection-model` for the ONNX prompt-injection classifier. Compile in only what you'll use; each one pulls in its own dependencies and, in two cases, a model download on first use.

## Enabling features

Opt in with `--features` at install time. The crate is `rover-fetch` (the name `rover` was taken on crates.io); the installed binary is still `rover`.

```sh
cargo install rover-fetch                                  # default: the lean basic build
cargo install rover-fetch --features headless              # one feature
cargo install rover-fetch --features headless,local-inference   # combine features
```

A plain `cargo install rover-fetch` builds the default, featureless binary. Pass `--features` to opt in; comma-separate to combine.

| Feature | Adds | Needs on first use |
| --- | --- | --- |
| `headless` | JavaScript / SPA rendering via `chromiumoxide` over the Chrome DevTools Protocol | A system Chrome/Chromium browser (not bundled) |
| `local-inference` | Local LLM summarisation via `mistral.rs` — the `local` backend kind and the `rover model` subcommand | Model download (~1.6 GB) |
| `injection-model` | The ONNX DeBERTa prompt-injection classifier — the optional model layer of the guard | A native ONNX runtime; model download (~200 MB) |

The prebuilt binary and the Homebrew formula already include `headless`. See [Installation](/docs/install) for the packaged channels.

## `headless` — JavaScript and SPAs

Compile in `headless` when the pages you fetch render their content in JavaScript. Rover drives a system Chrome/Chromium over the DevTools Protocol (via `chromiumoxide`) to get the rendered DOM, then runs the same extraction pipeline as a static fetch.

The browser is not bundled — Rover expects one already on the host. It auto-detects the standard install paths; override the executable when yours lives somewhere non-standard:

```toml
[headless]
chrome_executable = "/opt/custom/chromium"
```

When the feature is compiled in, `rover doctor` verifies the launch path so you find a missing or misdetected browser before a fetch does. Full usage and configuration: [JavaScript & dynamic pages](/docs/dynamic-pages).

## `local-inference` — on-device summarisation

Compile in `local-inference` to summarise without a network round-trip or an API key. It enables the `local` summariser backend kind, backed by `mistral.rs`. The default model is **Qwen 3.5 0.8B** (~1.6 GB), downloaded on first use.

```toml
[backends.offline]
kind = "local"
model = "Qwen/Qwen3.5-0.8B"     # any Hugging Face repo id

[summarization]
default_backend = "offline"
```

On macOS, Metal acceleration is enabled automatically. The feature also brings in the `rover model` subcommand and model-integrity checking — Rover verifies cached files against their manifest before loading. Model integrity is covered on [Security & threat model](/docs/security).

## `injection-model` — the ONNX classifier

Compile in `injection-model` to add the model layer of the prompt-injection guard. The structural wrapper and the pattern detector are always present; this feature adds an ONNX DeBERTa classifier for novel phrasings the rules don't enumerate. It pulls in a native ONNX runtime, and the classifier model (~200 MB) downloads on first use.

```toml
[prompt_injection]
model = "deberta-base"          # the classifier to load; "disabled" turns it off
```

`rover doctor` checks that the configured model is cached and valid. The wrapper holds regardless — the classifier is an extra net, not the load-bearing guarantee. See [Trust & prompt injection](/docs/trust) for the full guard.

## Image captioning needs no feature flag

Image captioning is always compiled in — there is no Cargo feature to enable. It runs through cloud or OpenAI-compatible providers (OpenAI, Anthropic, Gemini, and anything speaking the OpenAI chat-completions dialect):

```toml
[captioners.openai]
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[image_captions]
default = "openai"
```

There is no native local vision backend. For fully-local captioning, point `provider = "openai_compat"` at a local vision server (Ollama, LM Studio, vLLM) hosting a vision-capable model. Full setup: [Images & captioning](/docs/images).

## Model cache management

The `rover model` subcommand ships with `local-inference` and manages the cached model weights it depends on.

```sh
rover model download <repo_id>      # fetch a model into the cache ahead of time
rover model list                    # show cached models
rover model remove <repo_id>        # delete cached files
rover model verify                  # check cached files against their integrity manifest
```

Models download to `$HF_HOME/hub` (default `~/.cache/huggingface/hub`), shared with any other Hugging Face tooling on the host. Download ahead of time to avoid a cold first `summarize`, or let the first call fetch on demand.

## A note on size

The default build is lean — around 20 MiB. CI enforces a hard ceiling: the default-features binary must stay under 75 MiB, asserted on every run. The features above add to that, and two of them add a separate on-disk model download you manage yourself: `local-inference` pulls its default model (~1.6 GB) on first use, and `injection-model` pulls the classifier (~200 MB). `headless` adds no model — it drives a Chrome/Chromium you already have. Compile in only what you need and the rest stays out of the binary.

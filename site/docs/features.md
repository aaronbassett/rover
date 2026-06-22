---
id: features
title: Features
---

# Feature Flags

Rover ships with two optional Cargo features. The default install
(`cargo install rover`) produces a lean binary under 75 MiB with no
mistralrs, no chromiumoxide, and no extra model weights to manage.

Enable any combination of features by passing `--features` to
`cargo install` (or to `cargo build` if you're working from source).

| Feature | Enables | Approx. binary size add |
| --- | --- | --- |
| `local-inference` | Local LLM summarization via `mistral.rs` (default model: Qwen 3.5 0.8B) | ~80 MB |
| `headless` | SPA rendering via `chromiumoxide` (system Chrome required) | ~32 MB |

Image captioning is **always compiled in** via cloud / OpenAI-compatible
providers (OpenAI, Anthropic, Gemini, plus local servers like ollama and
LM Studio through `provider = "openai_compat"`) — no feature flag required.
There is no local mistralrs vision backend; see "Image captioning" below.

---

## `local-inference`

```
cargo install rover --features local-inference
rover model download Qwen/Qwen3.5-0.8B    # ~1.6 GB; one-time
```

In `~/.config/rover/config.toml`:

```toml
[backends.local]
kind = "local"
model = "Qwen/Qwen3.5-0.8B"

[summarization]
default_backend = "local"
```

**Memory profile:** ~1.5–2 GB resident with the default model loaded.
The model loads lazily on first `summarize` call (cold latency: 5–20
seconds depending on hardware); subsequent calls warm.

**macOS:** Metal acceleration enabled automatically.
**Linux/Windows:** CPU-only by default. CUDA support is a v2 feature.

---

## Image captioning (always-on)

No feature flag required — captioning runs through `genai`-backed cloud or
OpenAI-compatible providers:

```toml
[captioners.openai]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[image_captions]
default = "openai"
```

Supported providers: `openai`, `anthropic`, `gemini`, `openai_compat`
(LM Studio, Ollama, vLLM, etc.). The `genai` crate documents the full
list.

### Local captioning via an OpenAI-compatible server

For fully-local captioning, point `provider = "openai_compat"` at a local
server running a vision-capable model:

```toml
[captioners.ollama]
kind = "cloud"
provider = "openai_compat"
model = "llama3.2-vision"                 # any vision model the server hosts
base_url = "http://localhost:11434/v1"    # LM Studio: http://localhost:1234/v1
# api_key_env optional — keyless local servers need no key
```

:::note
A native local backend (`mistralrs`/SmolVLM, behind a `local-vision` feature)
previously existed but was removed: it was unusable on the CPU backend the
nightly runs on (vision-attention contiguity + encoder-cache bugs in
mistralrs 0.8.x). The OpenAI-compatible path above replaces it and offloads
inference to a server built for it. See git history to restore the old code.
:::

---

## `headless`

```
cargo install rover --features headless
```

Requires a Chrome/Chromium browser on the host. Rover auto-detects:

| Platform | Default detection path |
| --- | --- |
| Linux | `google-chrome` or `chromium` on `$PATH` |
| macOS | `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` |
| Windows | `Program Files` + registry lookups |

Install hints:
- **Linux:** `sudo apt install chromium-browser` or distro equivalent
- **macOS:** `brew install --cask google-chrome` (or use Chromium)
- **Windows:** Download Chrome from [https://www.google.com/chrome/](https://www.google.com/chrome/)

Override the detected path:

```toml
[headless]
chrome_executable = "/opt/custom/chromium"
```

Verify the launch path with `rover doctor`.

**Asset interception.** Rover uses CDP's Fetch domain to block (via
`FulfillRequest` with empty 200 — never `failRequest`) ad/tracker
domains, third-party requests, fonts, media, and (by default) images.
See [Security](/docs/security) for the security model and [Configuration](/docs/configuration)
for the full `[headless]` block reference.

---

## `rover model` cache management

When `local-inference` is compiled in:

```
rover model download <repo_id>      # download to HF_HOME cache
rover model list                    # show cached models
rover model remove <repo_id>        # delete cached files
```

Cache root: `$HF_HOME/hub` (default `~/.cache/huggingface/hub`).
The cache is shared with any other HuggingFace-using tools.

---

## Binary size

Default-features binary: < 75 MiB (asserted nightly in the smoketest workflow).

With features enabled, expect roughly:

| Combination | Approx. size |
| --- | --- |
| `local-inference` | ~105 MB |
| `headless` | ~57 MB |
| `local-inference + headless` | ~135 MB |
| All features | ~140 MB |

Real numbers depend on toolchain and target; the CI matrix tracks current
sizes for `x86_64-unknown-linux-gnu`.

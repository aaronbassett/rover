# Feature Flags

Rover ships with three optional Cargo features. The default install
(`cargo install rover`) produces a lean binary under 75 MiB with no
mistralrs, no chromiumoxide, and no extra model weights to manage.

Enable any combination of features by passing `--features` to
`cargo install` (or to `cargo build` if you're working from source).

| Feature | Enables | Approx. binary size add |
| --- | --- | --- |
| `local-inference` | Local LLM summarization via `mistral.rs` (default model: Qwen 3.5 0.8B) | ~80 MB |
| `local-vision` | Local image captioning via `mistral.rs` (default model: Qwen2-VL 2B) | shared with `local-inference`; ~5 MB additional |
| `headless` | SPA rendering via `chromiumoxide` (system Chrome required) | ~32 MB |

Cloud image captioners (OpenAI, Anthropic, Gemini, anything `genai`
supports) are **always compiled in** and don't require any feature flag.

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

## `local-vision`

```
cargo install rover --features local-vision
rover model download Qwen/Qwen2-VL-2B-Instruct
```

Configure under `[captioners.<name>]`:

```toml
[captioners.local]
kind = "local"
model = "Qwen/Qwen2-VL-2B-Instruct"

[image_captions]
default = "local"
```

Available variants (swap via the `model` field):
- `Qwen/Qwen2-VL-2B-Instruct` — smallest, runs on the CPU backend
- `Qwen/Qwen2.5-VL-3B-Instruct` — larger, better quality

> The smaller SmolVLM/Idefics3 family is **not** recommended: on the CPU
> backend mistralrs uses, its vision attention feeds candle a non-contiguous
> matmul input and its encoder cache panics when image-splitting is enabled
> (upstream [PR #2074](https://github.com/EricLBuehler/mistral.rs/pull/2074)
> covers only the latter). Qwen2-VL's vision path is contiguity-safe.

---

## Cloud captioners (always-on)

No feature flag required:

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
- **Windows:** Download Chrome from <https://www.google.com/chrome/>

Override the detected path:

```toml
[headless]
chrome_executable = "/opt/custom/chromium"
```

Verify the launch path with `rover doctor`.

**Asset interception.** Rover uses CDP's Fetch domain to block (via
`FulfillRequest` with empty 200 — never `failRequest`) ad/tracker
domains, third-party requests, fonts, media, and (by default) images.
See `docs/security.md` for the security model and `docs/configuration.md`
for the full `[headless]` block reference.

---

## `rover model` cache management

When either `local-inference` or `local-vision` is compiled in:

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
| `local-vision` | ~105 MB (shares mistralrs with local-inference) |
| `headless` | ~57 MB |
| `local-inference + headless` | ~135 MB |
| All features | ~140 MB |

Real numbers depend on toolchain and target; the CI matrix tracks current
sizes for `x86_64-unknown-linux-gnu`.

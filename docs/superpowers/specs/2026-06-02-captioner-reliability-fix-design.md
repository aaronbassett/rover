# Captioner reliability fix — design

- **Date:** 2026-06-02
- **Branch context:** follows `fix/remove-local-vision` (commit `1b2cbae`, which removed the local mistralrs vision captioner)
- **Status:** approved design, pending implementation plan

## Background

After `local-vision` was removed, the only remaining captioner is `CloudCaptioner`
(`src/vlm/cloud.rs`), which wraps `genai` 0.4.4 and is compiled into every build (no
feature flag). It is meant to cover both hosted vision APIs (OpenAI, Anthropic, Gemini)
and any OpenAI-compatible server (Ollama, LM Studio, vLLM) via
`provider = "openai_compat"` + `base_url`.

We set out to confirm, with live runs, that captioning still works through:

1. a hosted API, and
2. a local model served over an OpenAI-compatible API (Ollama).

Captioning is reachable only through the MCP `fetch` tool (`images.mode = "caption"`)
and the `rover doctor` probe; the plain `rover fetch` CLI has no images flag.

## Live confirmation results

**Cloud (Gemini `gemini-2.5-flash-lite`) — confirmed, accurate.**
Captioned the Wikipedia *Cat* article and the screenshots in an askubuntu answer
end-to-end through the real MCP `fetch_tool` path. Captions were faithful, including
text-heavy screenshots:

| Image | Ground truth | Caption | Match |
|---|---|---|---|
| askubuntu `XyZsY.png` | bash config with `alias rm='rm -i'` | "This code snippet defines an alias for the `rm` command to prompt for confirmation." | yes |
| askubuntu `N4Dki.png` | terminal showing `rm: remove regular file 'Screenshot.png'?` | "Terminal prompt asks to confirm removal of a file." | yes |

The caption cache (`summary_cache`) was also verified (second run returned
`cache_status: hit`).

**Local (Ollama `moondream`) — works only after fixing a bug (see Finding A).**
The model and the OpenAI-compatible endpoint are fine: a direct
`curl` to `http://localhost:11434/v1/chat/completions` with a base64 `image_url`
returned HTTP 200 and a caption. Through rover, every image returned
`404 page not found` and silently degraded to `decision: skipped`,
`reason: captioner_error`. Adding a trailing slash to `base_url` made every image
`decision: captioned` with no error. (moondream's caption *quality* on text-heavy
screenshots is poor — a 1.8B-model limitation, not a rover issue.)

## Findings

### A — `openai_compat` base_url is not normalized in the captioner path (functional bug, high)

`normalize_openai_compat_base_url` (`src/summarizer/registry.rs:229`) forces a trailing
`/v1/`. It is applied **only** in the summarizer build path (`registry.rs:165–182`).
The VLM captioner path (`src/vlm/mod.rs:185`) passes `cfg.base_url` straight to
`CloudCaptioner::new` → `build_client` with no normalization.

Consequence: `base_url = "http://localhost:11434/v1"` — **the exact Ollama example in
`docs/configuration.md:171`** — makes genai build a malformed URL and return
`404 Not Found` / `404 page not found`. The documented "point rover at Ollama/LM Studio"
happy path is broken out of the box.

Evidence (genai error captured from the `images_processed` `error:` annotation):

```
captioner ollama unavailable: Web call failed for model 'moondream (adapter: OpenAI)'.
Cause: Request failed with status code '404 Not Found'. Response body:
404 page not found
```

### B — `rover doctor` skips keyless captioners (medium)

`CaptionersAuthenticate::run` (`src/doctor/checks.rs`) filters to captioners whose
`api_key_env` resolves to a non-empty value before probing. A keyless `openai_compat`
captioner (the normal Ollama/LM Studio case — the shared client sends `"noop"` as a
bearer token, which the local server ignores) is therefore **skipped**, so doctor never
validates the local path. The summarizer's `CloudBackendsAuthenticate` check has the
identical filter and gap.

### C — `rover doctor` probe budget is too small (medium)

The probe calls `cap.caption(PROBE_PNG, None, 1)` — `max_tokens = 1` with a 1×1
transparent PNG. With a 1-token budget the model emits no content; for Gemini this
surfaces as a genai stream error, so doctor reports failure even though captioning
works at a normal budget. Confirmed live: `gemini-2.5-flash` returned
`promptTokensDetails` (image processed: 258 image tokens) but errored with
`finishReason: null` under the 1-token cap.

### D — Caption/download failures are silent and partly mislabeled (diagnosability)

Caption failures (`images.rs` ~254) and download failures (~201) degrade to `skipped`
with the underlying error stored only in the `error:` annotation and **no log**, which
made Finding A hard to diagnose. Additionally, a *download* failure is recorded with
`reason: "captioner_error"` (`images.rs:206`) — the Wikimedia `429` download errors were
mislabeled as captioner errors.

## Design

Centralize normalization so all paths are fixed from one place, and make doctor exercise
and validate the local path it currently skips.

### A — normalize in `build_client`

`build_client(provider, base_url, api_key)` (`src/summarizer/cloud.rs:59`) is the single
chokepoint already called by `CloudBackend::new` (summarizer/cloud.rs:166),
`CloudCaptioner::new` (vlm/cloud.rs:45), and indirectly by both doctor probes
(checks.rs:276, :377).

- Move `normalize_openai_compat_base_url` and its unit tests from
  `summarizer/registry.rs` to `summarizer/cloud.rs`, beside `build_client`.
- Inside `build_client`'s `if provider == OpenAiCompat` branch, normalize `base_url`
  before constructing the `ServiceTargetResolver`. The function is idempotent, so an
  already-`/v1/` URL is unaffected.
- Remove the redundant pre-normalization block at `registry.rs:165–182`; the summarizer
  passes raw `base_url` and `build_client` normalizes. Drop the per-backend
  "auto-normalized" `info!` log (not worth threading the backend name into
  `build_client`).
- Result: captioner registry, captioner doctor probe, summarizer registry, and
  summarizer doctor probe all receive a normalized base_url.

### B — doctor probes keyless/local captioners

In `CaptionersAuthenticate::run`, replace the "non-empty `api_key_env`" gate with: probe
when `provider == openai_compat` **and** `base_url` is set (keyless local OK), **or**
non-`openai_compat` cloud **and** a key resolves (current behavior). Skip only what can't
be built or authenticated. Update the skip/summary detail strings accordingly. Apply the
same change to `CloudBackendsAuthenticate` (summarizer) for consistency.

### C — doctor probe budget + image

Replace the 1×1 transparent PNG with a small non-degenerate image (an 8×8 solid-color
PNG) and raise the probe budget to `max_tokens = 64`. This reliably elicits at least one
token from Gemini (lite and thinking) and Ollama, so the probe reflects reachability +
auth + a real caption call rather than a budget artifact.

### D — diagnosability

- Add `tracing::warn!(target: "rover::extractor", url = %src, err = %e, …)` at the
  caption-failure site (`images.rs` ~254) and the download-failure site (~201).
- Give download failures a distinct `reason: "download_error"` (currently
  `"captioner_error"` at `images.rs:206`) so download problems are distinguishable from
  genai/captioner problems. The full error string remains in the `error:` annotation.

## Testing (TDD — failing tests first)

- **A:** extend `tests/vlm_cloud_smoke.rs` to build a `CloudCaptioner` with a base_url
  lacking `/v1/` (bare host, and `…/v1`) and assert the wiremock server receives
  `POST /v1/chat/completions`. Keep the moved `normalize_openai_compat_base_url` unit
  tests green in their new location.
- **B:** doctor test (wiremock-backed) asserting a keyless `openai_compat` captioner is
  probed, not skipped.
- **C:** assert the probe constants are sane (non-degenerate image, budget > 1) and a
  wiremock probe round-trips successfully.
- **D:** assert a download failure yields `reason = "download_error"` and a captioner
  failure yields `reason = "captioner_error"`.

## Docs

After A, the `docs/configuration.md` examples (`…/v1`, bare host) become correct. Verify
the "auto-normalized to end in `/v1/`" claim now also holds for `[captioners.*]` and
adjust wording if it currently implies summarizer-only. No structural doc changes.

## Out of scope

- moondream caption quality on text-heavy images (model limitation; use a hosted model or
  a larger local model such as `llava:7b` / `qwen2.5vl`).
- Wikimedia `429`s on rapid image downloads — image fetches may not share the page
  fetcher's per-domain pacing. Real, but a separate concern; tracked as a follow-up, not
  fixed here.

## Notes for implementation

- Toolchain: this repo's MSRV is 1.96.0 and a Homebrew `rustc 1.93.1` shadows rustup on
  this machine. Build/test with the 1.96.0 toolchain forced:
  `RUSTC=$(rustup which --toolchain 1.96.0 rustc) rustup run 1.96.0 cargo <cmd>`.
- The throwaway confirmation config used during testing lives at `./rover-confirm.toml`.
  It is an untracked local file, not part of this change; delete it (or leave it
  untracked) and do not commit it.

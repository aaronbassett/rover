# Rover M9 — Feature-Flagged Extras — Design

> Status: design complete, awaiting implementation plan.
>
> Prerequisites: M1 (fetcher + SSRF validator), M2 (cache + storage actor), M3 (MCP server, fetch tool schema, tokenizer infra), M4 (extraction pipeline; `ImagesMode::CaptionVlm` stub), M5 (rate-limited fetcher + robots), M6 (task scheduler), M7 (`SummarizerBackend` trait, registry pattern, summary cache, fallback machinery), M8 (`rover doctor` `Check` trait + registry, full SSRF level matrix, `[debug]` config, HAR recorder).
>
> Canonical references:
> - PRD §2 (V1 in/out scope, including the three feature-flag entries), §5.7 (headless), §7.3 (local inference), §7.4 (image captioning), §11.2 (`rover doctor`), §12 (configuration), §13 (recommended crate deps), §14 M9 (acceptance), §15 (non-functional — binary size, memory), §16 (security: SSRF + resource exhaustion).
> - Design supplement §2 (general architecture); no §-specific items M9 inherits beyond what the PRD covers.
> - Milestone manifest §M9 (file layout, original open questions, deferrals).
> - M7 design §3 (registry pattern), §3.14 (per-module error → MCP code mapping).
> - M8 design §7 (`Check` trait, doctor extension pattern), §3.2 (component-boundary discipline).

---

## 1. Scope and Goals

M9 ships the three Cargo-feature-gated subsystems the PRD's §2 ("V1 — Behind Cargo Feature Flags") promised. Each subsystem is independent of the other two; default `cargo install rover` produces today's lean binary.

**Local LLM summarization**
1. New `LocalMistralRs` summarizer backend (PRD §7.3) implementing the M7 `SummarizerBackend` trait. Gated by the `local-inference` Cargo feature. Configured as `[backends.<name>]` with `kind = "local"`. Default model: `Qwen/Qwen3.5-0.8B`.

**Headless SPA support**
2. New `HeadlessRenderer` (PRD §5.7) using `chromiumoxide`. Gated by the `headless` Cargo feature. Three per-call modes — `Off`, `On`, `Auto` — wired into `fetcher::cached::fetch_with_cache`. SPA detection heuristics drive `Auto`. CDP Fetch-domain interception fulfils blocked sub-requests with empty 200 responses (PRD §5.7 mandate — never `failRequest`).

**Image captioning (local + cloud)**
3. New `VlmCaptioner` trait with two implementations:
   - `MistralRsCaptioner` (local SmolVLM via `mistralrs`), gated by the `local-vision` Cargo feature. Default model: `HuggingFaceTB/SmolVLM-256M-Instruct`.
   - `CloudCaptioner` (vision-capable cloud models via the existing `genai` dep), **always compiled in**. Supports OpenAI (`gpt-4o`, `gpt-4o-mini`), Anthropic (Claude with vision), Gemini, anything `genai` exposes that accepts image inputs.
4. New `CaptionerRegistry` mirroring `SummarizerRegistry`. Configured under `[captioners.<name>]` and `[image_captions]`.
5. Wires the M4 `ImagesMode::CaptionVlm` stub (renamed `ImagesMode::Caption` — see §2) into a working captioner call.
6. Cost-control knobs: `max_per_page`, `min_width`, `min_height`, `max_bytes`.

**Operational surface for downloaded models**
7. New `rover model {download|list|remove}` CLI subcommand. Compiled in whenever `local-inference` or `local-vision` is enabled. Wraps the existing `hf-hub` dep with stderr progress.

**`rover doctor` extensions**
8. Three new feature-gated checks appended to `doctor::run_all` via `#[cfg(...)]` blocks: `local_inference_model_cached`, `local_vision_model_cached`, `headless_browser_launches`. Plus a non-gated `captioners_authenticate` check that mirrors M8's `backends_authenticate` for cloud captioners.

**Docs**
9. New `docs/features.md`: per-feature install/setup, system Chrome guidance, expected binary sizes, model download UX. Plus targeted edits to `docs/security.md` (headless asset interception + SSRF) and `docs/configuration.md` (the new sections).

**Acceptance (PRD §14, M9).** Each feature works in isolation. Default `cargo install rover` produces a lean binary (binary-size CI assertion, see §11). Users opting in get the extras.

---

## 2. Decisions Inherited from Open-Question Round

| Question | Decision |
| --- | --- |
| `mistralrs` version pin | `mistralrs = "0.8.1"` on crates.io. Single unified `ModelBuilder::new(repo_id)` covers both text and multimodal models — auto-detected from the HF repo id. The same `mistralrs` dep is shared by `local-inference` and `local-vision`; Cargo's feature unification compiles the crate once. Final patch-version pin verified at plan time. Pinned against the published crate + upstream `mistralrs/examples/getting_started/{text_generation,multimodal}/main.rs` (docs.rs failed to build 0.8.1; we do not depend on docs.rs). |
| `chromiumoxide` browser discovery | System-Chrome first, no bundling. `BrowserConfig::default()` auto-detects an installed Chrome/Chromium on Linux/macOS/Windows (PATH + standard system paths). Users override via `[headless] chrome_executable = "/path/to/chrome"`. `rover doctor` runs a real launch + immediate close and reports the resolved executable path. Bundling via `chromiumoxide_fetcher` rejected: ~150 MB download, version-drift vs system Chrome, macOS code-signing pain. Documented as a prerequisite in `docs/features.md` with one-line install hints per platform. |
| Model file distribution UX | Hybrid: auto-download on first use **plus** explicit `rover model download <repo_id>`. First-use path logs one stderr line up-front (`downloading <repo_id> from HuggingFace; cached at ~/.cache/huggingface — this may take several minutes`) and lets `mistralrs::ModelBuilder` resolve via the HF hub. The `rover model` subcommand wraps the existing `hf-hub` dep with explicit stderr progress for ahead-of-time installation. Single subcommand covers both text and vision models — same HF cache. |
| `vlm` feature naming | Renamed **`vlm` → `local-vision`**. Pairs with `local-inference`, both share the `mistralrs` dep, both describe what users get. This is a divergence from the PRD's `vlm` naming, recorded here and in `docs/features.md`. PRD will be updated alongside the M9 docs commit. |
| `ImagesMode::CaptionVlm` enum variant | Renamed **`CaptionVlm` → `Caption`** (wire value `"caption"`). Captioning is the only image-captioning mode; the `Vlm` suffix carried no information once cloud captioners landed. The variant has never executed (M4 stubbed it with an error), so renaming is safe. Variant is **feature-gated**: `#[cfg(any(feature = "local-vision", feature = "image_captions"))]` — but since cloud captioners ship in default builds, the variant is effectively always-present. Treated as always-present in the spec; the `#[cfg]` is only on the `local-vision`-only fallback when no captioners are configured at all. |
| Cloud captioners | In scope. New `CaptionerRegistry` parallels `SummarizerRegistry`. `CloudCaptioner` uses the existing `genai` dep (M7), compiled in by default. Supports any `genai` provider that accepts images. `MistralRsCaptioner` is the local impl, gated by `local-vision`. |
| Cost-control filters | Three knobs under `[image_captions]`: `max_per_page` (default 10), `min_width`/`min_height` (default 200/200), `max_bytes` (default 10 MiB). Filter order: dimension gate → size gate → budget gate (first-N in document order). Each skipped image annotated in the frontmatter under `images_processed`. |
| Image dimension probe strategy | Trust `<img width=… height=…>` HTML attributes when both are present — those are *display* dimensions, which is the right signal for "is this an icon". When absent, partial-fetch the image header bytes (`Range: bytes=0-2047`) and use the `image` crate's `Reader::with_format(...).into_dimensions()` to read dimensions without decoding. Probes go through the SSRF gate, rate limiter, and HAR recorder. |
| Headless asset interception action | **Always `FulfillRequestParams` with an empty 200 body — never `FailRequestParams`.** PRD §5.7 mandate: many SPAs error hard on failed CSS/font requests but handle empty stylesheets gracefully. Implemented in `src/fetcher/headless/intercept.rs`. |
| Headless asset block defaults | Per PRD §5.7: `block_media = true`, `block_images = true`, `block_fonts = true`, `block_css = false`, `block_third_party = true`, `block_service_workers = true`. Already present in PRD §12's `[headless]` section; M9 honours these defaults and adds two M9-specific keys (`chrome_executable`, `max_concurrent`). |
| Headless SSRF interaction | The CDP intercept handler runs *every* sub-request URL through the existing `fetcher::ssrf::validate_addresses` against the configured `SsrfLevel`. Sub-requests that would violate the level get fulfilled with empty 200 (same action as block-list matches). This closes a real privacy hole: without this, a `Strict`-level fetch via headless could reach RFC1918 addresses through page-embedded `<iframe>` / `<img>`. Documented in `docs/security.md`. |
| Headless concurrency model | One `chromiumoxide::Browser` per process for the binary's lifetime, hosted by the `HeadlessRenderer`. Page-level concurrency capped by a `tokio::sync::Semaphore`. Default permits = 4 (`[headless] max_concurrent`). Configurable. |
| Headless HAR recording | Only the top-level navigation is recorded in HAR — not sub-resource fetches. Sub-resource HAR entries would explode file size and obscure what Rover actually returned. Documented in `docs/cli.md` HAR section. |
| `rover model` subcommand surface | `rover model download <repo_id>` (positional), `rover model list` (lists cached models from the HF cache root), `rover model remove <repo_id>`. Compile-gated on `any(feature = "local-inference", feature = "local-vision")`. Otherwise absent from `--help`. |
| `--model <hf_repo_id>` per-call override | Deferred. M9 ships `[backends.<name>] model = ...` and `[captioners.<name>] model = ...` as the model-swap surface, plus per-call `backend: "<name>"` / `captioner: "<name>"` override. A `--model` CLI shortcut is a v2 ergonomic; full multi-backend config covers the swap use-case today. |
| `local-inference` first-call latency | Documented, not engineered. Cold load takes seconds; the first MCP `summarize { backend: "local" }` call blocks for the load. The `OnceCell<Arc<Model>>` warms after the first call. M7's synchronous tool contract stands. |
| `mistralrs` backend-feature selection | Pull `mistralrs` with `default-features = false`. Enable `metal` only on `target_os = "macos"` via Cargo target-specific deps. CUDA stays explicitly off — adding it is a v2 ask gated behind a separate `cuda` feature. CPU-only is the default on Linux. |
| `local-vision` doctor check shape | Verify the configured `[captioners.<name>] model` for any `kind = "local"` captioner is present in the HF cache (`~/.cache/huggingface/hub/models--<owner>--<repo>/`). Cheap check — no actual model load. Skip if no local captioner is configured. |
| Caption cache reuse | Captions are deterministic over `(sha256(image_bytes), captioner_id, captioner_model_id, max_tokens)`. Stored in the existing `summary_cache` table keyed by a `params_hash` whose inputs include the captioner identity. Field naming reuses M7's pattern. |
| Cloud captioner concurrency | Same shape as headless: shared registry + per-captioner `Semaphore`. Default permits = 2 (`[image_captions] max_concurrent`). VLM inference is slow and API quotas burn fast. |
| Schema migration | None. The new caption-cache rows reuse the existing `summary_cache` table (M7 migration `005_summary_cache.sql`). The `params_hash` field absorbs the new captioner identity. |

---

## 3. Architecture

### 3.1 Module layout

```
src/
  summarizer/
    local.rs                   # NEW: LocalMistralRs; cfg(feature = "local-inference")
    registry.rs                # MODIFIED: build_one's "local" arm; cfg(feature = "local-inference")
  vlm/
    mod.rs                     # NEW: VlmCaptioner trait + CaptionerRegistry; always compiled
    cloud.rs                   # NEW: CloudCaptioner via genai; always compiled
    local.rs                   # NEW: MistralRsCaptioner; cfg(feature = "local-vision")
    error.rs                   # NEW: VlmError (thiserror)
    cache.rs                   # NEW: image-caption cache helpers over summary_cache
  fetcher/
    cached.rs                  # MODIFIED: optional HeadlessRenderer on FetchOptions; auto-detect retry path
    headless/                  # NEW directory; cfg(feature = "headless")
      mod.rs                   #   re-exports HeadlessRenderer, HeadlessMode
      browser.rs               #   browser launch + page-pool semaphore
      detect.rs                #   SPA heuristics (PRD §5.7)
      intercept.rs             #   CDP Fetch domain handler: fulfill blocked w/ empty 200
      third_party.rs           #   minimal EasyList-derived block list
  extractor/
    images.rs                  # MODIFIED: ImagesMode::Caption arm; dimension probes; filters; per-image annotation
    options.rs                 # MODIFIED: ImagesMode::Caption variant; ImageCaptionFilters struct
    frontmatter.rs             # MODIFIED: emit images_processed sidecar
  doctor/
    checks.rs                  # MODIFIED: feature-gated checks appended to run_all
  config/
    mod.rs                     # MODIFIED: [vlm] removed; [image_captions] + [captioners.*]; [headless] gains keys
  cli/
    mod.rs                     # MODIFIED: register `Model` subcommand
    model.rs                   # NEW: download/list/remove
  mcp/tools/
    fetch.rs                   # MODIFIED: typed headless arg; Caption wiring; ImagesArg::Caption rename

tests/
  local_inference_smoke.rs     # cfg(feature = "local-inference"); ignored by default
  headless_smoke.rs            # cfg(feature = "headless"); ignored by default
  vlm_local_smoke.rs           # cfg(feature = "local-vision"); ignored by default
  vlm_cloud_smoke.rs           # always; wiremock-backed openai_compat vision
  cli_model.rs                 # cfg(any(feature = "local-inference", feature = "local-vision"))
  images_caption_filters.rs    # always; covers dimension/size/budget gates
  headless_ssrf_intercept.rs   # cfg(feature = "headless"); sub-request SSRF gate

docs/
  features.md                  # NEW: per-feature install/setup/sizing/UX
  security.md                  # MODIFIED: headless asset interception + SSRF section
  configuration.md             # MODIFIED: new sections + chrome_executable
```

### 3.2 Component boundaries

Each new subsystem honours the M8 component-boundary discipline:

- **`summarizer::local`** — single type `LocalMistralRs` implementing `SummarizerBackend`. Owns one `OnceCell<Arc<mistralrs::Model>>`. No knowledge of registry construction (handled in `registry.rs::build_one`). No knowledge of caching (handled by `SummarizerService`).
- **`vlm`** — exposes `VlmCaptioner` trait + `CaptionerRegistry`. Two impls (`CloudCaptioner`, `MistralRsCaptioner`). The image-caption cache wrapper (`vlm::cache`) is the only consumer of `summary_cache` for caption rows; callers go through it, not the table directly.
- **`fetcher::headless`** — exposes `HeadlessRenderer` with `new(config) -> Result<Self>`, `render(url, opts) -> Result<RenderedPage>`. Independent of the M9 summarizer + captioner work. The renderer is the only consumer of `chromiumoxide`; the rest of the codebase sees `RenderedPage { final_url, html, status, top_level_har_entry }`.
- **`cli::model`** — wraps the existing `hf-hub` crate. Reads no Rover config and writes no Rover state. Output to stderr.
- **`extractor::images`** — extended with three new pure helpers (`should_caption_by_dimensions`, `partial_fetch_dimensions`, `pick_caption_budget`) plus the captioner call site. The filter knobs live in `ImageCaptionFilters` (passed through `ExtractOptions`).

### 3.3 Component diagram

```
                        ┌──────────────────────────┐
                        │      MCP Server          │
                        │  (rover mcp / rmcp stdio)│
                        └──┬────┬──────────────┬───┘
                           │    │              │
                ┌──────────┴┐  ┌┴────────────┐ ┌┴────────────┐
                │ fetch tool │  │ summarize    │ │ images tools │
                │ - headless │  │ - LocalMist- │ │ via fetch    │
                │   arg      │  │   ralRs      │ │ - Caption    │
                │ - caption  │  │   backend    │ │   mode       │
                └──┬─────────┘  └──┬───────────┘ └─┬───────────┘
                   │               │               │
        ┌──────────┴──────┐  ┌─────┴──────┐  ┌─────┴───────┐
        │ HeadlessRenderer│  │ Summarizer │  │ Captioner   │
        │  (cfg=headless) │  │ Registry   │  │ Registry    │
        │   - Browser     │  │ + Service  │  │   (always)  │
        │   - Semaphore   │  │            │  │             │
        │   - Intercept   │  └────┬───────┘  └─┬────┬──────┘
        │      handler    │       │            │    │
        └────┬────────────┘  ┌────┴───┐  ┌─────┴──┐ ┌┴───────────┐
             │               │Local-  │  │ Cloud   │ │ MistralRs   │
             │               │Mistral │  │Captioner│ │ Captioner   │
             │               │Rs      │  │(always) │ │(local-vision)│
             │               │(local- │  │ via     │ │              │
             │               │inferen-│  │ genai   │ │              │
             │               │ce)     │  │         │ │              │
             │               └────────┘  └─────────┘ └──────────────┘
             │
             └─── fetcher::cached::fetch_with_cache (M2/M5/M6)
                       │
                       └─── HAR recorder (M8) [top-level nav only]
                       └─── SSRF validator (M8) [also applied per intercepted sub-request]
                       └─── Storage actor (M2) [pages + summary_cache]
```

### 3.4 Lifecycle: `fetch` with headless

```
1. RoverHandler::fetch dispatches into fetch_inner.
2. Resolve headless mode from args: Off | On | Auto (default Off; or from
   [headless] auto_detect_spa for the no-arg case).
3. Build FetchOptions { headless: Option<Arc<HeadlessRenderer>>, headless_mode, ... }.
4. Call fetcher::cached::fetch_with_cache.
5. Inside fetch_with_cache:
   - HeadlessMode::Off: today's reqwest path (M1).
   - HeadlessMode::On: skip reqwest network; renderer.render(url) → html;
     existing extractor::pipeline::extract runs against rendered html.
   - HeadlessMode::Auto:
       a. Try reqwest path first; if it errors (Status/Network), propagate.
       b. Run extractor on the reqwest html.
       c. Call headless::detect::detect_spa(html, extracted_md).
          Returns a HitCount (struct with per-heuristic booleans + total).
       d. If total >= 2: re-render via renderer, re-extract, replace
          ExtractResult. Tag metadata.headless_used = true.
       e. Otherwise: keep the reqwest result.
6. The cache write (Step 7 in M2's cached.rs) is unchanged. Rendered pages cache
   exactly like reqwest pages — keyed on the canonical URL, body is the rendered
   HTML at the moment of capture.
```

The renderer is **not** cache-aware. Caching lives in `fetch_with_cache`. The renderer just produces HTML.

### 3.5 Lifecycle: `summarize` with local backend

Same shape as M7's lifecycle (§3.3 of M7 spec) — no changes to the service layer or the cache. `LocalMistralRs::compact` is just another `SummarizerBackend` impl. The only behavioural difference: cold-load latency on the first call.

```
1. Service.compact(content, opts):
   - hash params
   - cache lookup
   - on miss: backend.compact(content, opts)
       LocalMistralRs::compact:
         a. Resolve self.model (OnceCell): if uninitialised, load via
            mistralrs::ModelBuilder::new(self.repo_id)
              .with_auto_isq(IsqBits::Eight)
              .with_logging()
              .build().await
            Emit one stderr line before the load if the HF cache for the repo
            id is absent (first-use download notice).
         b. Build TextMessages with system + user roles.
         c. model.send_chat_request(messages).await
         d. Extract response.choices[0].message.content; trim; return.
   - on cloud-style errors (load failure, OOM, panic): map to
     BackendError::Unavailable. The existing fallback_to_extractive path takes over.
   - cache write.
```

### 3.6 Lifecycle: `images.mode = "caption"`

```
extractor::images::apply(markdown, mode, paths, http, vlm, filters):
    enumerate INLINE_IMG matches
    for each (alt, src, rest):
        if mode != Caption: existing arm
        else:
            decision = filter_pipeline(src, &filters):
                # 1. dimension gate
                dims = html_attr_dims(rest) or partial_fetch_dims(http, src).await
                if dims.is_some() && dims.below_min(filters): skip(below_min_dimensions)
                # 2. size gate
                if content_length(http, src).await > filters.max_bytes: skip(above_max_bytes)
                # 3. budget gate
                if captioned_so_far >= filters.max_per_page: skip(per_page_budget)
            if decision == skip(...):
                annotate images_processed[i]; emit alt-text-only as fallback
                continue
            fetch full image bytes
            caption = captioner.caption(bytes, Some(alt), filters.max_tokens).await
            on caption error: annotate skipped(captioner_error); emit alt-text-only
            on caption ok: replace markdown with ![{caption}]({src}); annotate captioned
```

`partial_fetch_dims` uses `Range: bytes=0-2047` and feeds the bytes to `image::io::Reader::with_format(detected).into_dimensions()`. For formats whose dimensions live outside the first 2 KiB (rare for web images), the probe returns `None` and the dimension gate is treated as "indeterminate → let through".

### 3.7 SPA detection heuristics

`headless::detect::detect_spa(html: &str, extracted_md: &str) -> HitCount` returns:

```rust
pub struct HitCount {
    pub short_extraction: bool,           // extracted_md.chars().count() < 300
    pub spa_marker: bool,                 // see SPA_MARKERS below
    pub high_script_ratio: bool,          // script bytes / total bytes > 0.5
    pub only_anchor_links: bool,          // every <a href> starts with "#/" or is js:void
    pub noscript_js_required: bool,       // <noscript> contents match JS_REQUIRED_RE
    pub total: usize,                     // count of trues
}
```

```rust
const SPA_MARKERS: &[&str] = &[
    "<div id=\"root\"",
    "<div id=\"app\"",
    "<div id=\"__next\"",
    "__NEXT_DATA__",
    "__NUXT__",
    "window.__INITIAL_STATE__",
];

static JS_REQUIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(javascript|enable js|js required|requires javascript)").unwrap()
});
```

`total >= 2` triggers the auto-retry path in `fetch_with_cache`.

### 3.8 CDP interception handler

```rust
// pseudo-rust; real impl in src/fetcher/headless/intercept.rs
async fn handle_paused(
    page: &Page,
    event: EventRequestPaused,
    cfg: &HeadlessAssetConfig,
    ssrf_level: SsrfLevel,
) -> Result<(), CdpError> {
    let url = &event.request.url;
    let req_type = event.resource_type;

    // 1. SSRF gate. Resolve the URL's host, validate IPs against the level.
    if !ssrf::validate_url_for_level(url, ssrf_level).await.is_ok() {
        return page.execute(FulfillRequestParams::new(event.request_id, 200)
            .with_body(""))
            .await;
    }

    // 2. Block-list gate.
    let should_block = match req_type {
        ResourceType::Image  => cfg.block_images,
        ResourceType::Media  => cfg.block_media,
        ResourceType::Font   => cfg.block_fonts,
        ResourceType::Stylesheet => cfg.block_css,
        // ... other resource types
        _ => false,
    } || (cfg.block_third_party && third_party::matches(url, &event.frame_id))
       || (cfg.block_service_workers && req_type == ResourceType::Other && is_sw(event));

    if should_block {
        return page.execute(FulfillRequestParams::new(event.request_id, 200)
            .with_body(""))
            .await;
    }

    // 3. Allow.
    page.execute(ContinueRequestParams::new(event.request_id)).await
}
```

All three gates use `FulfillRequestParams` with status 200 and an empty body. No `FailRequestParams` anywhere.

### 3.9 Cloud captioner shape

```rust
// src/vlm/cloud.rs
pub struct CloudCaptioner {
    name: String,
    client: genai::Client,
    model: String,
}

#[async_trait]
impl VlmCaptioner for CloudCaptioner {
    fn name(&self) -> &str { &self.name }
    fn model_id(&self) -> &str { &self.model }

    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError> {
        // 1. base64-encode the image; detect MIME via `infer` or magic-byte sniff.
        // 2. build a genai ChatRequest with one user message containing
        //    `ChatContent::Image { ... }` and a short caption prompt that
        //    incorporates `alt` as a hint when present.
        // 3. send_chat_request via genai's vision-capable provider path.
        // 4. extract response text; trim; return.
        // Errors mapped: 401/403 → AuthFailed, 429 → RateLimited, 5xx → Unavailable,
        // model error → ModelError. Mirror M7's cloud backend error mapping.
    }
}
```

Caption prompt template (`src/vlm/prompts.rs`, parallels M7's `summarizer/prompts.rs`):

```
Caption this image in a single short sentence. No preamble.
{if alt}Existing alt text (may be unreliable): {alt}{endif}
Respond with the caption only.
```

### 3.10 Local captioner shape

```rust
// src/vlm/local.rs
pub struct MistralRsCaptioner {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
}

#[async_trait]
impl VlmCaptioner for MistralRsCaptioner {
    fn name(&self) -> &str { &self.name }
    fn model_id(&self) -> &str { &self.repo_id }

    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError> {
        let _guard = self.permit.acquire().await?;
        let model = self.model_get_or_load().await?;
        let img = image::load_from_memory(image_bytes)?;
        let messages = MultimodalMessages::new()
            .add_image_message(TextMessageRole::User,
                render_caption_prompt(alt),
                vec![img]);
        let resp = model.send_chat_request(messages).await?;
        Ok(extract_caption(&resp, max_tokens)?)
    }
}
```

The `OnceCell` load mirrors the summarizer's local-load. Errors mapped: load failure → `Unavailable`; OOM/panic → `Unavailable` (`catch_unwind` guard).

### 3.11 Captioner registry

```rust
// src/vlm/mod.rs
pub struct CaptionerRegistry {
    captioners: HashMap<String, Arc<dyn VlmCaptioner>>,
    default: String,
}

impl CaptionerRegistry {
    pub fn get(&self, name: &str) -> Result<Arc<dyn VlmCaptioner>, VlmError> { ... }
    pub fn default_name(&self) -> &str { &self.default }
    pub fn names(&self) -> impl Iterator<Item = &str> { ... }
}

pub fn build(config: &Config) -> Result<CaptionerRegistry, VlmError>;
```

Build rules (parallel to `summarizer::registry::build`):
1. Every `[captioners.<name>]` parses into a concrete captioner.
2. `[image_captions] default` must refer to a configured captioner.
3. `kind = "local"` without `local-vision` compiled in → startup error `captioner_local_feature_not_compiled`.
4. If no `[captioners.*]` blocks are configured but `[image_captions]` is present with a non-default value → startup error `no_captioners_configured`. Otherwise: lazy — `ImagesMode::Caption` calls error at fetch time with `caption_no_captioner_configured`.

### 3.12 Caption cache reuse

Caption rows live in `summary_cache`. The `content_hash` column holds `sha256(image_bytes)`; the `params_hash` column holds:

```
captioner_name + RS + captioner_model_id + RS + max_tokens_str
```

where `RS = U+001E`. Lowercase hex SHA-256 of the joined string. New helper `vlm::cache::lookup` / `vlm::cache::insert` wraps the existing `storage::summaries` module so callers don't reach into `summary_cache` directly.

No schema migration. The table is generic over its keys.

### 3.13 `rover model` CLI

```
$ rover model download Qwen/Qwen3.5-0.8B
downloading Qwen/Qwen3.5-0.8B from HuggingFace…
  config.json                                                4 KB / 4 KB
  tokenizer.json                                         11 MB / 11 MB
  model.safetensors                                     1.6 GB / 1.6 GB
✓ cached at ~/.cache/huggingface/hub/models--Qwen--Qwen3.5-0.8B

$ rover model list
~/.cache/huggingface/hub
  Qwen/Qwen3.5-0.8B               1.6 GB
  HuggingFaceTB/SmolVLM-256M-Instruct   240 MB

$ rover model remove Qwen/Qwen3.5-0.8B
removed ~/.cache/huggingface/hub/models--Qwen--Qwen3.5-0.8B (1.6 GB freed)
```

Implementation:
- `download`: `hf_hub::api::tokio::Api::new()?.model(repo_id).get(file)?` for each well-known file (`config.json`, `tokenizer.json`, `tokenizer_config.json`, `*.safetensors`/`*.gguf`). Driven by a per-format manifest in `cli/model.rs`. Progress to stderr via a `hf_hub::Progress`-impl wrapper.
- `list`: walk `~/.cache/huggingface/hub` (or `HF_HOME`), find `models--*--*` dirs, sum file sizes, render as a table.
- `remove`: resolve `models--<owner>--<repo>` and `std::fs::remove_dir_all`. Confirm if the dir is missing.

The subcommand is compile-gated on `any(feature = "local-inference", feature = "local-vision")` via a `#[cfg(...)]` block in `cli/mod.rs`. When neither feature is enabled, `rover model --help` returns `error: unrecognized subcommand`.

---

## 4. Schema and Config

### 4.1 Schema

No new migrations. M9 reuses `summary_cache` (M7 migration 005) for caption rows via a new `params_hash` shape (see §3.12). The schema is already generic enough — only the codepaths writing into it change.

### 4.2 Config additions

```toml
# Existing in PRD §12, gains M9 keys:
[headless]
auto_detect_spa     = true
default_wait        = "domcontentloaded"     # or "networkidle2"
timeout             = "15s"
block_images        = true
block_fonts         = true
block_media         = true
block_css           = false
block_third_party   = true
block_service_workers = true
max_concurrent      = 4                       # NEW in M9
chrome_executable   = ""                       # NEW in M9; empty = auto-detect

# New in M9:
[image_captions]
default      = "openai"                       # name of a [captioners.<name>] block
max_tokens   = 50
max_per_page = 10                              # cap captioner calls per fetch
min_width    = 200                              # px; skip icons / thumbs / spacers
min_height   = 200                              # px
max_bytes    = "10MiB"                         # skip absurd hero images
max_concurrent = 2                              # captioner-level semaphore

# New in M9 (registries):
[captioners.local]
kind   = "local"                                # requires `local-vision` feature
model  = "HuggingFaceTB/SmolVLM-256M-Instruct"

[captioners.openai]
kind         = "cloud"
provider     = "openai"
model        = "gpt-4o-mini"
api_key_env  = "OPENAI_API_KEY"

# Renamed/Removed:
# [vlm]          ← removed entirely; superseded by [image_captions] + [captioners.*]
```

Local-backend block (already supported by registry-key surface; M9 implements the parser):

```toml
[backends.local]
kind  = "local"                                # requires `local-inference` feature
model = "Qwen/Qwen3.5-0.8B"
```

Defaults preserved:
- `[image_captions]` block is optional. If absent and no captioners configured: `ImagesMode::Caption` errors at fetch time. If absent but at least one `[captioners.*]` exists: implicit defaults (`default = <lex-first captioner>`, other knobs as above).
- `[headless]` block already existed in PRD §12 — defaults stand; M9 adds the two new keys with the defaults above.

### 4.3 Config provenance integration (M8 carry-over)

The M8 settable-key whitelist (`docs/superpowers/specs/2026-05-22-rover-m8-ssrf-diagnostics-design.md` §8.3) gains the following new entries:

```
headless.max_concurrent
headless.chrome_executable
image_captions.default
image_captions.max_tokens
image_captions.max_per_page
image_captions.min_width
image_captions.min_height
image_captions.max_bytes
image_captions.max_concurrent
```

Backend/captioner block fields (provider-style) stay outside the whitelist; users edit those by hand. Matches the M8 decision to keep `[backends.<name>]` fields off the whitelist.

---

## 5. `local-inference`: `LocalMistralRs` Summarizer Backend

### 5.1 Type sketch

```rust
// src/summarizer/local.rs
#[cfg(feature = "local-inference")]
pub struct LocalMistralRs {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
    tokenizer: Tokenizer,
}

#[cfg(feature = "local-inference")]
#[async_trait]
impl SummarizerBackend for LocalMistralRs {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        let _guard = self.permit.acquire().await
            .map_err(|_| BackendError::Unavailable("semaphore closed".into()))?;
        let model = self.model_get_or_load().await?;
        let messages = render_messages(opts, content);                   // re-uses M7 prompts
        let resp = model.send_chat_request(messages).await
            .map_err(map_mistralrs_err)?;
        let text = resp.choices.first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or(BackendError::ModelError("empty response".into()))?
            .clone();
        Ok(text.trim().to_string())
    }
    fn name(&self) -> &str { &self.name }
    fn model_id(&self) -> &str { &self.repo_id }
}
```

### 5.2 Cold load

```rust
async fn model_get_or_load(&self) -> Result<Arc<mistralrs::Model>, BackendError> {
    if let Some(m) = self.model.get() { return Ok(m.clone()); }
    if !hf_cache_has(&self.repo_id) {
        eprintln!(
            "downloading {} from HuggingFace; cached at {} — this may take several minutes",
            self.repo_id, hf_cache_root().display()
        );
    }
    let m = mistralrs::ModelBuilder::new(&self.repo_id)
        .with_auto_isq(mistralrs::IsqBits::Eight)
        .with_logging()
        .build()
        .await
        .map_err(|e| BackendError::Unavailable(format!("model load failed: {e}")))?;
    let arc = Arc::new(m);
    let _ = self.model.set(arc.clone());
    Ok(arc)
}
```

`OnceCell::set` is racy-safe — if two callers race, one wins, the other discards its load. Acceptable (model load is idempotent and infrequent).

### 5.3 Registry integration

`summarizer::registry::build_one` gains a `"local"` arm:

```rust
"local" => {
    #[cfg(not(feature = "local-inference"))]
    return Err(SummarizerError::LocalFeatureNotCompiled);
    #[cfg(feature = "local-inference")]
    {
        let model = cfg.model.as_deref().ok_or_else(|| SummarizerError::BackendUnavailable {
            name: name.to_string(),
            reason: "local backend requires `model`".into(),
        })?;
        Ok(Arc::new(LocalMistralRs::new(name, model, tokenizer)))
    }
}
```

New error variant `SummarizerError::LocalFeatureNotCompiled` maps to the new MCP code `summarizer_local_feature_not_compiled` (§9).

### 5.4 Cargo wiring

```toml
[dependencies]
mistralrs = { version = "0.8.1", optional = true, default-features = false }
# additional features pulled per target:
# - macOS: metal acceleration is opt-in via target-specific deps
[target.'cfg(target_os = "macos")'.dependencies]
mistralrs = { version = "0.8.1", optional = true, default-features = false, features = ["metal"] }

[features]
local-inference = ["dep:mistralrs"]
```

CPU-only on Linux/Windows is the default. CUDA is explicitly out — adding it is a v2 ask gated behind a separate `cuda` feature, documented as future work in `docs/features.md`.

---

## 6. `local-vision` + always-on cloud captioners

### 6.1 Trait

```rust
// src/vlm/mod.rs
#[async_trait]
pub trait VlmCaptioner: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;

    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError>;
}
```

### 6.2 `CloudCaptioner` (always compiled)

`genai 0.4` supports vision-capable models on OpenAI (`gpt-4o`, `gpt-4o-mini`), Anthropic (Claude with vision), and Gemini through the same `ChatRequest` shape with `ChatContent::Image` (image bytes + MIME). For `openai_compat` providers, vision works against any server that implements OpenAI's `chat/completions` vision spec.

`CloudCaptioner::caption` builds a one-message request, sends, and extracts the response text. Error mapping mirrors M7's cloud backend (`map_genai_err`).

### 6.3 `MistralRsCaptioner` (gated by `local-vision`)

See §3.10. Uses the same `mistralrs::ModelBuilder` path as `LocalMistralRs`. The HF cache key differs because the repo id differs — they don't share a loaded `Model`. If both features are enabled, the binary holds two `Arc<Model>` instances at runtime (one text, one vision), each protected by its own semaphore.

### 6.4 Registry

```rust
pub struct CaptionerRegistry { ... }
pub fn build(config: &Config) -> Result<CaptionerRegistry, VlmError>;
```

Construction matches `summarizer::registry::build`. The `kind = "local"` arm is `#[cfg(feature = "local-vision")]`-gated and returns `VlmError::LocalFeatureNotCompiled` otherwise.

### 6.5 Image-caption filter pipeline

```rust
// src/extractor/images.rs
#[derive(Debug, Clone)]
pub struct ImageCaptionFilters {
    pub max_per_page: usize,    // 10
    pub min_width:    u32,      // 200
    pub min_height:   u32,      // 200
    pub max_bytes:    u64,      // 10 * 1024 * 1024
    pub max_tokens:   usize,    // 50
}

enum CaptionDecision {
    Caption,
    Skip(SkipReason),           // BelowMinDimensions | AboveMaxBytes | PerPageBudget | CaptionerError(String)
}

async fn classify(
    src: &str,
    rest: &str,
    http: &reqwest::Client,
    captioned_so_far: usize,
    filters: &ImageCaptionFilters,
) -> CaptionDecision { ... }
```

`<img>` attribute extraction is a tiny regex over `rest` (the `[^)]*` capture from the existing `INLINE_IMG` pattern); the markdown extractor preserves attrs only when they appear in the URL parens — which they generally don't for HTML-sourced images. Fallback: when `rest` has no width/height, the partial-fetch path runs.

### 6.6 Per-image annotation

The frontmatter `images_processed` array (PRD §6.2 pattern, parallel to `tables_transformed`):

```yaml
images_processed:
  - src: ./hero.jpg
    decision: captioned
    captioner: openai
    caption: "A black labrador retriever sitting on a wooden dock."
  - src: ./icon-search.svg
    decision: skipped
    reason: below_min_dimensions
    dimensions: { width: 24, height: 24 }
  - src: ./figure-3.png
    decision: skipped
    reason: per_page_budget
  - src: ./photo-12.jpg
    decision: skipped
    reason: above_max_bytes
    bytes: 18234567
  - src: ./photo-13.jpg
    decision: skipped
    reason: captioner_error
    error: "openai: rate limited"
```

Skipped images fall back to alt-text-only inline (existing M4 behaviour).

### 6.7 Cargo wiring

`mistralrs` is shared between `local-inference` and `local-vision`. `image` is required by the cloud-captioner dimension gate (header-only decode), so it stays a non-optional dep with only the format decoders we need:

```toml
[dependencies]
image = { version = "0.25", default-features = false,
          features = ["png", "jpeg", "webp", "gif"] }
mistralrs = { version = "0.8.1", optional = true, default-features = false }

[features]
local-inference = ["dep:mistralrs"]
local-vision    = ["dep:mistralrs"]              # `image` is always on
headless        = ["dep:chromiumoxide", "dep:base64"]
```

The `image` crate with these four format decoders adds ~80 KB to the default binary — acceptable, and within the §11.4 binary-size budget.

---

## 7. `headless`: `chromiumoxide`-based renderer

### 7.1 Renderer surface

```rust
// src/fetcher/headless/mod.rs
#[cfg(feature = "headless")]
pub struct HeadlessRenderer {
    browser: Browser,
    handler_task: tokio::task::JoinHandle<()>,
    permit: Arc<Semaphore>,
    asset_cfg: HeadlessAssetConfig,
}

#[cfg(feature = "headless")]
impl HeadlessRenderer {
    pub async fn new(cfg: &HeadlessConfig) -> Result<Self, HeadlessError> { ... }
    pub async fn render(&self, url: &Url, ssrf_level: SsrfLevel)
        -> Result<RenderedPage, HeadlessError>;
    pub async fn shutdown(self) { ... }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessMode { Off, On, Auto }

pub struct RenderedPage {
    pub final_url: Url,
    pub html: String,
    pub status: u16,
    pub top_level_har_entry: Option<har::v1_2::Entries>,
}
```

The handler task drives the global `Browser` event loop (`browser.handler()` returned by `chromiumoxide::Browser::launch`). It runs for the binary's lifetime; `shutdown` aborts it and closes the browser.

### 7.2 Browser launch

```rust
let mut cfg = BrowserConfig::builder();
if !cfg_headless.chrome_executable.is_empty() {
    cfg = cfg.chrome_executable(&cfg_headless.chrome_executable);
}
cfg = cfg.request_intercept(true);
let cfg = cfg.build().map_err(HeadlessError::ConfigInvalid)?;
let (browser, handler) = Browser::launch(cfg).await.map_err(HeadlessError::LaunchFailed)?;
```

`chromiumoxide::BrowserConfig::default()` auto-detects an installed Chrome/Chromium on Linux (PATH lookup for `google-chrome`, `chromium`, `chromium-browser`), macOS (standard `/Applications/...` locations), and Windows (Program Files + registry). Override via `chrome_executable`.

### 7.3 Render path

```rust
pub async fn render(&self, url: &Url, ssrf_level: SsrfLevel)
    -> Result<RenderedPage, HeadlessError>
{
    let _guard = self.permit.acquire().await?;
    let page = self.browser.new_page("about:blank").await?;
    page.execute(SetCacheDisabledParams { cache_disabled: true }).await?;
    page.execute(FetchEnableParams::default()).await?;            // turn on Fetch domain

    // spawn an interception task scoped to this page
    let asset_cfg = self.asset_cfg.clone();
    let intercept_task = {
        let page_clone = page.clone();
        let mut events = page.event_listener::<EventRequestPaused>().await?;
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let _ = intercept::handle_paused(&page_clone, event, &asset_cfg, ssrf_level).await;
            }
        })
    };

    let nav_result = page.goto(url.as_str()).await;
    let wait_result = wait_for_condition(&page, &self.asset_cfg.default_wait,
                                         self.asset_cfg.timeout).await;
    let final_url = page.url().await?.parse().unwrap_or_else(|_| url.clone());
    let html = page.content().await?;
    let status = extract_top_level_status(&page).await.unwrap_or(0);
    intercept_task.abort();
    page.close().await?;

    Ok(RenderedPage { final_url, html, status, top_level_har_entry: None })
}
```

`wait_for_condition` switches on `default_wait`: `"domcontentloaded"` → wait for the `DomContentLoadedEvent`; `"networkidle2"` → poll until fewer than 2 in-flight requests for 500 ms. Both subject to `timeout`.

### 7.4 Mode dispatch in `fetch_with_cache`

```rust
match opts.headless_mode {
    HeadlessMode::Off => reqwest_path().await,
    HeadlessMode::On => {
        let r = opts.headless.as_ref().ok_or(FetcherError::HeadlessFeatureNotCompiled)?;
        let page = r.render(url, opts.ssrf_level).await?;
        extract_from_rendered(page)
    }
    HeadlessMode::Auto => {
        let reqwest_result = reqwest_path().await?;
        let extracted = extract_fn(&reqwest_result.body, &reqwest_result.final_url)?;
        if let Some(r) = opts.headless.as_ref() {
            let hits = headless::detect::detect_spa(&reqwest_result.body, &extracted.body_md);
            if hits.total >= 2 {
                let rendered = r.render(url, opts.ssrf_level).await?;
                let re_extracted = extract_fn(&rendered.html, &rendered.final_url)?;
                return Ok(/* with metadata.headless_used = true */);
            }
        }
        Ok(/* reqwest result */)
    }
}
```

When the feature is compiled but no renderer is wired (`opts.headless == None`), `On` errors with `headless_renderer_unavailable`; `Auto` silently keeps the reqwest result with `metadata.headless_used = false`.

### 7.5 Wire arg shape (MCP `fetch`)

Today's `headless: Option<serde_json::Value>` (accept-no-op since M3) becomes a typed `Option<HeadlessArg>`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadlessArg {
    #[serde(default)]
    pub mode: Option<HeadlessModeWire>,        // off | on | auto; default off
    #[serde(default)]
    pub wait: Option<HeadlessWaitWire>,        // domcontentloaded | networkidle2
    #[serde(default)]
    pub timeout_secs: Option<u32>,             // override [headless] timeout
    // The block_* knobs intentionally omitted in v1 — driven by config only.
}
```

When the user passes `mode: "on"` against a binary built without the `headless` feature: error code `headless_feature_not_compiled` returned at tool-call time. `mode: "off"` and the absent case always succeed.

### 7.6 Cargo wiring

```toml
[dependencies]
chromiumoxide = { version = "0.9.1", optional = true, default-features = false,
                  features = ["tokio-runtime"] }
base64 = { version = "0.22", optional = true }       # for FulfillRequestParams body encoding

[features]
headless = ["dep:chromiumoxide", "dep:base64"]
```

---

## 8. `rover doctor` Extensions

### 8.1 Always-compiled extension

`captioners_authenticate` mirrors M8's `backends_authenticate`. For each `[captioners.<name>]` with `kind = "cloud"` and a non-empty `api_key_env`, run a one-image probe (a 1×1 pixel transparent PNG, hardcoded constant) with `max_tokens = 1`. Skip if no cloud captioners with creds. 5s per-captioner timeout.

### 8.2 Feature-gated extensions

Inserted into `doctor::run_all` with `#[cfg(feature = "...")]` blocks:

```rust
#[cfg(feature = "local-inference")]
checks.push(Box::new(checks::LocalInferenceModelCached));

#[cfg(feature = "local-vision")]
checks.push(Box::new(checks::LocalVisionModelCached));

#[cfg(feature = "headless")]
checks.push(Box::new(checks::HeadlessBrowserLaunches));
```

#### 8.2.1 `local_inference_model_cached`

```rust
async fn run(&self, ctx: &CheckCtx) -> CheckReport {
    let local_bes: Vec<_> = ctx.config.backends.iter()
        .filter(|(_, c)| c.kind == "local")
        .collect();
    if local_bes.is_empty() {
        return skip("no [backends.<name>] kind = \"local\" configured");
    }
    for (name, cfg) in local_bes {
        let model = cfg.model.as_deref().unwrap_or("");
        if !hf_cache_has(model) {
            return fail(format!("{name}: model {model} not cached. \
                Run `rover model download {model}`"));
        }
    }
    ok("all configured local-inference backends have cached weights")
}
```

#### 8.2.2 `local_vision_model_cached`

Identical shape, against `[captioners.*]` with `kind = "local"`.

#### 8.2.3 `headless_browser_launches`

```rust
async fn run(&self, ctx: &CheckCtx) -> CheckReport {
    let cfg = ctx.config.headless.to_browser_config();
    let (browser, handler) = match Browser::launch(cfg).await {
        Ok(p) => p,
        Err(e) => return fail(format!("browser launch failed: {e}. \
            See docs/features.md for install instructions.")),
    };
    let _h = tokio::spawn(async move {
        // drain handler events until browser closes
        let mut handler = handler;
        while let Some(_) = handler.next().await {}
    });
    let exec = browser.process().map(|p| p.executable.clone());
    drop(browser);
    ok(format!("browser launched: {}",
        exec.map(|p| p.display().to_string()).unwrap_or("(unknown path)".into())))
}
```

### 8.3 NDJSON shape unchanged

`{check, status, detail?}` per M8. Exit code: 0 iff no `Fail`. `Skip` is non-failing.

---

## 9. Error Model

### 9.1 New per-module errors

```rust
// src/summarizer/error.rs (extends M7's enum)
#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    // ... existing M7 variants
    #[error("local-inference backend requires the `local-inference` cargo feature")]
    LocalFeatureNotCompiled,
}

// src/vlm/error.rs (new module)
#[derive(Debug, thiserror::Error)]
pub enum VlmError {
    #[error("no such captioner: {name}")]
    NoSuchCaptioner { name: String },

    #[error("local captioner requires the `local-vision` cargo feature")]
    LocalFeatureNotCompiled,

    #[error("no captioners configured for image captioning")]
    NoCaptionersConfigured,

    #[error("captioner {name} unavailable: {reason}")]
    Unavailable { name: String, reason: String },

    #[error("captioner {name} rate limited")]
    RateLimited { name: String },

    #[error("captioner {name} auth failed")]
    AuthFailed { name: String },

    #[error("captioner {name} model error: {reason}")]
    ModelError { name: String, reason: String },

    #[error("image decode failed: {0}")]
    ImageDecode(#[from] image::ImageError),

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
}

// src/fetcher/headless/mod.rs (cfg(feature = "headless"))
#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    #[error("browser launch failed: {0}")]
    LaunchFailed(String),

    #[error("browser config invalid: {0}")]
    ConfigInvalid(String),

    #[error("render timeout after {timeout_secs}s on {url}")]
    Timeout { url: String, timeout_secs: u32 },

    #[error("page closed unexpectedly: {0}")]
    PageClosed(String),

    #[error("CDP error: {0}")]
    Cdp(String),
}

// src/fetcher/mod.rs (existing FetcherError gains)
#[error("headless feature not compiled into this binary")]
HeadlessFeatureNotCompiled,
#[error("headless renderer is not wired into this fetcher")]
HeadlessRendererUnavailable,
#[error("headless render failed: {0}")]
Headless(#[from] HeadlessError),
```

### 9.2 MCP code mapping

New stable codes in `src/mcp/envelope.rs`:

```
summarizer_local_feature_not_compiled
captioner_no_such                     # was: vlm_no_such_captioner
captioner_local_feature_not_compiled
captioner_no_captioners_configured
captioner_unavailable
captioner_rate_limited
captioner_auth_failed
captioner_model_error
caption_image_decode_failed
headless_feature_not_compiled
headless_renderer_unavailable
headless_launch_failed
headless_render_timeout
headless_page_closed
```

The `caption_no_captioner_configured` runtime error (raised when `ImagesMode::Caption` is requested but no captioner is configured) maps to `captioner_no_captioners_configured`.

---

## 10. Documentation Deliverables

### 10.1 New: `docs/features.md`

| Section | Contents |
| --- | --- |
| Overview | The three Cargo features and what each enables. Install matrix. |
| `local-inference` | `cargo install rover --features local-inference`. Default model. Memory profile. First-call latency. Swap-model via config. |
| `local-vision` | Same shape. SmolVLM variants (256M/500M/2.2B) and how to switch. |
| `headless` | System Chrome prerequisite. Per-platform install hints (`apt install chromium`, `brew install --cask chromium`, Windows installer link). `chrome_executable` override. SPA detection. Asset blocking. |
| `rover model` | Subcommand reference. HF cache location. Disk-usage notes. |
| Cost control | Per-feature cost notes — local CPU vs. cloud-API quota. Caption-filter knobs. |
| Binary sizes | Expected size matrix: default <25 MB; `+local-inference` ~80 MB; `+local-vision` adds ~5 MB (shares mistralrs); `+headless` ~32 MB; all features ~115 MB. Verified by CI. |
| Cross-platform notes | macOS Metal acceleration (auto). Linux CPU-only. Windows best-effort. No CUDA in v1. |

### 10.2 Modified: `docs/security.md`

New section "Headless asset interception and SSRF":
- The browser issues sub-requests Rover doesn't control directly.
- M9 wires every intercepted sub-request URL through the same `SsrfLevel` validator the top-level fetch uses.
- Sub-requests that would violate the level are fulfilled with an empty 200 response, not aborted (PRD §5.7 mandate).
- The HAR recorder only records the top-level navigation — sub-resources are not in the HAR file.
- Threat model: a malicious page cannot use Rover's headless renderer to scan internal networks via embedded `<iframe>` / `<img>` / `fetch()`. The asset block list (`block_third_party = true` default) also applies.

New section "Local model files":
- `local-inference` and `local-vision` download weights from HuggingFace on first use.
- Weights are stored in `~/.cache/huggingface/hub/` (or `HF_HOME` override).
- Rover does not modify or upload the weights.
- HuggingFace requires no auth for the default models we ship.
- Users pulling private/gated models must set `HF_TOKEN`.

### 10.3 Modified: `docs/configuration.md`

Add the new sections (`[image_captions]`, `[captioners.*]`, the M9 keys under `[headless]`, `kind = "local"` under `[backends.*]`). Add the renamed wire enum (`ImagesArg::Caption` instead of `CaptionVlm`). Update the settable-key list from M8.

### 10.4 Modified: `docs/mcp-tools.md`

Document the typed `headless` arg shape and the `caption` images mode. Document `images.captioner` per-call override.

### 10.5 Modified: `docs/cli.md`

Document `rover model {download|list|remove}`. Note feature-gated visibility.

### 10.6 PRD: drift correction

Two PRD edits land alongside the M9 docs commit:
- §2 V1 feature flag list: rename `vlm` → `local-vision`.
- §7.4: replace "via `mistral.rs`" with "via `mistral.rs` (local) or cloud vision APIs (always-on)".
- §12: rename `[vlm]` section to `[image_captions]` + `[captioners.*]`.

---

## 11. Test Strategy

### 11.1 New integration tests

| Test | Feature gate | Asserts |
| --- | --- | --- |
| `local_inference_smoke::loads_qwen_and_summarizes_short_input` | `local-inference` | Constructs a `LocalMistralRs`, runs `compact` against a 200-word fixture; assert non-empty output. `#[ignore]` by default — opt-in via `cargo test --features local-inference -- --ignored`. |
| `local_inference_smoke::fallback_engages_on_load_failure` | `local-inference` | Construct with a bogus repo id; assert fallback to extractive when `fallback_to_extractive = true`. |
| `headless_smoke::renders_spa_fixture` | `headless` | Serve a Vite + React no-SSR fixture from a wiremock server; fetch with `mode: "on"`; assert rendered HTML contains the React-rendered text. `#[ignore]` by default. |
| `headless_smoke::auto_mode_triggers_on_short_extraction` | `headless` | Fixture serves an empty SPA shell; fetch with `mode: "auto"`; assert `metadata.headless_used == true` in the response. |
| `headless_smoke::block_list_fulfills_not_aborts` | `headless` | Fixture page references a font URL. Set `block_fonts = true`. Assert page render completes (no font-load error) and the font request returned 200 (capturable via wiremock recorder). |
| `headless_ssrf_intercept::rfc1918_subrequest_blocked_at_strict_level` | `headless` | Fixture page (served from a public-ish address) `<img src="http://10.0.0.1/x.png">`. Fetch with `SsrfLevel::Strict`. Assert page renders, the sub-request was intercepted and fulfilled with empty 200, no actual connect was made to 10.0.0.1. |
| `vlm_local_smoke::captions_solid_color_image` | `local-vision` | Load SmolVLM 256M, caption a 256x256 solid-red PNG; assert non-empty caption. `#[ignore]` by default. |
| `vlm_cloud_smoke::wiremock_openai_compat_caption_round_trip` | (always) | wiremock-backed openai_compat vision provider; assert the request shape (`messages[0].content` has an `image_url`-typed entry) and that the response text is returned verbatim. |
| `vlm_cloud_smoke::caption_cache_short_circuits_second_call` | (always) | Two `caption()` calls against the same image+params; second hits cache, no second wiremock request. |
| `images_caption_filters::below_min_dimensions_skipped` | (always) | Markdown with one `<img width="24" height="24">`; assert `images_processed[0].decision = skipped, reason = below_min_dimensions`. |
| `images_caption_filters::above_max_bytes_skipped` | (always) | wiremock serves a 12 MiB image (Content-Length only, no body needed in test); `max_bytes = 10MiB`; assert skip. |
| `images_caption_filters::per_page_budget_respected` | (always) | 15 images, `max_per_page = 3`; assert exactly 3 captioned + 12 skipped with `per_page_budget`. |
| `images_caption_filters::dimension_probe_via_partial_fetch` | (always) | Markdown image without width/height attrs; wiremock serves a 200x200 PNG; assert no skip. |
| `cli_model::download_then_list_then_remove` | `any(local-inference, local-vision)` | Stub the HF API via env override (or use `HF_HOME` to a temp dir); download a tiny test model; assert `list` shows it; assert `remove` clears it. |

### 11.2 Existing tests touched

- `tests/mcp_fetch_*.rs` — gain a case verifying `headless: { mode: "off" }` matches the no-arg default behaviour.
- `src/extractor/images.rs::tests` — extend the existing module with caption-mode tests (cloud captioner only, since the local one is feature-gated).
- `src/mcp/tools/fetch.rs` — `images_mode` translator change (`CaptionVlm` → `Caption`); existing assertion that the schema accepts the value updated.

### 11.3 CI matrix

`smoketest.yml` (the existing nightly workflow, M8) gains three new feature-test jobs:

```yaml
jobs:
  - name: feature-local-inference
    run: cargo test --features local-inference,test-loopback --test local_inference_smoke -- --ignored
  - name: feature-headless
    run: cargo test --features headless,test-loopback --test headless_smoke -- --ignored
  - name: feature-local-vision
    run: cargo test --features local-vision,test-loopback --test vlm_local_smoke -- --ignored
```

The `#[ignore]` gate keeps these out of the merge-path tests (the `--ignored` flag opts in). Acceptable cost for nightly.

### 11.4 Binary-size CI assertion

New CI job (`ci.yml`):

```yaml
- name: binary-size
  run: |
    cargo build --release --no-default-features
    size_bytes=$(stat -c%s target/release/rover || stat -f%z target/release/rover)
    test "$size_bytes" -lt 26214400 || { echo "binary size $size_bytes >= 25 MiB"; exit 1; }
```

26214400 = 25 × 1024 × 1024. PRD §15 mandate. Stripping is on by default in our release profile (carry-over from M1 setup).

### 11.5 Coverage of the no-feature builds

`cargo test --no-default-features --features test-loopback` must keep passing — this is the binary `cargo install rover` produces. All M9 work must be invisible from that vantage point except for:
- The renamed `ImagesArg::Caption` wire value (the new variant is present unconditionally; only its execution requires a captioner).
- The new `[image_captions]` + `[captioners.*]` config sections.
- The new `images_processed` frontmatter array.

These are visible in default builds because cloud captioners ship by default. The local-only paths (`LocalMistralRs`, `MistralRsCaptioner`, `HeadlessRenderer`) are absent.

---

## 12. Crate Dependencies Added

| Crate | Why | Where | Notes |
| --- | --- | --- | --- |
| `mistralrs` | Local LLM + VLM inference | optional; shared by `local-inference` and `local-vision` | `0.8.1`. `default-features = false`. macOS gets `metal` via target-specific deps. |
| `chromiumoxide` | Headless browser via CDP | optional; `headless` only | `0.9.1`. `default-features = false`, `features = ["tokio-runtime"]`. |
| `base64` | Encode empty `FulfillRequestParams` body | optional; `headless` only | `0.22`. |
| `image` | Image dimension probes + decode for local captioner | **always-on** (default features off, only PNG/JPEG/WebP/GIF) | `0.25`. ~80 KB binary cost; needed by cloud-captioner dimension gate. |

No new deps for `rover model` — uses the existing `hf-hub` dep (M3). No new deps for the SPA detector — pure-Rust string matching + the existing `regex` dep.

---

## 13. Acceptance Criteria

1. ✅ `cargo install rover` (no features) produces a binary under 25 MiB. CI job `binary-size` asserts.
2. ✅ `cargo build --no-default-features` succeeds. `cargo test --no-default-features --features test-loopback` passes.
3. ✅ `cargo build --features local-inference` succeeds; `rover model download Qwen/Qwen3.5-0.8B` populates the HF cache; `rover doctor` reports `local_inference_model_cached` as Ok. Tested by `cli_model::download_then_list_then_remove` and a manual smoke covering the doctor check.
4. ✅ `cargo build --features local-vision` succeeds; cloud captioner round-trip works against a wiremock-backed openai_compat vision endpoint. Tested by `vlm_cloud_smoke::wiremock_openai_compat_caption_round_trip`.
5. ✅ `cargo build --features headless` succeeds; against a fixture SPA, `mode: "auto"` triggers re-render and produces non-empty extracted markdown. Tested by `headless_smoke::auto_mode_triggers_on_short_extraction`.
6. ✅ Cargo `--all-features` build succeeds and `cargo test --all-features --features test-loopback` passes (excluding `#[ignore]`-marked tests that require actual model downloads / system Chrome).
7. ✅ Headless sub-requests against RFC1918 addresses at `SsrfLevel::Strict` are intercepted and fulfilled with empty 200, not aborted, with no real TCP connect attempt. Tested by `headless_ssrf_intercept::rfc1918_subrequest_blocked_at_strict_level`.
8. ✅ A `[captioners.openai]` block with `kind = "cloud"` produces captions in default builds (no feature flags). Tested by `vlm_cloud_smoke`.
9. ✅ Three caption-filter knobs (`max_per_page`, dimensions, `max_bytes`) skip the expected images with the expected `images_processed` annotations. Tested by the `images_caption_filters::*` suite.
10. ✅ Local-inference fallback works: a misconfigured `[backends.local]` (bogus repo id) falls back to the extractive backend when `fallback_to_extractive = true`. Tested by `local_inference_smoke::fallback_engages_on_load_failure`.
11. ✅ Five docs deliverables exist with the §10 contents. Lint-level check: each file has the expected top-level `##` sections.

---

## 14. Open Items Deferred to Writing-Plans

These don't change the design; they're plan-level details that need concrete settlement when each task is scaffolded.

1. **`mistralrs` patch-version pin and ISQ quantization choice.** The spec pins `0.8.1`; confirm the latest patch at plan-write time. Pick a default `IsqBits` for text (likely `Eight`) and vision (likely `Four`) based on memory/quality tradeoffs documented in the mistralrs README. Pin in the plan.
2. **`chromiumoxide` initial-page URL.** Spec uses `"about:blank"` before `goto`. Confirm this is the documented pattern in chromiumoxide 0.9.1 examples; alternative is `new_page(url)` directly which races with interceptor setup.
3. **`hf-hub` Progress trait shape.** The spec assumes a `Progress`-impl wrapper for stderr bars. Confirm the 0.4.x API exposes this hook; if not, use a polling loop over the download size.
4. **`genai` vision content shape.** Spec uses `ChatContent::Image` for vision messages. Confirm against `genai 0.4` source — the chat content union has shifted between minor versions in the past.
5. **CPU model load on Linux CI.** The CI feature-test job for `local-inference` runs `cargo test ... -- --ignored`. Decide whether to (a) cache HF models between runs in a GitHub Actions cache, (b) use a much smaller model (e.g. `HuggingFaceTB/SmolLM2-135M-Instruct`) in CI specifically, or (c) keep this nightly-only. Pin in the plan.
6. **Third-party block list source.** Spec references "minimal EasyList-derived". Either bake a tiny domain list into the binary (~50 lines) or vendor a small subset of EasyList. The 50-line hardcoded list is the path of least resistance; confirm at plan time.
7. **`Browser::process()` API.** The doctor check pulls the resolved executable path from `browser.process()`. Confirm the public API in chromiumoxide 0.9.1 — if not exposed, log only the `chrome_executable` config value.
8. **`InlineSummarizeArgs` and the renamed `Caption` mode** — verify the `images_mode` translator in `fetch.rs` and the existing `ImagesArg::CaptionVlm` rename don't break any wire-shape tests added in M4/M7.
9. **Wire shape for `images.captioner` override.** Decide between (a) extending `ImagesArg::Caption` with an optional `captioner` field, (b) hoisting `captioner` to the top-level `ImagesArg` (parallels `summarize.backend`). Recommend (a) for locality.
10. **OS-specific Chrome install hints in `docs/features.md`.** Verify the apt/brew/registry paths at doc-write time. Out-of-date install hints are worse than no hints.

---

## 15. Decision Log

| # | Decision | Why |
| - | -------- | --- |
| 1 | `mistralrs = "0.8.1"`, unified `ModelBuilder` for text and vision | One dep, one loading path, two backends. Cargo feature unification means enabling both `local-inference` and `local-vision` compiles the crate once. |
| 2 | System Chrome, not bundled | ~150 MB binary cost vs. the alternative of trusting users to install Chrome (most already have it). Doctor check + clear install hints make the gap actionable. |
| 3 | Hybrid model distribution (auto-download + `rover model download`) | Auto-only blocks first call for minutes; explicit-only is hostile UX. Both paths cover the spectrum cleanly. |
| 4 | Rename `vlm` → `local-vision` | `vlm` is a TLA; `local-vision` parallels `local-inference` and describes the feature. PRD divergence noted in the spec and corrected in the docs commit. |
| 5 | Rename `ImagesArg::CaptionVlm` → `ImagesArg::Caption` | The variant has never executed (M4 stubbed it). Renaming pre-M9 ship is the right time. |
| 6 | Cloud captioners always compiled in | The `genai` dep is already present (M7). Default builds get cloud captioning at no binary-size cost. Strong UX win. |
| 7 | `CaptionerRegistry` mirrors `SummarizerRegistry` | One mental model, one validation pattern, one extension story. |
| 8 | Caption rows reuse `summary_cache` | The table is generic over its keys. No new migration. |
| 9 | Three caption filters: `max_per_page`, dimensions, `max_bytes` | Sane defaults so unaware users don't burn quota. Document-order pick avoids fetching all images. Display-dimension trust matches "is it an icon" intent. |
| 10 | Image-dimension probes via partial fetch | Cheaper than full image fetch when HTML attrs are absent. Goes through the SSRF/rate-limit/HAR machinery for consistency. |
| 11 | `FulfillRequestParams` with empty 200 — never `FailRequestParams` | PRD §5.7 mandate. SPAs error hard on failed CSS/font requests. |
| 12 | Headless sub-requests pass through the SSRF validator | Closes a real privacy hole: without this, headless renders could reach RFC1918 from a `Strict` config. |
| 13 | One shared `Browser` per process; page-level concurrency capped by semaphore | Cheaper than browser-per-render. Bounded concurrency keeps the renderer's memory profile predictable. |
| 14 | HAR records top-level navigation only | Sub-resource entries would balloon the file and obscure what Rover returned. |
| 15 | `OnceCell` cold-load for local models | Model load is expensive and idempotent. First call pays, subsequent calls warm. Matches mistralrs's intended pattern. |
| 16 | `image` is always-on (not feature-gated) | Cloud captioner's dimension gate needs it. ~80 KB binary cost is acceptable. |
| 17 | macOS Metal acceleration only via target-specific dep, CUDA off entirely | Default users get CPU. macOS gets Metal automatically. CUDA is a v2 ask gated behind a separate feature. Keeps the matrix small. |
| 18 | `rover model` is a top-level subcommand | List/remove also belong there. Cleaner than tucking under `doctor` or `cache`. |
| 19 | `--model <hf_repo_id>` per-call flag deferred | Multi-backend config covers the swap use-case. Adding the flag is a one-line PR if a user asks. |
| 20 | New cargo features and config block names recorded as PRD drift correction | The PRD and the milestone manifest are corrected alongside this milestone, not bypassed. |


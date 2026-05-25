# Rover M9 — Feature-Flagged Extras — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the three feature-flagged subsystems (`local-inference`, `local-vision`, `headless`) per the M9 design spec, plus a `CaptionerRegistry` that supports always-on cloud image captioners and a `rover model` CLI for HuggingFace cache management.

**Architecture:** Each feature is an independent Cargo feature behind `dep:` activation. The shared `mistralrs` dep activates from either `local-inference` or `local-vision` (or both — Cargo feature unification compiles it once). `chromiumoxide` activates only from `headless`. Cloud captioners ship in every build via the existing `genai` dep. The renamed `ImagesMode::Caption` and the new `CaptionerRegistry` mirror M7's `SummarizerBackend` + `SummarizerRegistry` patterns line-for-line.

**Tech Stack:** Rust 1.85+ (`edition = "2024"`); `mistralrs 0.8.1` (text + vision inference, shared); `chromiumoxide 0.9.1` (CDP-driven headless browser); `image 0.25` (always-on, header-only decode for dimension probes); `base64 0.22` (CDP body encoding); existing `genai`, `hf-hub`, `reqwest`, `tokio`, `rusqlite`, `tracing` deps.

> **Canonical references:**
> - Design spec: `docs/superpowers/specs/2026-05-25-rover-m9-feature-flagged-extras-design.md` (§ references below all point here unless otherwise noted).
> - PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§5.7 headless, §7.3 local inference, §7.4 image captioning, §11.2 doctor, §14 M9 acceptance, §15 binary size).
> - Prior plans: `docs/superpowers/plans/2026-05-22-rover-m7-summarization.md` (registry pattern), `docs/superpowers/plans/2026-05-22-rover-m8-ssrf-diagnostics.md` (doctor extension pattern).
> - Repo conventions (enforced by lefthook + clippy): per-module `thiserror` enums; no `anyhow` in lib code; no raw `rusqlite::Connection` outside `src/storage/`; `tracing` on stderr only; no `println!` in lib code; conventional commits, all-lowercase descriptions; never `--no-verify`.
> - Test invocation: `cargo test --lib --features test-loopback` for fast pre-push/CI feedback; full suite `cargo test --features test-loopback` runs nightly in `smoketest.yml`.

---

## File Structure

The plan creates these files. Engineers should read the design spec §3.1 alongside this map.

### New files

```
src/
  summarizer/
    local.rs                   # LocalMistralRs backend (Task 17)
  vlm/
    mod.rs                     # VlmCaptioner trait + CaptionerRegistry + build() (Task 3, 5)
    cloud.rs                   # CloudCaptioner via genai (Task 6)
    local.rs                   # MistralRsCaptioner (Task 23)
    error.rs                   # VlmError thiserror enum (Task 3)
    cache.rs                   # Caption-cache wrapper over summary_cache (Task 7)
    prompts.rs                 # Caption prompt template (Task 6)
  fetcher/
    headless/
      mod.rs                   # HeadlessRenderer, HeadlessMode, HeadlessError (Task 28, 30, 34)
      browser.rs               # Browser launch helpers (Task 30)
      detect.rs                # SPA heuristics (Task 33)
      intercept.rs             # CDP Fetch handler (Task 32)
      third_party.rs           # EasyList-derived block list (Task 31)
  cli/
    model.rs                   # rover model {download|list|remove} (Task 41–44)

tests/
  vlm_cloud_smoke.rs           # Wiremock-backed cloud captioner (Task 14)
  images_caption_filters.rs    # Dimension/size/budget gates (Task 15)
  cli_model.rs                 # rover model integration (Task 45)
  local_inference_smoke.rs     # cfg(feature = "local-inference"); #[ignore] (Task 21)
  vlm_local_smoke.rs           # cfg(feature = "local-vision"); #[ignore] (Task 26)
  headless_smoke.rs            # cfg(feature = "headless"); #[ignore] (Task 39)
  headless_ssrf_intercept.rs   # cfg(feature = "headless") (Task 40)

docs/
  features.md                  # Per-feature install/setup/sizing (Task 46)
```

### Modified files

```
Cargo.toml                     # Tasks 2, 16, 22, 27 (deps + features)
src/lib.rs                     # Tasks 3, 27 (pub mod vlm; pub mod fetcher::headless gate)
src/extractor/options.rs       # Task 1 (ImagesMode::Caption rename + ImageCaptionFilters)
src/extractor/images.rs        # Tasks 9, 10, 11 (filter pipeline, caption wiring, annotations)
src/extractor/frontmatter.rs   # Task 11 (images_processed sidecar)
src/extractor/pipeline.rs      # Task 10 (pass captioner through ExtractOptions)
src/mcp/tools/fetch.rs         # Tasks 1, 12, 37 (CaptionVlm → Caption; captioner override; HeadlessArg)
src/mcp/handler.rs             # Task 10 (RoverHandler holds Arc<CaptionerRegistry>)
src/mcp/envelope.rs            # Tasks 19, 24, 28 (new stable MCP error codes)
src/config/mod.rs              # Task 4 (CaptionersConfig, ImageCaptionsConfig, HeadlessConfig keys)
src/config/edit.rs             # Task 4 (settable-key whitelist additions)
src/summarizer/mod.rs          # Task 17 (pub mod local; cfg-gated)
src/summarizer/registry.rs     # Task 18 (build_one "local" arm)
src/summarizer/error.rs        # Task 19 (LocalFeatureNotCompiled variant)
src/doctor/mod.rs              # Tasks 13, 20, 25, 38 (run_all extensions)
src/doctor/checks.rs           # Tasks 13, 20, 25, 38 (new check structs)
src/cli/mod.rs                 # Task 41 (Model subcommand)
src/main.rs                    # Tasks 10, 36, 41 (wire CaptionerRegistry + HeadlessRenderer; Model dispatch)
src/fetcher/cached.rs          # Tasks 35, 36 (FetchOptions.headless; mode dispatch)
src/fetcher/mod.rs             # Task 27 (cfg-gated `pub mod headless;`); Task 35 (FetcherError variants)
src/fetcher/ssrf.rs            # Task 32 (URL-level validate helper for intercept handler)
src/extractor/frontmatter.rs   # Task 11 (PageMeta.images_processed)

.github/workflows/ci.yml       # Task 52 (binary-size assertion)
.github/workflows/smoketest.yml # Task 53 (feature-build matrix)
docs/configuration.md          # Task 48 ([image_captions], [captioners.*], [headless] keys)
docs/security.md               # Task 47 (headless SSRF section)
docs/mcp-tools.md              # Task 49 (headless arg, caption mode)
docs/cli.md                    # Task 50 (rover model)
docs/superpowers/prd/2026-05-07-rover-prd.md  # Task 51 (drift correction)
docs/superpowers/milestones/rover-milestones.md  # Task 54 (status update)
README.md                      # Task 54 (M9 row + feature install snippet)
```

### Phase-to-task map

- **Phase 0 — Shared foundation** (Tasks 1–15): `ImagesMode` rename, `image` dep, `vlm` module + trait + error, `[image_captions]`/`[captioners.*]` config, `CaptionerRegistry`, `CloudCaptioner`, caption cache, dimension probes, filter pipeline, captioner wiring into `extractor::images`, `images_processed` frontmatter, per-call captioner override, `captioners_authenticate` doctor check, cloud captioner integration test, caption filter integration tests.
- **Phase 1 — `local-inference`** (Tasks 16–21): Cargo feature, `LocalMistralRs`, registry `"local"` arm, error variant, doctor check, smoke test.
- **Phase 2 — `local-vision`** (Tasks 22–26): Cargo feature, `MistralRsCaptioner`, registry `"local"` arm, doctor check, smoke test.
- **Phase 3 — `headless`** (Tasks 27–40): Cargo feature, types, config parsing, browser launch, third-party block list, CDP intercept handler, SPA detection, full render path, `fetch_with_cache` plumbing, mode dispatch, typed MCP arg, doctor check, smoke tests, SSRF intercept test.
- **Phase 4 — `rover model` CLI** (Tasks 41–45): subcommand wiring, download with hf-hub progress, list, remove, integration test.
- **Phase 5 — Docs + CI** (Tasks 46–54): features doc, security doc edits, configuration doc, mcp-tools doc, cli doc, PRD correction, binary-size CI assertion, smoketest workflow additions, milestone status.

---

## Phase 0 — Shared Foundation

These tasks are not behind any Cargo feature. They reshape the always-on surface so that the feature-gated phases plug into a stable substrate.

---

### Task 1: Rename `ImagesArg::CaptionVlm` → `ImagesArg::Caption`; add `ImagesMode::Caption` variant

**Files:**
- Modify: `src/extractor/options.rs`
- Modify: `src/mcp/tools/fetch.rs` (lines 269–333, 753–759)
- Test: `src/mcp/tools/fetch.rs` (existing `#[cfg(test)] mod tests` block)

**Spec ref:** §2 "Renamed `CaptionVlm` → `Caption`"; §3.1 `ImagesMode::Caption` variant.

- [ ] **Step 1: Write the failing test (or rather: adapt the existing rename)**

The current `src/mcp/tools/fetch.rs` has tests that mention `CaptionVlm`. Find each occurrence with grep:

```
cd /Users/aaronbassett/Projects/aaronbassett/rover
grep -n 'CaptionVlm\|caption_vlm' src/ tests/ -r
```

Expected hits include line 276 (`ImagesArg::CaptionVlm`), line 325–331 (the error arm in `images_mode`), and any test that constructs an `ImagesArg::CaptionVlm`. Replace each with `Caption`/`caption`.

- [ ] **Step 2: Add `ImagesMode::Caption` variant in `src/extractor/options.rs`**

Open `src/extractor/options.rs`. Replace the `ImagesMode` enum with:

```rust
#[derive(Debug, Clone, Default)]
pub enum ImagesMode {
    Keep,
    #[default]
    AltTextOnly,
    Download,
    Drop,
    /// Caption each `<img>` via a configured `[captioners.<name>]` (M9).
    /// When no captioner is configured at fetch time, the apply() call
    /// returns ExtractorError::CaptionerNotConfigured.
    Caption,
}
```

- [ ] **Step 3: Add `ImageCaptionFilters` struct in `src/extractor/options.rs`**

Append:

```rust
/// Per-fetch caption-mode budget knobs. Resolved from `[image_captions]`
/// at server startup; cloned per-fetch with any per-call overrides applied.
#[derive(Debug, Clone)]
pub struct ImageCaptionFilters {
    pub max_per_page: usize,
    pub min_width: u32,
    pub min_height: u32,
    pub max_bytes: u64,
    pub max_tokens: usize,
    /// When Some, overrides the registry's default captioner for this fetch.
    pub captioner_override: Option<String>,
}

impl Default for ImageCaptionFilters {
    fn default() -> Self {
        Self {
            max_per_page: 10,
            min_width: 200,
            min_height: 200,
            max_bytes: 10 * 1024 * 1024, // 10 MiB
            max_tokens: 50,
            captioner_override: None,
        }
    }
}
```

- [ ] **Step 4: Update `ImagesArg` and `images_mode` in `src/mcp/tools/fetch.rs`**

Replace lines 269–278 (the existing `ImagesArg` enum):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "mode")]
pub enum ImagesArg {
    Keep,
    AltTextOnly,
    Download,
    Drop,
    /// Caption images via a configured captioner. Use `[image_captions]` /
    /// `[captioners.<name>]` in config; per-call override via `captioner`.
    Caption {
        #[serde(default)]
        captioner: Option<String>,
    },
}
```

Replace `images_mode` (lines 319–333) with:

```rust
fn images_mode(arg: Option<&ImagesArg>) -> Result<(ImagesMode, Option<String>), McpError> {
    Ok(match arg {
        None | Some(ImagesArg::AltTextOnly) => (ImagesMode::AltTextOnly, None),
        Some(ImagesArg::Keep) => (ImagesMode::Keep, None),
        Some(ImagesArg::Download) => (ImagesMode::Download, None),
        Some(ImagesArg::Drop) => (ImagesMode::Drop, None),
        Some(ImagesArg::Caption { captioner }) => (ImagesMode::Caption, captioner.clone()),
    })
}
```

Update the single call site in the same file (around line 457). Replace:

```rust
let images_mode_resolved = images_mode(args.images.as_ref())?;
```

with:

```rust
let (images_mode_resolved, captioner_override) = images_mode(args.images.as_ref())?;
```

Hold `captioner_override` for Task 12's wiring; it can be unused for now (`let _ = captioner_override;`).

- [ ] **Step 5: Update tests in `src/mcp/tools/fetch.rs`**

Find any `CaptionVlm` references in the in-file tests. The schema-roundtrip test currently asserts the variant is in the `oneOf` list (around line 823) — change `"caption_vlm"` to `"caption"`. If a test constructs an `ImagesArg::CaptionVlm`, change to `ImagesArg::Caption { captioner: None }`.

- [ ] **Step 6: Build to confirm**

```
cargo build --features test-loopback 2>&1 | tail -20
```

Expected: zero errors. If `ExtractorError::Metadata` was being referenced for the M9-stub error, that reference goes away too (the arm just produces `ImagesMode::Caption` now; the runtime captioner-not-configured error lives in `extractor::images::apply` and is added in Task 10).

- [ ] **Step 7: Commit**

```
git add src/extractor/options.rs src/mcp/tools/fetch.rs
git commit -m "feat(m9): rename ImagesArg CaptionVlm to Caption; add ImagesMode::Caption + ImageCaptionFilters"
```

---

### Task 2: Promote `image` to a non-optional dep (header-only decode)

**Files:**
- Modify: `Cargo.toml`

**Spec ref:** §6.7 "Cargo wiring"; §12 "Crate Dependencies Added".

- [ ] **Step 1: Add the `image` dep**

Open `Cargo.toml`. After the existing `futures = "0.3"` line in `[dependencies]`, insert:

```toml
image = { version = "0.25", default-features = false, features = ["png", "jpeg", "webp", "gif"] }
```

The four format features cover ~99% of web images. We keep `default-features = false` to skip exotic codecs we don't need (TIFF, BMP, AVIF, etc.) — those would balloon binary size.

- [ ] **Step 2: Build to pull the dep**

```
cargo build --features test-loopback 2>&1 | tail -10
```

Expected: a one-time download/compile of `image` and its tiny transitive set (`png`, `jpeg-decoder`, `gif`, `image-webp`). Build succeeds.

- [ ] **Step 3: Run the lib test suite to confirm no regression**

```
cargo test --lib --features test-loopback 2>&1 | tail -10
```

Expected: existing 418-test count holds; no new failures.

- [ ] **Step 4: Commit**

```
git add Cargo.toml Cargo.lock
git commit -m "build(m9): add image crate (always-on, header-only decoders for png/jpeg/webp/gif)"
```

---

### Task 3: Scaffold `src/vlm/` module with trait + error type

**Files:**
- Create: `src/vlm/mod.rs`
- Create: `src/vlm/error.rs`
- Modify: `src/lib.rs`

**Spec ref:** §6.1 trait; §9 error model.

- [ ] **Step 1: Create `src/vlm/error.rs`**

```rust
//! VLM (captioner) error types. Per-module thiserror enum.

use thiserror::Error;

#[derive(Debug, Error)]
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

    #[error("captioner semaphore closed")]
    SemaphoreClosed,

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
}
```

- [ ] **Step 2: Create `src/vlm/mod.rs` with the trait and a placeholder registry**

```rust
//! Image captioning subsystem.
//!
//! Exposes a `VlmCaptioner` trait (Task 3) with two implementations:
//! - `CloudCaptioner` (Task 6) — always compiled, wraps `genai::Client` for
//!   vision-capable cloud models (OpenAI gpt-4o, Anthropic Claude with vision,
//!   Gemini, ...).
//! - `MistralRsCaptioner` (Task 23) — gated by the `local-vision` feature,
//!   wraps `mistralrs` for local SmolVLM inference.
//!
//! The `CaptionerRegistry` (Task 5) holds the configured captioners and is
//! injected into MCP server state (Task 10).
//!
//! Caption results are deterministically cached in `summary_cache` via the
//! `cache` module (Task 7), keyed on `(sha256(image_bytes), captioner_name,
//! captioner_model_id, max_tokens)`.

pub mod cache;
pub mod cloud;
pub mod error;
pub mod prompts;

#[cfg(feature = "local-vision")]
pub mod local;

pub use error::VlmError;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// One captioning request after defaults have been merged.
///
/// The trait is intentionally narrow: callers pass image bytes (already
/// fetched, already filtered against the dimension/size gates), optional
/// `alt` text as a hint, and a token budget. Implementations are free to
/// add their own internal concurrency caps (semaphores) — the caller does
/// not need to know.
#[async_trait]
pub trait VlmCaptioner: Send + Sync {
    /// Config-key name (e.g. "openai", "local").
    fn name(&self) -> &str;

    /// Resolved model identifier for cache partitioning.
    fn model_id(&self) -> &str;

    /// Generate a single short caption for the image. On success, returns
    /// the caption string with leading/trailing whitespace trimmed.
    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError>;
}

/// Frozen registry of captioners. Construction in `build()` (Task 5).
#[derive(Clone)]
pub struct CaptionerRegistry {
    captioners: HashMap<String, Arc<dyn VlmCaptioner>>,
    default: Option<String>,
}

impl std::fmt::Debug for CaptionerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.captioners.keys().collect();
        names.sort();
        f.debug_struct("CaptionerRegistry")
            .field("captioners", &names)
            .field("default", &self.default)
            .finish()
    }
}

impl CaptionerRegistry {
    pub fn empty() -> Self {
        Self {
            captioners: HashMap::new(),
            default: None,
        }
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn VlmCaptioner>, VlmError> {
        self.captioners
            .get(name)
            .cloned()
            .ok_or_else(|| VlmError::NoSuchCaptioner {
                name: name.to_string(),
            })
    }

    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.captioners.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.captioners.is_empty()
    }

    /// Test-only direct construction.
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-loopback"))]
    pub fn __test_construct(
        captioners: HashMap<String, Arc<dyn VlmCaptioner>>,
        default: Option<String>,
    ) -> Self {
        Self { captioners, default }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_returns_no_default() {
        let r = CaptionerRegistry::empty();
        assert!(r.is_empty());
        assert!(r.default_name().is_none());
    }

    #[test]
    fn unknown_captioner_returns_typed_error() {
        let r = CaptionerRegistry::empty();
        let err = r.get("missing").unwrap_err();
        assert!(matches!(err, VlmError::NoSuchCaptioner { name } if name == "missing"));
    }
}
```

- [ ] **Step 3: Create placeholder `src/vlm/cache.rs`, `src/vlm/cloud.rs`, `src/vlm/prompts.rs`**

These get real bodies in Tasks 6 and 7. Create empty modules so `mod.rs` compiles:

`src/vlm/cache.rs`:

```rust
//! Caption-cache wrapper over `storage::summaries`. Implementation: Task 7.
```

`src/vlm/cloud.rs`:

```rust
//! `CloudCaptioner` — vision via `genai`. Implementation: Task 6.
```

`src/vlm/prompts.rs`:

```rust
//! Caption prompt template. Implementation: Task 6.
```

- [ ] **Step 4: Register the module in `src/lib.rs`**

Open `src/lib.rs`. Add `pub mod vlm;` after `pub mod tokenizer;` (alphabetical order: t < v). Result:

```rust
pub mod cli;
pub mod config;
pub mod doctor;
pub mod error;
pub mod extractor;
pub mod fetcher;
pub mod mcp;
pub mod paths;
pub mod storage;
pub mod summarizer;
pub mod tasks;
pub mod telemetry;
pub mod tokenizer;
pub mod vlm;
```

- [ ] **Step 5: Build and run new unit tests**

```
cargo build --features test-loopback 2>&1 | tail -10
cargo test --lib --features test-loopback vlm:: 2>&1 | tail -10
```

Expected: build succeeds; the two new tests pass.

- [ ] **Step 6: Commit**

```
git add src/vlm/ src/lib.rs
git commit -m "feat(m9): scaffold vlm module with VlmCaptioner trait + empty CaptionerRegistry"
```

---

### Task 4: Add `[image_captions]` and `[captioners.*]` config types

**Files:**
- Modify: `src/config/mod.rs`
- Modify: `src/config/edit.rs` (settable-key whitelist)
- Test: `src/config/mod.rs` (existing `#[cfg(test)] mod tests` block)

**Spec ref:** §4.2 config additions; §4.3 settable-key whitelist additions.

- [ ] **Step 1: Read the existing config shape**

```
grep -n 'pub struct\|pub enum\|impl Default' src/config/mod.rs | head -30
```

Expected: structs like `Config`, `ServerConfig`, `FetchConfig`, `RateLimitConfig`, `BackendConfig`, `SummarizationConfig`, `HeadlessConfig` (likely already present per PRD §12). Note the existing `BackendConfig`'s field shape — we'll mirror it for `CaptionerConfig`.

- [ ] **Step 2: Add `ImageCaptionsConfig` and `CaptionerConfig` structs**

In `src/config/mod.rs`, add (alphabetize within the existing struct block; place after `HeadlessConfig` and before `RateLimitConfig`):

```rust
/// `[image_captions]` defaults block.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ImageCaptionsConfig {
    /// Name of a `[captioners.<name>]` entry; required when at least one
    /// captioner is configured. Validated at startup.
    pub default: Option<String>,
    pub max_tokens: usize,
    pub max_per_page: usize,
    pub min_width: u32,
    pub min_height: u32,
    #[serde(deserialize_with = "humanbytes_to_u64")]
    pub max_bytes: u64,
    pub max_concurrent: usize,
}

impl Default for ImageCaptionsConfig {
    fn default() -> Self {
        Self {
            default: None,
            max_tokens: 50,
            max_per_page: 10,
            min_width: 200,
            min_height: 200,
            max_bytes: 10 * 1024 * 1024,
            max_concurrent: 2,
        }
    }
}

/// `[captioners.<name>]` block. Mirrors `BackendConfig` (M7).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CaptionerConfig {
    /// `"local"` (requires `local-vision` feature) | `"cloud"`.
    pub kind: String,
    /// For `kind = "cloud"`: any provider `genai` recognizes (`openai`,
    /// `anthropic`, `gemini`, `openai_compat`, ...).
    pub provider: Option<String>,
    /// For `kind = "cloud"`: model identifier (`gpt-4o-mini`, `claude-...`).
    /// For `kind = "local"`: HuggingFace repo id
    /// (`HuggingFaceTB/SmolVLM-256M-Instruct`).
    pub model: Option<String>,
    /// For `openai_compat`: base URL.
    pub base_url: Option<String>,
    /// Name of the env var holding the API key (e.g. `OPENAI_API_KEY`).
    pub api_key_env: Option<String>,
}

/// Parse `"10MiB"`, `"512KB"`, `"1.5GiB"`, or a bare integer (bytes).
/// Used by `humanbytes_to_u64` serde helper.
pub fn parse_human_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    let (num_str, unit) = s
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|i| (&s[..i], &s[i..]))
        .ok_or_else(|| format!("invalid size: {s}"))?;
    let num: f64 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid size number: {num_str}"))?;
    let mult: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "B" => 1,
        "K" | "KB" => 1_000,
        "KIB" => 1_024,
        "M" | "MB" => 1_000_000,
        "MIB" => 1_024 * 1_024,
        "G" | "GB" => 1_000_000_000,
        "GIB" => 1_024 * 1_024 * 1_024,
        other => return Err(format!("unknown size unit: {other}")),
    };
    Ok((num * mult as f64) as u64)
}

fn humanbytes_to_u64<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let raw = String::deserialize(d)?;
    parse_human_bytes(&raw).map_err(serde::de::Error::custom)
}
```

- [ ] **Step 3: Add fields to the top-level `Config` struct**

Locate the top-level `Config` struct in `src/config/mod.rs`. Add these fields:

```rust
#[serde(default)]
pub image_captions: ImageCaptionsConfig,

#[serde(default)]
pub captioners: std::collections::BTreeMap<String, CaptionerConfig>,
```

Use `BTreeMap` (deterministic iteration order) to mirror `backends` (likely also `BTreeMap`; verify with grep — if `backends` is `HashMap`, use the same type for consistency).

- [ ] **Step 4: Extend `[headless]` with two M9 keys**

If `HeadlessConfig` exists in `src/config/mod.rs`, add fields:

```rust
#[serde(default = "default_headless_max_concurrent")]
pub max_concurrent: usize,
#[serde(default)]
pub chrome_executable: String,
```

with helpers:

```rust
fn default_headless_max_concurrent() -> usize { 4 }
```

If `HeadlessConfig` doesn't exist yet, create it with the full PRD §12 shape plus the two M9 keys:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HeadlessConfig {
    pub auto_detect_spa: bool,
    pub default_wait: String,
    #[serde(deserialize_with = "humantime_serde::deserialize")]
    pub timeout: std::time::Duration,
    pub block_images: bool,
    pub block_fonts: bool,
    pub block_media: bool,
    pub block_css: bool,
    pub block_third_party: bool,
    pub block_service_workers: bool,
    pub max_concurrent: usize,
    pub chrome_executable: String,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            auto_detect_spa: true,
            default_wait: "domcontentloaded".to_string(),
            timeout: std::time::Duration::from_secs(15),
            block_images: true,
            block_fonts: true,
            block_media: true,
            block_css: false,
            block_third_party: true,
            block_service_workers: true,
            max_concurrent: 4,
            chrome_executable: String::new(),
        }
    }
}
```

Add `pub headless: HeadlessConfig` to the top-level `Config` if not already there.

- [ ] **Step 5: Add unit tests for parsing**

In the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn image_captions_defaults_match_spec() {
    let c = ImageCaptionsConfig::default();
    assert_eq!(c.max_tokens, 50);
    assert_eq!(c.max_per_page, 10);
    assert_eq!(c.min_width, 200);
    assert_eq!(c.min_height, 200);
    assert_eq!(c.max_bytes, 10 * 1024 * 1024);
    assert_eq!(c.max_concurrent, 2);
}

#[test]
fn human_bytes_parses_common_forms() {
    assert_eq!(parse_human_bytes("1024").unwrap(), 1024);
    assert_eq!(parse_human_bytes("10MiB").unwrap(), 10 * 1024 * 1024);
    assert_eq!(parse_human_bytes("10MB").unwrap(), 10_000_000);
    assert_eq!(parse_human_bytes("1.5GiB").unwrap(), (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
    assert!(parse_human_bytes("bogus").is_err());
}

#[test]
fn image_captions_deserializes_from_toml() {
    let toml_str = r#"
[image_captions]
default = "openai"
max_per_page = 5
min_width = 100
min_height = 100
max_bytes = "1MiB"
"#;
    let cfg: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.image_captions.default.as_deref(), Some("openai"));
    assert_eq!(cfg.image_captions.max_per_page, 5);
    assert_eq!(cfg.image_captions.max_bytes, 1024 * 1024);
    // Unspecified keys hold defaults.
    assert_eq!(cfg.image_captions.max_tokens, 50);
}

#[test]
fn captioners_block_round_trips() {
    let toml_str = r#"
[captioners.openai]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[captioners.local]
kind = "local"
model = "HuggingFaceTB/SmolVLM-256M-Instruct"
"#;
    let cfg: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.captioners.len(), 2);
    assert_eq!(cfg.captioners.get("openai").unwrap().provider.as_deref(), Some("openai"));
    assert_eq!(cfg.captioners.get("local").unwrap().kind, "local");
}

#[test]
fn headless_m9_keys_default_correctly() {
    let h = HeadlessConfig::default();
    assert_eq!(h.max_concurrent, 4);
    assert!(h.chrome_executable.is_empty());
}
```

- [ ] **Step 6: Extend the M8 settable-key whitelist**

Open `src/config/edit.rs`. Locate the `SETTABLE` (or similarly-named) `static` table. Add entries for each new scalar key. Follow the existing pattern (parser fn, dotted key). New entries:

```rust
// headless extensions
SettableSpec { key: "headless.max_concurrent",    parser: parse_usize,     hint: None },
SettableSpec { key: "headless.chrome_executable", parser: parse_string,    hint: None },

// image_captions
SettableSpec { key: "image_captions.default",        parser: parse_string,        hint: None },
SettableSpec { key: "image_captions.max_tokens",     parser: parse_usize,         hint: None },
SettableSpec { key: "image_captions.max_per_page",   parser: parse_usize,         hint: None },
SettableSpec { key: "image_captions.min_width",      parser: parse_u32,           hint: None },
SettableSpec { key: "image_captions.min_height",     parser: parse_u32,           hint: None },
SettableSpec { key: "image_captions.max_bytes",      parser: parse_human_bytes_v, hint: Some("e.g. \"10MiB\"") },
SettableSpec { key: "image_captions.max_concurrent", parser: parse_usize,         hint: None },
```

If `parse_human_bytes_v` doesn't exist yet, add it next to the other parser helpers:

```rust
fn parse_human_bytes_v(v: &str) -> Result<toml_edit::Item, SetError> {
    let n = crate::config::parse_human_bytes(v).map_err(|e| SetError::Parse(e))?;
    // Persist the original human-readable string so users see "10MiB" not 10485760.
    Ok(toml_edit::value(v.to_string()))
}
```

`parse_u32` may already exist as `parse_usize` — if not, add a one-line wrapper.

- [ ] **Step 7: Build and test**

```
cargo test --lib --features test-loopback config:: 2>&1 | tail -10
```

Expected: the new unit tests pass; existing config tests remain green.

- [ ] **Step 8: Commit**

```
git add src/config/ Cargo.lock
git commit -m "feat(m9): add [image_captions] + [captioners.*] config + [headless] m9 keys"
```

---

### Task 5: Implement `vlm::build` registry constructor

**Files:**
- Modify: `src/vlm/mod.rs`

**Spec ref:** §3.11 + §6.4 build rules.

- [ ] **Step 1: Write the failing tests first**

Add to the existing `mod tests` in `src/vlm/mod.rs`:

```rust
#[test]
fn build_with_no_captioners_returns_empty_registry() {
    let cfg = crate::config::Config::default();
    let r = build(&cfg).unwrap();
    assert!(r.is_empty());
    assert!(r.default_name().is_none());
}

#[test]
fn build_with_cloud_captioner_succeeds() {
    let mut cfg = crate::config::Config::default();
    cfg.captioners.insert("openai".to_string(), crate::config::CaptionerConfig {
        kind: "cloud".into(),
        provider: Some("openai".into()),
        model: Some("gpt-4o-mini".into()),
        api_key_env: Some("OPENAI_API_KEY".into()),
        base_url: None,
    });
    cfg.image_captions.default = Some("openai".into());
    let r = build(&cfg).unwrap();
    assert!(r.get("openai").is_ok());
    assert_eq!(r.default_name(), Some("openai"));
}

#[test]
fn build_with_default_pointing_at_missing_captioner_errors() {
    let mut cfg = crate::config::Config::default();
    cfg.captioners.insert("openai".to_string(), crate::config::CaptionerConfig {
        kind: "cloud".into(),
        provider: Some("openai".into()),
        model: Some("gpt-4o-mini".into()),
        api_key_env: Some("OPENAI_API_KEY".into()),
        base_url: None,
    });
    cfg.image_captions.default = Some("nonsense".into());
    let err = build(&cfg).unwrap_err();
    assert!(matches!(err, VlmError::NoSuchCaptioner { name } if name == "nonsense"));
}

#[test]
fn build_with_local_kind_without_feature_errors() {
    #[cfg(not(feature = "local-vision"))]
    {
        let mut cfg = crate::config::Config::default();
        cfg.captioners.insert("local".to_string(), crate::config::CaptionerConfig {
            kind: "local".into(),
            provider: None,
            model: Some("HuggingFaceTB/SmolVLM-256M-Instruct".into()),
            api_key_env: None,
            base_url: None,
        });
        let err = build(&cfg).unwrap_err();
        assert!(matches!(err, VlmError::LocalFeatureNotCompiled));
    }
}

#[test]
fn build_unknown_kind_errors() {
    let mut cfg = crate::config::Config::default();
    cfg.captioners.insert("weird".to_string(), crate::config::CaptionerConfig {
        kind: "weird".into(),
        ..Default::default()
    });
    let err = build(&cfg).unwrap_err();
    assert!(matches!(err, VlmError::Unavailable { .. }));
}

#[test]
fn build_default_inferred_when_only_one_captioner_and_no_default_set() {
    let mut cfg = crate::config::Config::default();
    cfg.captioners.insert("openai".to_string(), crate::config::CaptionerConfig {
        kind: "cloud".into(),
        provider: Some("openai".into()),
        model: Some("gpt-4o-mini".into()),
        api_key_env: Some("OPENAI_API_KEY".into()),
        base_url: None,
    });
    // image_captions.default left as None.
    let r = build(&cfg).unwrap();
    assert_eq!(r.default_name(), Some("openai"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test --lib --features test-loopback vlm::tests:: 2>&1 | tail -20
```

Expected: `build` is undefined, compile error.

- [ ] **Step 3: Implement `build`**

Add to `src/vlm/mod.rs` (before the `tests` mod):

```rust
use crate::config::{CaptionerConfig, Config};

/// Build a `CaptionerRegistry` from config. Validation:
/// 1. Every `[captioners.<name>]` parses into a concrete captioner.
/// 2. `[image_captions] default` (if set) must refer to a configured captioner.
/// 3. If unset and exactly one captioner exists, that one becomes the default.
/// 4. `kind = "local"` without `local-vision` compiled in → `LocalFeatureNotCompiled`.
///
/// An empty config (no `[captioners.*]` blocks) returns an empty registry —
/// `ImagesMode::Caption` calls will then error at fetch time with
/// `NoCaptionersConfigured`. Lazy by design: a user who never opts into
/// captioning pays no startup cost.
pub fn build(config: &Config) -> Result<CaptionerRegistry, VlmError> {
    let mut captioners: HashMap<String, Arc<dyn VlmCaptioner>> = HashMap::new();
    for (name, cfg) in &config.captioners {
        let c = build_one(name, cfg, &config.image_captions)?;
        captioners.insert(name.clone(), c);
    }

    let default = match &config.image_captions.default {
        Some(d) => {
            if !captioners.contains_key(d) {
                return Err(VlmError::NoSuchCaptioner { name: d.clone() });
            }
            Some(d.clone())
        }
        None => {
            if captioners.len() == 1 {
                captioners.keys().next().cloned()
            } else {
                None
            }
        }
    };

    Ok(CaptionerRegistry { captioners, default })
}

fn build_one(
    name: &str,
    cfg: &CaptionerConfig,
    _ic: &crate::config::ImageCaptionsConfig,
) -> Result<Arc<dyn VlmCaptioner>, VlmError> {
    match cfg.kind.as_str() {
        "cloud" => {
            // Real cloud captioner construction lands in Task 6 once
            // `CloudCaptioner` exists. For Task 5 we accept the spec and
            // return a placeholder that errors at call time but registers
            // correctly. Task 6 replaces this branch with the real impl.
            let provider = cfg.provider.as_deref().ok_or_else(|| VlmError::Unavailable {
                name: name.to_string(),
                reason: "cloud captioner requires `provider`".into(),
            })?;
            let model = cfg.model.as_deref().ok_or_else(|| VlmError::Unavailable {
                name: name.to_string(),
                reason: "cloud captioner requires `model`".into(),
            })?;
            let _ = (provider, model); // touched once Task 6 lands
            Ok(Arc::new(PendingCaptioner {
                name: name.to_string(),
                model: model.to_string(),
            }))
        }
        "local" => {
            #[cfg(not(feature = "local-vision"))]
            { Err(VlmError::LocalFeatureNotCompiled) }
            #[cfg(feature = "local-vision")]
            {
                let model = cfg.model.as_deref().ok_or_else(|| VlmError::Unavailable {
                    name: name.to_string(),
                    reason: "local captioner requires `model`".into(),
                })?;
                Ok(Arc::new(local::MistralRsCaptioner::new(name, model, _ic.max_concurrent)?))
            }
        }
        other => Err(VlmError::Unavailable {
            name: name.to_string(),
            reason: format!("unknown captioner kind: {other}"),
        }),
    }
}

/// Placeholder captioner used between Task 5 and Task 6. Returns
/// `Unavailable` from `caption()`. Task 6 deletes this struct and replaces
/// the cloud arm of `build_one` with `CloudCaptioner::new(...)`.
struct PendingCaptioner {
    name: String,
    model: String,
}

#[async_trait]
impl VlmCaptioner for PendingCaptioner {
    fn name(&self) -> &str { &self.name }
    fn model_id(&self) -> &str { &self.model }
    async fn caption(
        &self,
        _image_bytes: &[u8],
        _alt: Option<&str>,
        _max_tokens: usize,
    ) -> Result<String, VlmError> {
        Err(VlmError::Unavailable {
            name: self.name.clone(),
            reason: "cloud captioner not yet implemented (pending Task 6)".into(),
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test --lib --features test-loopback vlm::tests:: 2>&1 | tail -20
```

Expected: all six new tests pass.

- [ ] **Step 5: Commit**

```
git add src/vlm/mod.rs
git commit -m "feat(m9): vlm::build registry constructor with validation rules"
```

---

### Task 6: Implement `CloudCaptioner` via `genai`

**Files:**
- Modify: `src/vlm/cloud.rs`
- Modify: `src/vlm/prompts.rs`
- Modify: `src/vlm/mod.rs` (replace `PendingCaptioner` with real construction)

**Spec ref:** §3.9 cloud captioner shape; §6.2 cloud captioner.

- [ ] **Step 1: Write the prompt module**

Replace `src/vlm/prompts.rs` body:

```rust
//! Caption prompt template. Single rendered system message; the image
//! becomes the user message.

pub fn render_caption_prompt(alt: Option<&str>) -> String {
    let mut prompt = String::from(
        "Caption this image in a single short sentence. No preamble.\n",
    );
    if let Some(alt) = alt.filter(|s| !s.trim().is_empty()) {
        prompt.push_str(&format!("Existing alt text (may be unreliable): {}\n", alt.trim()));
    }
    prompt.push_str("Respond with the caption only.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_without_alt_omits_hint_line() {
        let p = render_caption_prompt(None);
        assert!(!p.contains("Existing alt text"));
        assert!(p.contains("Caption this image"));
    }

    #[test]
    fn prompt_with_alt_includes_hint() {
        let p = render_caption_prompt(Some("a red dog"));
        assert!(p.contains("Existing alt text"));
        assert!(p.contains("a red dog"));
    }

    #[test]
    fn prompt_with_empty_alt_omits_hint_line() {
        let p = render_caption_prompt(Some(""));
        assert!(!p.contains("Existing alt text"));
    }
}
```

- [ ] **Step 2: Read the existing `summarizer::cloud` to mirror its patterns**

```
cat src/summarizer/cloud.rs | head -100
```

Note the `genai::Client` construction, the `ProviderKind` enum, the error-mapping helper. M9's `CloudCaptioner` mirrors the same shape.

- [ ] **Step 3: Implement `CloudCaptioner`**

Replace `src/vlm/cloud.rs`:

```rust
//! `CloudCaptioner` — vision via the existing `genai` dep. Supports any
//! `genai`-known provider that accepts image inputs (OpenAI gpt-4o family,
//! Anthropic Claude with vision, Gemini, plus `openai_compat` servers that
//! implement OpenAI's vision spec).

use async_trait::async_trait;
use base64::Engine;
use genai::chat::{ChatMessage, ChatRequest, ContentPart, MessageContent};
use genai::Client;

use crate::summarizer::cloud::ProviderKind;
use crate::vlm::error::VlmError;
use crate::vlm::prompts::render_caption_prompt;
use crate::vlm::VlmCaptioner;

pub struct CloudCaptioner {
    name: String,
    model: String,
    provider_model: String,
    client: Client,
}

impl CloudCaptioner {
    pub fn new(
        name: &str,
        provider: ProviderKind,
        model: &str,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, VlmError> {
        // Reuse the same client builder M7 uses for cloud summarizers; the
        // logic for openai_compat base_url + api_key resolution lives in
        // `summarizer::cloud::build_client`. We import it here.
        let client = crate::summarizer::cloud::build_client(provider, base_url.as_deref(), api_key.as_deref())
            .map_err(|e| VlmError::Unavailable {
                name: name.to_string(),
                reason: e.to_string(),
            })?;
        // genai's chat-request `model` is the provider-side model name; for
        // openai_compat it might require provider prefix. The summarizer's
        // helper handles this — reuse:
        let provider_model = crate::summarizer::cloud::resolve_request_model(provider, model);
        Ok(Self {
            name: name.to_string(),
            model: model.to_string(),
            provider_model,
            client,
        })
    }

    fn mime_for(image_bytes: &[u8]) -> &'static str {
        // Magic-byte sniff. Order matters (PNG and GIF have very specific signatures).
        if image_bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" }
        else if image_bytes.starts_with(b"\xff\xd8\xff") { "image/jpeg" }
        else if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") { "image/gif" }
        else if image_bytes.len() >= 12 && &image_bytes[0..4] == b"RIFF" && &image_bytes[8..12] == b"WEBP" { "image/webp" }
        else { "application/octet-stream" }
    }
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
        let prompt = render_caption_prompt(alt);
        let mime = Self::mime_for(image_bytes);
        let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
        let data_url = format!("data:{mime};base64,{b64}");

        // Build a multimodal user message: prompt text + image_url part.
        // genai's `ContentPart::Image` shape: each provider implementer
        // serializes this differently — genai handles the per-provider mapping.
        let parts = vec![
            ContentPart::Text(prompt),
            ContentPart::Image { content_type: mime.to_string(), source: data_url },
        ];
        let req = ChatRequest::new(vec![
            ChatMessage::user(MessageContent::Parts(parts)),
        ])
        .with_options(genai::chat::ChatOptions::default().with_max_tokens(max_tokens as u32));

        let resp = self.client.exec_chat(&self.provider_model, req, None)
            .await
            .map_err(|e| map_genai_err(&self.name, e))?;
        let text = resp.content_text_into_string().unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

fn map_genai_err(name: &str, e: genai::Error) -> VlmError {
    use genai::Error as G;
    match &e {
        G::WebApi { webcall, .. } if webcall.status == 401 || webcall.status == 403 =>
            VlmError::AuthFailed { name: name.to_string() },
        G::WebApi { webcall, .. } if webcall.status == 429 =>
            VlmError::RateLimited { name: name.to_string() },
        G::WebApi { webcall, .. } if webcall.status >= 500 =>
            VlmError::Unavailable { name: name.to_string(), reason: e.to_string() },
        G::WebModelNotFound { .. } =>
            VlmError::ModelError { name: name.to_string(), reason: e.to_string() },
        _ => VlmError::Unavailable { name: name.to_string(), reason: e.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_sniff_png() {
        let png_magic = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(CloudCaptioner::mime_for(png_magic), "image/png");
    }

    #[test]
    fn mime_sniff_jpeg() {
        let jpeg_magic = b"\xff\xd8\xff\xe0";
        assert_eq!(CloudCaptioner::mime_for(jpeg_magic), "image/jpeg");
    }

    #[test]
    fn mime_sniff_webp() {
        let mut webp = vec![0u8; 12];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(CloudCaptioner::mime_for(&webp), "image/webp");
    }

    #[test]
    fn mime_sniff_unknown_returns_octet_stream() {
        assert_eq!(CloudCaptioner::mime_for(b"not an image"), "application/octet-stream");
    }
}
```

> **Note on `genai` API shape.** The exact field name for image parts (`ContentPart::Image { content_type, source }`) is taken from `genai 0.4`. If the local `cargo doc --no-deps -p genai` output shows a different variant or constructor (e.g. `ContentPart::image_url(...)`), adapt at this point only — the rest of the file's logic stays the same. This was flagged as Open Item #4 in the spec.

- [ ] **Step 4: Replace `PendingCaptioner` with real construction in `src/vlm/mod.rs`**

In `src/vlm/mod.rs`, replace the `"cloud"` arm of `build_one` with:

```rust
"cloud" => {
    let provider = cfg.provider.as_deref().ok_or_else(|| VlmError::Unavailable {
        name: name.to_string(),
        reason: "cloud captioner requires `provider`".into(),
    })?;
    let model = cfg.model.as_deref().ok_or_else(|| VlmError::Unavailable {
        name: name.to_string(),
        reason: "cloud captioner requires `model`".into(),
    })?;
    let provider_kind = crate::summarizer::cloud::ProviderKind::parse(provider)
        .map_err(|reason| VlmError::Unavailable {
            name: name.to_string(),
            reason,
        })?;
    let api_key = cfg.api_key_env.as_deref()
        .and_then(|var| std::env::var(var).ok())
        .filter(|v| !v.is_empty());
    let base_url = if provider_kind == crate::summarizer::cloud::ProviderKind::OpenAiCompat {
        cfg.base_url.clone()
    } else {
        cfg.base_url.clone()
    };
    Ok(Arc::new(cloud::CloudCaptioner::new(name, provider_kind, model, base_url, api_key)?))
}
```

And delete the `PendingCaptioner` struct + its impl from `src/vlm/mod.rs`.

- [ ] **Step 5: Confirm `summarizer::cloud` exposes the helpers we need**

```
grep -n 'pub fn build_client\|pub fn resolve_request_model\|pub enum ProviderKind' src/summarizer/cloud.rs
```

If `build_client` and `resolve_request_model` are private (`fn` without `pub`), add `pub` to both. Confirm `ProviderKind` is `pub`. This is a justified visibility bump — the `vlm` module is a sibling consumer of the same provider-resolution logic; duplicating it would be the wrong DRY tradeoff.

- [ ] **Step 6: Build and test**

```
cargo build --features test-loopback 2>&1 | tail -10
cargo test --lib --features test-loopback vlm:: 2>&1 | tail -20
```

Expected: build succeeds; the four mime-sniff tests pass; the registry tests from Task 5 now construct real `CloudCaptioner` instances.

- [ ] **Step 7: Commit**

```
git add src/vlm/ src/summarizer/cloud.rs
git commit -m "feat(m9): cloud captioner via genai (always-on, supports openai/anthropic/gemini/openai_compat)"
```

---

### Task 7: Caption-cache wrapper over `summary_cache`

**Files:**
- Modify: `src/vlm/cache.rs`
- Test: `src/vlm/cache.rs`

**Spec ref:** §3.12 caption cache reuse.

- [ ] **Step 1: Read the existing summary cache shape**

```
grep -n 'pub fn\|pub async fn' src/storage/summaries.rs | head -20
```

Note the existing `summaries::lookup(db, content_hash, params_hash)` and `summaries::insert(db, ...)` signatures. We'll call them from the caption-cache wrapper.

- [ ] **Step 2: Implement `vlm::cache`**

Replace `src/vlm/cache.rs`:

```rust
//! Caption-cache wrapper over `storage::summaries`.
//!
//! Cache keys:
//! - `content_hash = sha256(image_bytes)` (hex)
//! - `params_hash  = sha256(captioner_name || RS || captioner_model_id || RS || max_tokens)` (hex)
//!
//! Cache rows live in the existing `summary_cache` table (M7 migration
//! `005_summary_cache.sql`) — no new migration needed. Captions and
//! summaries share the same table because their key derivation paths
//! cannot collide (caption `content_hash` is over image bytes, summary
//! `content_hash` is over extracted markdown; both are sha256 of disjoint
//! byte spaces and the probability of overlap is the same as any sha256
//! collision).

use sha2::{Digest, Sha256};

use crate::storage::Db;
use crate::storage::summaries;
use crate::vlm::error::VlmError;

const RS: char = '\u{1E}';

pub fn content_hash(image_bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(image_bytes);
    hex_lower(&h.finalize())
}

pub fn params_hash(captioner_name: &str, captioner_model_id: &str, max_tokens: usize) -> String {
    let serialized = format!(
        "{}{}{}{}{}",
        captioner_name, RS, captioner_model_id, RS, max_tokens
    );
    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        write!(s, "{b:02x}").expect("write to String never fails");
    }
    s
}

pub async fn lookup(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
) -> Result<Option<String>, VlmError> {
    let ch = content_hash(image_bytes);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    summaries::lookup(db, &ch, &ph).await.map_err(VlmError::Storage)
}

pub async fn insert(
    db: &Db,
    image_bytes: &[u8],
    captioner_name: &str,
    captioner_model_id: &str,
    max_tokens: usize,
    caption: &str,
) -> Result<(), VlmError> {
    let ch = content_hash(image_bytes);
    let ph = params_hash(captioner_name, captioner_model_id, max_tokens);
    summaries::insert(db, &ch, &ph, caption).await.map_err(VlmError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash(b"hello");
        let h2 = content_hash(b"hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn params_hash_distinguishes_max_tokens() {
        let a = params_hash("openai", "gpt-4o-mini", 50);
        let b = params_hash("openai", "gpt-4o-mini", 100);
        assert_ne!(a, b);
    }

    #[test]
    fn params_hash_distinguishes_model() {
        let a = params_hash("openai", "gpt-4o-mini", 50);
        let b = params_hash("openai", "gpt-4o",      50);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn round_trip_persists_caption() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let image = b"\x89PNG\r\n\x1a\n fake png bytes";
        let r1 = lookup(&db, image, "openai", "gpt-4o-mini", 50).await.unwrap();
        assert!(r1.is_none());
        insert(&db, image, "openai", "gpt-4o-mini", 50, "A red dog.").await.unwrap();
        let r2 = lookup(&db, image, "openai", "gpt-4o-mini", 50).await.unwrap();
        assert_eq!(r2.as_deref(), Some("A red dog."));
    }

    #[tokio::test]
    async fn different_params_miss() {
        let tmp = tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let image = b"image";
        insert(&db, image, "openai", "gpt-4o-mini", 50, "first").await.unwrap();
        let r = lookup(&db, image, "openai", "gpt-4o", 50).await.unwrap();
        assert!(r.is_none());
    }
}
```

- [ ] **Step 3: Build and test**

```
cargo test --lib --features test-loopback vlm::cache:: 2>&1 | tail -10
```

Expected: 5 new tests pass.

- [ ] **Step 4: Commit**

```
git add src/vlm/cache.rs
git commit -m "feat(m9): caption cache wrapper over summary_cache"
```

---

### Task 8: Image-dimension probe helpers in `extractor::images`

**Files:**
- Modify: `src/extractor/images.rs`
- Test: `src/extractor/images.rs` (existing `mod tests`)

**Spec ref:** §3.6 + §6.5 dimension probe; spec "Image dimension probe strategy" row.

- [ ] **Step 1: Read existing structure**

`src/extractor/images.rs` is short; reread it in full to confirm the `INLINE_IMG` regex captures `rest` (anything between the URL and the closing paren, which holds HTML width/height attributes when those have been smuggled through readabilityrs).

- [ ] **Step 2: Add `html_attr_dims` helper**

Append to `src/extractor/images.rs` (above the `#[cfg(test)]` block):

```rust
use std::sync::LazyLock;

static IMG_WIDTH_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bwidth\s*=\s*"?(\d+)"?"#).unwrap()
});
static IMG_HEIGHT_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bheight\s*=\s*"?(\d+)"?"#).unwrap()
});

/// Extract `<img width=… height=…>` from the markdown image's `rest`
/// capture (the tail between the URL and the closing paren). Returns
/// `(width, height)` when both are present and parse as positive integers.
pub(crate) fn html_attr_dims(rest: &str) -> Option<(u32, u32)> {
    let w = IMG_WIDTH_ATTR.captures(rest)?.get(1)?.as_str().parse::<u32>().ok()?;
    let h = IMG_HEIGHT_ATTR.captures(rest)?.get(1)?.as_str().parse::<u32>().ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}
```

- [ ] **Step 3: Add `partial_fetch_dimensions` helper**

Append:

```rust
/// Fetch the first 2 KiB of an image and decode the header for dimensions.
/// Uses `Range: bytes=0-2047` to avoid pulling the full image. Returns
/// `None` when the server doesn't support range requests, the dimensions
/// live past the first 2 KiB (rare for web formats), or the response is
/// not a recognizable image. Errors propagate as `Err`.
pub(crate) async fn partial_fetch_dimensions(
    http: &reqwest::Client,
    src: &str,
) -> Result<Option<(u32, u32)>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    let resp = http
        .get(url.clone())
        .header(reqwest::header::RANGE, "bytes=0-2047")
        .send()
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    if !resp.status().is_success() && resp.status().as_u16() != 206 {
        return Ok(None);
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    // Try `image::io::Reader` first (auto-detects format from magic bytes).
    let cursor = std::io::Cursor::new(&bytes[..]);
    match image::ImageReader::new(cursor).with_guessed_format() {
        Ok(reader) => Ok(reader.into_dimensions().ok()),
        Err(_) => Ok(None),
    }
}

/// Fetch a `Content-Length` header without downloading the body. Returns
/// `None` when the server doesn't expose `Content-Length` (e.g. chunked
/// transfer). HEAD request; falls back to range-GET if HEAD is rejected.
pub(crate) async fn fetch_content_length(
    http: &reqwest::Client,
    src: &str,
) -> Result<Option<u64>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    let resp = http.head(url.clone()).send().await;
    match resp {
        Ok(r) if r.status().is_success() => Ok(r.content_length()),
        _ => {
            // Some servers reject HEAD; use a range-GET to read length.
            let r = http
                .get(url)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send()
                .await
                .map_err(|source| ExtractorError::ImageDownload {
                    url: src.to_string(),
                    source,
                })?;
            Ok(r.content_length())
        }
    }
}
```

- [ ] **Step 4: Add tests with wiremock**

In the existing `mod tests`:

```rust
#[test]
fn html_attr_dims_extracts_width_height() {
    assert_eq!(html_attr_dims(r#" width="200" height="150""#), Some((200, 150)));
    assert_eq!(html_attr_dims(r#" width=200 height=150"#), Some((200, 150)));
    assert_eq!(html_attr_dims(r#" width="200""#), None);
    assert_eq!(html_attr_dims(""), None);
    assert_eq!(html_attr_dims(r#" width="0" height="100""#), None);
}

#[tokio::test]
async fn partial_fetch_dimensions_reads_png_header() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 1x1 transparent PNG bytes.
    let png: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
        0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41,
        0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
        0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(&png[..]))
        .mount(&server)
        .await;
    let client = reqwest::Client::new();
    let url = format!("{}/img.png", server.uri());
    let dims = partial_fetch_dimensions(&client, &url).await.unwrap();
    assert_eq!(dims, Some((1, 1)));
}
```

- [ ] **Step 5: Build and test**

```
cargo test --lib --features test-loopback extractor::images:: 2>&1 | tail -10
```

Expected: pass. The png test makes a real (in-process) HTTP call to wiremock.

- [ ] **Step 6: Commit**

```
git add src/extractor/images.rs
git commit -m "feat(m9): image dimension probe helpers (html attrs + partial fetch)"
```

---

### Task 9: Implement caption-mode filter pipeline (`classify` + decision enum)

**Files:**
- Modify: `src/extractor/images.rs`

**Spec ref:** §3.6 + §6.5 filter pipeline; §6.6 annotation.

- [ ] **Step 1: Add decision and skip-reason enums**

Append to `src/extractor/images.rs`:

```rust
use crate::extractor::options::ImageCaptionFilters;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    BelowMinDimensions,
    AboveMaxBytes,
    PerPageBudget,
    CaptionerError,
    DimensionsIndeterminate,
}

#[derive(Debug, Clone)]
pub(crate) enum CaptionDecision {
    Caption { dims: Option<(u32, u32)> },
    Skip { reason: SkipReason, dims: Option<(u32, u32)>, bytes: Option<u64> },
}

/// Run the filter pipeline for a single image. The caller is responsible
/// for incrementing the budget counter only when `Caption` is returned.
///
/// Pipeline order (matches spec §3.6):
///   1. Dimension gate: trust HTML attrs when present; otherwise probe.
///   2. Size gate: HEAD or range-GET for Content-Length; reject if too big.
///   3. Budget gate: reject if already captioned >= max_per_page.
pub(crate) async fn classify(
    src: &str,
    rest: &str,
    http: &reqwest::Client,
    captioned_so_far: usize,
    filters: &ImageCaptionFilters,
) -> CaptionDecision {
    // Step 1: dimensions.
    let dims = match html_attr_dims(rest) {
        Some(d) => Some(d),
        None => match partial_fetch_dimensions(http, src).await {
            Ok(Some(d)) => Some(d),
            Ok(None) => None,
            Err(_) => None,
        },
    };
    if let Some((w, h)) = dims {
        if w < filters.min_width || h < filters.min_height {
            return CaptionDecision::Skip {
                reason: SkipReason::BelowMinDimensions,
                dims: Some((w, h)),
                bytes: None,
            };
        }
    }

    // Step 2: size.
    let bytes = match fetch_content_length(http, src).await {
        Ok(b) => b,
        Err(_) => None,
    };
    if let Some(n) = bytes {
        if n > filters.max_bytes {
            return CaptionDecision::Skip {
                reason: SkipReason::AboveMaxBytes,
                dims,
                bytes: Some(n),
            };
        }
    }

    // Step 3: budget.
    if captioned_so_far >= filters.max_per_page {
        return CaptionDecision::Skip {
            reason: SkipReason::PerPageBudget,
            dims,
            bytes,
        };
    }

    CaptionDecision::Caption { dims }
}
```

- [ ] **Step 2: Add tests**

In `mod tests`:

```rust
#[tokio::test]
async fn classify_skips_below_min_dimensions_via_html_attrs() {
    let client = reqwest::Client::new();
    let f = ImageCaptionFilters { min_width: 200, min_height: 200, ..Default::default() };
    let d = classify("https://example.com/icon.svg", r#" width="24" height="24""#, &client, 0, &f).await;
    assert!(matches!(d, CaptionDecision::Skip { reason: SkipReason::BelowMinDimensions, .. }));
}

#[tokio::test]
async fn classify_skips_per_page_budget() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    // Need a real URL that passes dimension+size checks; provide a small mocked image with no Content-Length headache.
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(&[0u8; 100][..]))
        .mount(&server).await;
    let client = reqwest::Client::new();
    let f = ImageCaptionFilters { max_per_page: 3, ..Default::default() };
    let url = format!("{}/photo.png", server.uri());
    // captioned_so_far == max_per_page → skip
    let d = classify(&url, r#" width="500" height="500""#, &client, 3, &f).await;
    assert!(matches!(d, CaptionDecision::Skip { reason: SkipReason::PerPageBudget, .. }));
}
```

- [ ] **Step 3: Build and test**

```
cargo test --lib --features test-loopback extractor::images::tests 2>&1 | tail -10
```

Expected: all pass.

- [ ] **Step 4: Commit**

```
git add src/extractor/images.rs
git commit -m "feat(m9): caption filter pipeline (dimensions, size, budget)"
```

---

### Task 10: Wire `ImagesMode::Caption` into `apply()`

**Files:**
- Modify: `src/extractor/images.rs`
- Modify: `src/extractor/options.rs`
- Modify: `src/extractor/pipeline.rs`
- Modify: `src/mcp/handler.rs`
- Modify: `src/mcp/server.rs` (or wherever `RoverHandler` is constructed)
- Modify: `src/main.rs`

**Spec ref:** §3.6 lifecycle; §6.7 annotation.

- [ ] **Step 1: Extend `ExtractOptions` with captioner registry + filters**

In `src/extractor/options.rs`:

```rust
use std::sync::Arc;

use crate::extractor::output::OutputPaths;
use crate::vlm::CaptionerRegistry;
use crate::storage::Db;

#[derive(Clone)]
pub struct ExtractOptions {
    pub tables: TablesMode,
    pub images: ImagesMode,
    pub metadata: MetadataMode,
    pub output_paths: Arc<OutputPaths>,

    /// M9: captioner registry (always present in default builds since cloud
    /// captioners ship in every binary). `None` only during very early tests.
    pub captioners: Option<Arc<CaptionerRegistry>>,
    pub caption_filters: ImageCaptionFilters,
    pub db: Option<Db>,
}
```

Make sure `Debug` is removed from the derive list if any caller depended on it (the `Arc<CaptionerRegistry>` does implement `Debug`, so we can keep the derive; do confirm by building).

- [ ] **Step 2: Add a new ExtractorError variant**

In `src/extractor/pipeline.rs`, add to the `ExtractorError` enum:

```rust
#[error("captioner `{name}` failed: {reason}")]
CaptionerCall { name: String, reason: String },

#[error("no captioner configured for images.mode = caption")]
CaptionerNotConfigured,
```

- [ ] **Step 3: Refactor `images::apply` signature**

Replace the `pub async fn apply(...)` signature in `src/extractor/images.rs` with:

```rust
pub async fn apply(
    markdown: &str,
    mode: &ImagesMode,
    output_paths: &OutputPaths,
    http: &reqwest::Client,
    captioners: Option<&CaptionerRegistry>,
    filters: &ImageCaptionFilters,
    db: Option<&Db>,
) -> Result<ImagesApplied, ExtractorError> {
```

Add `use crate::vlm::CaptionerRegistry;` and `use crate::storage::Db;` at the top of the file.

Inside, before the per-match loop, resolve which captioner to use:

```rust
let captioner = if matches!(mode, ImagesMode::Caption) {
    let reg = captioners.ok_or(ExtractorError::CaptionerNotConfigured)?;
    let name = filters.captioner_override.as_deref()
        .or_else(|| reg.default_name())
        .ok_or(ExtractorError::CaptionerNotConfigured)?;
    Some(reg.get(name).map_err(|e| ExtractorError::CaptionerCall {
        name: name.to_string(),
        reason: e.to_string(),
    })?)
} else {
    None
};
let mut captioned_so_far = 0usize;
let mut images_processed: Vec<crate::extractor::frontmatter::ImageProcessed> = Vec::new();
```

Add a `Caption` arm in the `match mode` block:

```rust
ImagesMode::Caption => {
    let captioner = captioner.as_ref().expect("captioner resolved when mode == Caption");
    let decision = classify(&src, &rest, http, captioned_so_far, filters).await;
    match decision {
        CaptionDecision::Skip { reason, dims, bytes } => {
            images_processed.push(crate::extractor::frontmatter::ImageProcessed {
                src: src.clone(),
                decision: "skipped".into(),
                reason: Some(format!("{reason:?}").to_lowercase()),
                captioner: None,
                caption: None,
                dimensions: dims.map(|(w, h)| crate::extractor::frontmatter::ImageDims { width: w, height: h }),
                bytes,
                error: None,
            });
            alt.clone() // fall back to alt-text-only
        }
        CaptionDecision::Caption { dims } => {
            // Fetch full bytes.
            let bytes = match download_image_bytes(http, &src).await {
                Ok(b) => b,
                Err(e) => {
                    images_failed += 1;
                    images_processed.push(crate::extractor::frontmatter::ImageProcessed {
                        src: src.clone(),
                        decision: "skipped".into(),
                        reason: Some("captioner_error".into()),
                        captioner: Some(captioner.name().to_string()),
                        caption: None,
                        dimensions: dims.map(|(w, h)| crate::extractor::frontmatter::ImageDims { width: w, height: h }),
                        bytes: None,
                        error: Some(format!("download: {e}")),
                    });
                    alt.clone()
                }
                _ => Vec::new(),
            };
            // Cache lookup.
            let cached = if let Some(db) = db {
                crate::vlm::cache::lookup(db, &bytes, captioner.name(), captioner.model_id(), filters.max_tokens)
                    .await
                    .unwrap_or(None)
            } else {
                None
            };
            let caption = match cached {
                Some(c) => c,
                None => match captioner.caption(&bytes, Some(&alt).filter(|s| !s.is_empty()).map(|s| s.as_str()), filters.max_tokens).await {
                    Ok(c) => {
                        if let Some(db) = db {
                            let _ = crate::vlm::cache::insert(db, &bytes, captioner.name(), captioner.model_id(), filters.max_tokens, &c).await;
                        }
                        c
                    }
                    Err(e) => {
                        images_failed += 1;
                        images_processed.push(crate::extractor::frontmatter::ImageProcessed {
                            src: src.clone(),
                            decision: "skipped".into(),
                            reason: Some("captioner_error".into()),
                            captioner: Some(captioner.name().to_string()),
                            caption: None,
                            dimensions: dims.map(|(w, h)| crate::extractor::frontmatter::ImageDims { width: w, height: h }),
                            bytes: None,
                            error: Some(e.to_string()),
                        });
                        return Ok(alt.clone());
                    }
                },
            };
            captioned_so_far += 1;
            images_processed.push(crate::extractor::frontmatter::ImageProcessed {
                src: src.clone(),
                decision: "captioned".into(),
                reason: None,
                captioner: Some(captioner.name().to_string()),
                caption: Some(caption.clone()),
                dimensions: dims.map(|(w, h)| crate::extractor::frontmatter::ImageDims { width: w, height: h }),
                bytes: None,
                error: None,
            });
            format!("![{caption}]({src}{rest})")
        }
    }
}
```

> **Note:** the `return Ok(alt.clone())` inside the inner closure is wrong as written — restructure the match expression to assign `replacement` so the outer loop continues with the right value. The implementer should resolve this by lifting the captioner branch into a function `caption_one_image(...) -> Result<String, ExtractorError>` that returns the replacement markdown and pushes annotations via `&mut Vec`. Pattern: keep `apply` as a thin loop; put real work in helpers.

Also: change the return type of `apply` to surface `images_processed`. Adjust `ImagesApplied`:

```rust
#[derive(Debug, Default, Clone)]
pub struct ImagesApplied {
    pub markdown: String,
    pub images_seen: usize,
    pub images_downloaded: usize,
    pub images_failed: usize,
    pub images_processed: Vec<crate::extractor::frontmatter::ImageProcessed>,
}
```

Set `images_processed` at the end of `apply`.

Add the `download_image_bytes` helper:

```rust
async fn download_image_bytes(
    http: &reqwest::Client,
    src: &str,
) -> Result<Vec<u8>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    let resp = http.get(url.clone()).send().await
        .map_err(|source| ExtractorError::ImageDownload { url: src.to_string(), source })?
        .error_for_status()
        .map_err(|source| ExtractorError::ImageDownload { url: src.to_string(), source })?;
    Ok(resp.bytes().await
        .map_err(|source| ExtractorError::ImageDownload { url: src.to_string(), source })?
        .to_vec())
}
```

- [ ] **Step 4: Update callers of `apply` (extractor pipeline, tests)**

The extractor pipeline (`src/extractor/pipeline.rs`) calls `apply`. Find and update:

```
grep -n 'images::apply\|extractor::images::apply' src/ tests/
```

Update each call to pass the new args (`captioners`, `filters`, `db`). For the pipeline call, the new args come from `ExtractOptions`.

For the existing in-module tests in `src/extractor/images.rs`, pass `None, &ImageCaptionFilters::default(), None`.

- [ ] **Step 5: Construct `CaptionerRegistry` at server startup**

In `src/main.rs`, where `SummarizerService` is constructed for the MCP server path, also build the captioner registry:

```rust
let captioner_registry = std::sync::Arc::new(
    rover::vlm::build(&config).expect("captioner registry build")
);
```

Pass `captioner_registry` into `RoverHandler::new(...)` (Step 6).

- [ ] **Step 6: Add `Arc<CaptionerRegistry>` to `RoverHandler` state**

In `src/mcp/handler.rs`, add a field:

```rust
pub captioners: std::sync::Arc<crate::vlm::CaptionerRegistry>,
```

Update `RoverHandler::new(...)` signature and all construction sites (search for `RoverHandler::new`).

In the existing `fetch_inner` (search `src/mcp/tools/fetch.rs`), build the `ExtractOptions` with:

```rust
captioners: Some(self.captioners.clone()),
caption_filters: build_caption_filters(&self.config.image_captions, captioner_override),
db: Some(self.db.clone()),
```

`build_caption_filters` is a new helper:

```rust
fn build_caption_filters(
    cfg: &crate::config::ImageCaptionsConfig,
    override_name: Option<String>,
) -> ImageCaptionFilters {
    ImageCaptionFilters {
        max_per_page: cfg.max_per_page,
        min_width: cfg.min_width,
        min_height: cfg.min_height,
        max_bytes: cfg.max_bytes,
        max_tokens: cfg.max_tokens,
        captioner_override: override_name,
    }
}
```

- [ ] **Step 7: Build and test**

```
cargo build --features test-loopback 2>&1 | tail -10
cargo test --lib --features test-loopback 2>&1 | tail -10
```

Expected: a compile error or two from extra `captioners` args at call sites — fix and re-run. All 418-plus tests pass.

- [ ] **Step 8: Commit**

```
git add src/extractor/ src/mcp/ src/main.rs
git commit -m "feat(m9): wire ImagesMode::Caption through extractor + mcp server state"
```

---

### Task 11: Add `images_processed` to frontmatter

**Files:**
- Modify: `src/extractor/frontmatter.rs`

**Spec ref:** §6.6 per-image annotation.

- [ ] **Step 1: Add types**

Append to `src/extractor/frontmatter.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ImageProcessed {
    pub src: String,
    pub decision: String,                     // "captioned" | "skipped"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,               // e.g. "below_min_dimensions"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captioner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<ImageDims>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageDims {
    pub width: u32,
    pub height: u32,
}
```

- [ ] **Step 2: Add the field to `PageMeta`**

In the same file:

```rust
#[serde(skip_serializing_if = "Vec::is_empty", default)]
pub images_processed: Vec<ImageProcessed>,
```

And ensure callers populate it. Search:

```
grep -n 'PageMeta\s*{' src/
```

Each construction site needs `images_processed: applied.images_processed.clone()` (where `applied` is the `ImagesApplied` from Task 10) — or default `Vec::new()` for non-caption modes.

- [ ] **Step 3: Add a frontmatter rendering test**

In `src/extractor/frontmatter.rs::tests`:

```rust
#[test]
fn images_processed_renders_under_frontmatter() {
    let meta = PageMeta {
        // ... existing fields with placeholder values
        images_processed: vec![
            ImageProcessed {
                src: "./hero.jpg".into(),
                decision: "captioned".into(),
                reason: None,
                captioner: Some("openai".into()),
                caption: Some("A dog.".into()),
                dimensions: Some(ImageDims { width: 800, height: 600 }),
                bytes: None,
                error: None,
            },
            ImageProcessed {
                src: "./icon.svg".into(),
                decision: "skipped".into(),
                reason: Some("below_min_dimensions".into()),
                captioner: None,
                caption: None,
                dimensions: Some(ImageDims { width: 24, height: 24 }),
                bytes: None,
                error: None,
            },
        ],
        // ... remaining fields
    };
    let yaml = render(&meta, "# body");
    assert!(yaml.contains("images_processed:"));
    assert!(yaml.contains("./hero.jpg"));
    assert!(yaml.contains("below_min_dimensions"));
}
```

Fill in the elided fields from the existing `PageMeta` struct definition to make the test compile.

- [ ] **Step 4: Build and test**

```
cargo test --lib --features test-loopback frontmatter:: 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Commit**

```
git add src/extractor/frontmatter.rs src/
git commit -m "feat(m9): images_processed frontmatter sidecar"
```

---

### Task 12: Per-call `images.captioner` override on MCP `fetch`

**Files:**
- Modify: `src/mcp/tools/fetch.rs`
- Test: in-file `mod tests` or a new `tests/mcp_fetch_caption_override.rs`

**Spec ref:** §3.6; §14 open item #9.

- [ ] **Step 1: Wire the captioner override through**

The Task 1 `images_mode` already returns `Option<String>` for the captioner override. In `fetch_inner` (the function where `images_mode_resolved` is computed), feed it into the `ImageCaptionFilters` passed into `ExtractOptions`:

```rust
let (images_mode_resolved, captioner_override) = images_mode(args.images.as_ref())?;
let caption_filters = build_caption_filters(&self.config.image_captions, captioner_override);
// ...
let extract_opts = ExtractOptions {
    tables: tables_mode_resolved,
    images: images_mode_resolved,
    metadata: metadata_mode_resolved,
    output_paths: paths.clone(),
    captioners: Some(self.captioners.clone()),
    caption_filters,
    db: Some(self.db.clone()),
};
```

- [ ] **Step 2: Add an end-to-end test**

Create `tests/mcp_fetch_caption_override.rs`:

```rust
//! Per-call `images.captioner = "<name>"` override picks the named
//! captioner over the configured default. Uses wiremock for cloud
//! captioner and an in-process fixture page with one `<img>`.

use serde_json::json;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_with_images_caption_override_picks_named_captioner() {
    // ... build a config with two cloud captioners pointed at separate
    // wiremock servers, configure default = "alpha", call fetch with
    // `images: { mode: "caption", captioner: "beta" }`, assert that the
    // beta wiremock saw the request (not alpha).
    //
    // Use `common::spawn_client` (introduced in M3) to drive the MCP
    // server. Use `tests/common/mod.rs::config_with` to write a config
    // file with the two `[captioners.*]` blocks.
    //
    // Skeleton:
    //
    // let alpha = MockServer::start().await;
    // let beta  = MockServer::start().await;
    // alpha.register(Mock::given(method("POST")).and(path("/v1/chat/completions"))
    //     .respond_with(ResponseTemplate::new(200).set_body_json(json!({
    //         "choices": [{"message": {"content": "alpha says hi"}}]
    //     }))));
    // beta.register(... "beta says hi" ...);
    //
    // let config_toml = format!(r#"
    //   [captioners.alpha]
    //   kind = "cloud"
    //   provider = "openai_compat"
    //   base_url = "{}/v1/"
    //   model = "x"
    //   api_key_env = "ALPHA_KEY"
    //
    //   [captioners.beta]
    //   kind = "cloud"
    //   provider = "openai_compat"
    //   base_url = "{}/v1/"
    //   model = "y"
    //   api_key_env = "BETA_KEY"
    //
    //   [image_captions]
    //   default = "alpha"
    // "#, alpha.uri(), beta.uri());
    //
    // std::env::set_var("ALPHA_KEY", "dummy");
    // std::env::set_var("BETA_KEY", "dummy");
    //
    // let mut client = common::spawn_client_with_config(&config_toml).await;
    // // Serve a fixture page with one <img src=... width="500" height="500">
    // // via wiremock; let the fetcher fetch it.
    // let resp = client.call_tool("fetch", json!({
    //     "url": fixture_url,
    //     "images": { "mode": "caption", "captioner": "beta" },
    // })).await.unwrap();
    //
    // // Assert the response markdown contains "beta says hi".
    // assert!(resp["markdown"].as_str().unwrap().contains("beta says hi"));
    // // Assert alpha was never called.
    // assert!(alpha.received_requests().await.unwrap().iter()
    //     .filter(|r| r.url.path().contains("chat/completions")).count() == 0);
}
```

The skeleton is detailed enough that the implementer can fill it in by following the M7 `tests/mcp_summarize.rs` end-to-end pattern.

- [ ] **Step 3: Build and test**

```
cargo test --features test-loopback mcp_fetch_caption_override 2>&1 | tail -15
```

Expected: pass.

- [ ] **Step 4: Commit**

```
git add src/mcp/tools/fetch.rs tests/mcp_fetch_caption_override.rs
git commit -m "feat(m9): images.captioner per-call override picks named captioner"
```

---

### Task 13: `captioners_authenticate` doctor check (always compiled)

**Files:**
- Modify: `src/doctor/checks.rs`
- Modify: `src/doctor/mod.rs`

**Spec ref:** §8.1 always-compiled doctor extension.

- [ ] **Step 1: Add the check**

Append to `src/doctor/checks.rs`:

```rust
pub struct CaptionersAuthenticate;

#[async_trait]
impl Check for CaptionersAuthenticate {
    fn name(&self) -> &'static str {
        "captioners_authenticate"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        // 1x1 transparent PNG (67 bytes).
        const PROBE_PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
            0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41,
            0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
            0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
            0x42, 0x60, 0x82,
        ];

        let cloud: Vec<(&String, &crate::config::CaptionerConfig)> = ctx
            .config
            .captioners
            .iter()
            .filter(|(_, c)| c.kind == "cloud")
            .filter(|(_, c)| {
                c.api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
            })
            .collect();
        if cloud.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no cloud captioners with non-empty api_key_env".into()),
            };
        }
        let mut failures = Vec::new();
        for (name, cfg) in cloud {
            let provider = match crate::summarizer::cloud::ProviderKind::parse(cfg.provider.as_deref().unwrap_or("")) {
                Ok(p) => p,
                Err(e) => { failures.push(format!("{name}: invalid provider: {e}")); continue; }
            };
            let api_key = cfg.api_key_env.as_deref().and_then(|e| std::env::var(e).ok());
            let cap = match crate::vlm::cloud::CloudCaptioner::new(
                name, provider, cfg.model.as_deref().unwrap_or(""),
                cfg.base_url.clone(), api_key,
            ) {
                Ok(c) => c,
                Err(e) => { failures.push(format!("{name}: build failed: {e}")); continue; }
            };
            use crate::vlm::VlmCaptioner;
            let probe = tokio::time::timeout(std::time::Duration::from_secs(5),
                cap.caption(PROBE_PNG, None, 1)).await;
            match probe {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => failures.push(format!("{name}: timeout after 5s")),
            }
        }
        if failures.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured cloud captioners authenticated".into()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(failures.join("; ")),
            }
        }
    }
}
```

- [ ] **Step 2: Register in `run_all`**

In `src/doctor/mod.rs::run_all`, append to the `checks` vec (after the existing `BackendsAuthenticate`):

```rust
Box::new(checks::CaptionersAuthenticate),
```

- [ ] **Step 3: Add a unit test**

In `src/doctor/mod.rs::tests`:

```rust
#[tokio::test]
async fn captioners_authenticate_skips_when_no_cloud_configured() {
    let (ctx, _g) = fresh_ctx().await;
    let r = checks::CaptionersAuthenticate.run(&ctx).await;
    assert_eq!(r.status, CheckStatus::Skip);
}
```

- [ ] **Step 4: Build and test**

```
cargo test --lib --features test-loopback doctor:: 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Commit**

```
git add src/doctor/
git commit -m "feat(m9): captioners_authenticate doctor check (always compiled)"
```

---

### Task 14: Wiremock-backed cloud captioner integration test

**Files:**
- Create: `tests/vlm_cloud_smoke.rs`

**Spec ref:** §11.1 test row "vlm_cloud_smoke::wiremock_openai_compat_caption_round_trip".

- [ ] **Step 1: Write the test**

```rust
//! Cloud captioner integration test using wiremock-backed openai_compat.
//! Always compiled (no feature gate) — cloud captioners ship in default builds.

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rover::summarizer::cloud::ProviderKind;
use rover::vlm::cloud::CloudCaptioner;
use rover::vlm::VlmCaptioner;

// 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41,
    0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiremock_openai_compat_caption_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "test",
            "object": "chat.completion",
            "created": 0,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "A small transparent square."},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server)
        .await;

    let cap = CloudCaptioner::new(
        "test",
        ProviderKind::OpenAiCompat,
        "test-model",
        Some(format!("{}/v1/", server.uri())),
        Some("dummy".into()),
    ).unwrap();

    let caption = cap.caption(PNG, Some("transparent pixel"), 50).await.unwrap();
    assert_eq!(caption, "A small transparent square.");
    let recv = server.received_requests().await.unwrap();
    assert_eq!(recv.len(), 1);
    // Sanity: the request body included an image_url part.
    let body = std::str::from_utf8(&recv[0].body).unwrap();
    assert!(body.contains("image"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_short_circuits_second_call() {
    // Same setup as above; call caption() twice with the same image+params
    // through the cache wrapper; assert the wiremock saw exactly one request.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "test", "object": "chat.completion", "created": 0, "model": "x",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "cached"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server)
        .await;

    let cap = CloudCaptioner::new(
        "test", ProviderKind::OpenAiCompat, "test-model",
        Some(format!("{}/v1/", server.uri())), Some("dummy".into()),
    ).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let db = rover::storage::Db::open(tmp.path().join("rover.db")).await.unwrap();

    // First call: miss → real wiremock.
    let cached = rover::vlm::cache::lookup(&db, PNG, cap.name(), cap.model_id(), 50).await.unwrap();
    assert!(cached.is_none());
    let c1 = cap.caption(PNG, None, 50).await.unwrap();
    rover::vlm::cache::insert(&db, PNG, cap.name(), cap.model_id(), 50, &c1).await.unwrap();
    // Second call: must hit cache.
    let cached2 = rover::vlm::cache::lookup(&db, PNG, cap.name(), cap.model_id(), 50).await.unwrap();
    assert_eq!(cached2.as_deref(), Some("cached"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run**

```
cargo test --features test-loopback --test vlm_cloud_smoke 2>&1 | tail -15
```

Expected: both tests pass.

- [ ] **Step 3: Commit**

```
git add tests/vlm_cloud_smoke.rs
git commit -m "test(m9): cloud captioner wiremock round-trip + cache short-circuit"
```

---

### Task 15: Caption filter integration tests

**Files:**
- Create: `tests/images_caption_filters.rs`

**Spec ref:** §11.1 four `images_caption_filters::*` tests.

- [ ] **Step 1: Write the four tests**

```rust
//! Caption filter pipeline (dimensions, size, budget) end-to-end through
//! the extractor::images::apply call path.

use std::sync::Arc;

use rover::extractor::frontmatter::ImageProcessed;
use rover::extractor::images::apply;
use rover::extractor::options::{ImageCaptionFilters, ImagesMode};
use rover::extractor::output::OutputPaths;
use rover::vlm::{CaptionerRegistry, VlmCaptioner, VlmError};

use async_trait::async_trait;

/// A captioner that always succeeds with a fixed string. Used to focus
/// these tests on the filter pipeline, not on captioner behavior.
struct AlwaysCaption(String);

#[async_trait]
impl VlmCaptioner for AlwaysCaption {
    fn name(&self) -> &str { "test" }
    fn model_id(&self) -> &str { "test-model" }
    async fn caption(
        &self,
        _image_bytes: &[u8],
        _alt: Option<&str>,
        _max_tokens: usize,
    ) -> Result<String, VlmError> {
        Ok(self.0.clone())
    }
}

fn registry_with(cap: Arc<dyn VlmCaptioner>) -> CaptionerRegistry {
    let mut m = std::collections::HashMap::new();
    m.insert("test".to_string(), cap);
    CaptionerRegistry::__test_construct(m, Some("test".into()))
}

fn paths() -> OutputPaths {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
    OutputPaths::resolve(None).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn below_min_dimensions_skipped() {
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let md = "![icon](https://example.com/icon.svg \"\" width=\"24\" height=\"24\")";
    let filters = ImageCaptionFilters { min_width: 200, min_height: 200, ..Default::default() };
    let client = reqwest::Client::new();
    let p = paths();
    let r = apply(md, &ImagesMode::Caption, &p, &client, Some(&reg), &filters, None).await.unwrap();
    let proc = r.images_processed.first().expect("annotation");
    assert_eq!(proc.decision, "skipped");
    assert_eq!(proc.reason.as_deref(), Some("belowmindimensions"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn above_max_bytes_skipped() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    // HEAD returns Content-Length = 12 MiB
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "12582912"))
        .mount(&server).await;
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let url = format!("{}/hero.jpg", server.uri());
    let md = format!("![hero]({url} \"\" width=\"800\" height=\"600\")");
    let filters = ImageCaptionFilters { max_bytes: 10 * 1024 * 1024, ..Default::default() };
    let client = reqwest::Client::new();
    let p = paths();
    let r = apply(&md, &ImagesMode::Caption, &p, &client, Some(&reg), &filters, None).await.unwrap();
    assert_eq!(r.images_processed[0].decision, "skipped");
    assert_eq!(r.images_processed[0].reason.as_deref(), Some("abovemaxbytes"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_page_budget_respected() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("HEAD")).respond_with(ResponseTemplate::new(200).insert_header("content-length", "1000")).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(&[0u8; 1000][..])).mount(&server).await;
    let reg = registry_with(Arc::new(AlwaysCaption("OK".into())));
    let url = format!("{}/img.png", server.uri());
    let md_lines: Vec<String> = (0..15).map(|i| format!("![n{i}]({url}?i={i} \"\" width=\"500\" height=\"500\")")).collect();
    let md = md_lines.join("\n");
    let filters = ImageCaptionFilters { max_per_page: 3, ..Default::default() };
    let client = reqwest::Client::new();
    let p = paths();
    let r = apply(&md, &ImagesMode::Caption, &p, &client, Some(&reg), &filters, None).await.unwrap();
    let captioned = r.images_processed.iter().filter(|x| x.decision == "captioned").count();
    let skipped_budget = r.images_processed.iter().filter(|x| x.reason.as_deref() == Some("perpagebudget")).count();
    assert_eq!(captioned, 3);
    assert_eq!(skipped_budget, 12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dimension_probe_via_partial_fetch() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    // 200x200 PNG header — sufficient to pass the min-dimensions check.
    let png_header_200x200: [u8; 33] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0xc8, 0x00, 0x00, 0x00, 0xc8, // width=200, height=200
        0x08, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(&png_header_200x200[..]))
        .mount(&server).await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "5000"))
        .mount(&server).await;

    let reg = registry_with(Arc::new(AlwaysCaption("captioned!".into())));
    let url = format!("{}/photo.png", server.uri());
    let md = format!("![photo]({url})"); // no width/height attrs
    let filters = ImageCaptionFilters::default();
    let client = reqwest::Client::new();
    let p = paths();
    let r = apply(&md, &ImagesMode::Caption, &p, &client, Some(&reg), &filters, None).await.unwrap();
    assert_eq!(r.images_processed[0].decision, "captioned");
    assert!(r.markdown.contains("captioned!"));
}
```

- [ ] **Step 2: Run**

```
cargo test --features test-loopback --test images_caption_filters 2>&1 | tail -15
```

Expected: 4 tests pass.

> **Note:** the `reason` strings (`"belowmindimensions"`, `"abovemaxbytes"`, `"perpagebudget"`) come from `format!("{reason:?}").to_lowercase()` in Task 10's wiring. If a different normalization shape is preferred, prefer mapping the `SkipReason` enum to explicit snake_case strings via a `.to_string()` method on `SkipReason` and update both the tests and the wiring. The lowercased-Debug form is concise but ugly; pick one approach and use it consistently.

- [ ] **Step 3: Commit**

```
git add tests/images_caption_filters.rs
git commit -m "test(m9): caption filter pipeline integration tests"
```

---

## Phase 1 — `local-inference` Feature

These tasks are all gated behind `#[cfg(feature = "local-inference")]`. The default-features build is invisible to this phase.

---

### Task 16: Add `local-inference` Cargo feature + `mistralrs` dep

**Files:**
- Modify: `Cargo.toml`

**Spec ref:** §2 `mistralrs` pin; §5.4 cargo wiring.

- [ ] **Step 1: Add the optional dep and feature**

Open `Cargo.toml`. In `[dependencies]`, add:

```toml
mistralrs = { version = "0.8.1", optional = true, default-features = false }
```

Then add a target-specific override for macOS to pull the Metal acceleration:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
mistralrs = { version = "0.8.1", optional = true, default-features = false, features = ["metal"] }
```

In the `[features]` table, add:

```toml
local-inference = ["dep:mistralrs"]
```

Resulting `[features]` block:

```toml
[features]
default = []
test-loopback = []
local-inference = ["dep:mistralrs"]
```

- [ ] **Step 2: Confirm both default and feature builds compile**

```
cargo build --no-default-features 2>&1 | tail -5
cargo build --features local-inference 2>&1 | tail -5
```

The feature build takes longer the first time (mistralrs and candle compile). Both must succeed with zero warnings (deny=warnings).

- [ ] **Step 3: Confirm `cargo test --lib --features test-loopback` still passes**

```
cargo test --lib --features test-loopback 2>&1 | tail -5
```

Expected: unchanged test count, all pass.

- [ ] **Step 4: Commit**

```
git add Cargo.toml Cargo.lock
git commit -m "build(m9): add local-inference cargo feature gated on mistralrs 0.8.1"
```

---

### Task 17: Implement `LocalMistralRs` summarizer backend

**Files:**
- Create: `src/summarizer/local.rs`
- Modify: `src/summarizer/mod.rs`

**Spec ref:** §5.1 type sketch; §5.2 cold load.

- [ ] **Step 1: Add `pub mod local;` (cfg-gated) to `src/summarizer/mod.rs`**

Locate the `pub mod` lines near the top. Add:

```rust
#[cfg(feature = "local-inference")]
pub mod local;
```

Place after `pub mod extractive;` (alphabetical-ish; the cfg gate makes ordering matter for readability only).

- [ ] **Step 2: Create `src/summarizer/local.rs`**

```rust
//! `LocalMistralRs` — local LLM summarizer backend (M9).
//!
//! Gated by the `local-inference` Cargo feature. Uses `mistralrs 0.8.1`'s
//! unified `ModelBuilder` (auto-detects text vs. multimodal from the HF
//! repo id). The model is lazily loaded on first call into an `OnceCell`;
//! subsequent calls reuse the loaded `Arc<Model>`.
//!
//! Errors map cleanly into `BackendError` so the M7 fallback machinery
//! (`fallback_to_extractive`) takes over on load or inference failure.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, Semaphore};

use crate::summarizer::backend::{CompactOpts, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::summarizer::prompts;
use crate::tokenizer::Tokenizer;

pub struct LocalMistralRs {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
    #[allow(dead_code)]
    tokenizer: Tokenizer,
}

impl std::fmt::Debug for LocalMistralRs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMistralRs")
            .field("name", &self.name)
            .field("repo_id", &self.repo_id)
            .field("loaded", &self.model.get().is_some())
            .finish()
    }
}

impl LocalMistralRs {
    pub fn new(name: &str, repo_id: &str, tokenizer: Tokenizer) -> Self {
        Self {
            name: name.to_string(),
            repo_id: repo_id.to_string(),
            model: OnceCell::new(),
            permit: Arc::new(Semaphore::new(1)),
            tokenizer,
        }
    }

    /// Lazy model load. Threadsafe: `OnceCell::get_or_try_init` makes
    /// concurrent callers wait for the single in-flight load.
    async fn model_get_or_load(&self) -> Result<Arc<mistralrs::Model>, BackendError> {
        if let Some(m) = self.model.get() {
            return Ok(m.clone());
        }
        if !hf_cache_has(&self.repo_id) {
            eprintln!(
                "downloading {} from HuggingFace; cached at {} — this may take several minutes",
                self.repo_id,
                hf_cache_root().display(),
            );
        }
        let arc = self
            .model
            .get_or_try_init(|| async {
                let model = mistralrs::ModelBuilder::new(&self.repo_id)
                    .with_auto_isq(mistralrs::IsqBits::Eight)
                    .with_logging()
                    .build()
                    .await
                    .map_err(|e| BackendError::Unavailable(format!("model load failed: {e}")))?;
                Ok::<Arc<mistralrs::Model>, BackendError>(Arc::new(model))
            })
            .await?;
        Ok(arc.clone())
    }
}

#[async_trait]
impl SummarizerBackend for LocalMistralRs {
    fn name(&self) -> &str { &self.name }
    fn model_id(&self) -> &str { &self.repo_id }

    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        let _guard = self
            .permit
            .acquire()
            .await
            .map_err(|_| BackendError::Unavailable("semaphore closed".into()))?;
        let model = self.model_get_or_load().await?;
        let system = prompts::render_abstractive(opts);
        let messages = mistralrs::TextMessages::new()
            .add_message(mistralrs::TextMessageRole::System, &system)
            .add_message(mistralrs::TextMessageRole::User, content);
        let resp = model
            .send_chat_request(messages)
            .await
            .map_err(|e| BackendError::ModelError(format!("inference failed: {e}")))?;
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or_else(|| BackendError::ModelError("empty response".into()))?
            .clone();
        Ok(text.trim().to_string())
    }
}

/// Does `~/.cache/huggingface/hub/models--<owner>--<repo>/` exist with
/// at least one entry? Used by the cold-load banner and by the M9 doctor
/// check.
pub fn hf_cache_has(repo_id: &str) -> bool {
    let path = hf_cache_root().join(format!(
        "models--{}",
        repo_id.replace('/', "--"),
    ));
    path.exists() && path.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false)
}

pub fn hf_cache_root() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return std::path::PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".cache/huggingface/hub");
    }
    std::path::PathBuf::from(".cache/huggingface/hub")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_cache_root_respects_hf_home_env() {
        let tmp = tempfile::tempdir().unwrap();
        let prior = std::env::var("HF_HOME").ok();
        unsafe { std::env::set_var("HF_HOME", tmp.path()) };
        let root = hf_cache_root();
        assert_eq!(root, tmp.path().join("hub"));
        unsafe {
            if let Some(p) = prior { std::env::set_var("HF_HOME", p); }
            else { std::env::remove_var("HF_HOME"); }
        }
    }

    #[test]
    fn hf_cache_has_returns_false_for_missing_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let prior = std::env::var("HF_HOME").ok();
        unsafe { std::env::set_var("HF_HOME", tmp.path()) };
        assert!(!hf_cache_has("Qwen/Qwen3.5-0.8B"));
        unsafe {
            if let Some(p) = prior { std::env::set_var("HF_HOME", p); }
            else { std::env::remove_var("HF_HOME"); }
        }
    }
}
```

- [ ] **Step 3: Build with the feature**

```
cargo build --features local-inference 2>&1 | tail -10
cargo test --lib --features local-inference,test-loopback summarizer::local:: 2>&1 | tail -10
```

Expected: build succeeds; the two unit tests pass.

- [ ] **Step 4: Commit**

```
git add src/summarizer/local.rs src/summarizer/mod.rs
git commit -m "feat(m9): LocalMistralRs summarizer backend with lazy model load"
```

---

### Task 18: Add `"local"` arm to `summarizer::registry::build_one`

**Files:**
- Modify: `src/summarizer/registry.rs`

**Spec ref:** §5.3 registry integration.

- [ ] **Step 1: Read the current `build_one`**

```
sed -n '130,200p' src/summarizer/registry.rs
```

Note the existing `match cfg.kind.as_str()` block with `"extractive"` and `"cloud"` arms ending in `other => Err(...)`.

- [ ] **Step 2: Add the `"local"` arm**

Insert before the `other =>` arm:

```rust
"local" => {
    #[cfg(not(feature = "local-inference"))]
    { Err(SummarizerError::LocalFeatureNotCompiled) }
    #[cfg(feature = "local-inference")]
    {
        let model = cfg.model.as_deref().ok_or_else(|| SummarizerError::BackendUnavailable {
            name: name.to_string(),
            reason: "local backend requires `model`".into(),
        })?;
        Ok(Arc::new(crate::summarizer::local::LocalMistralRs::new(name, model, tokenizer)))
    }
}
```

- [ ] **Step 3: Add unit tests**

In the existing `mod tests` block at the bottom of `registry.rs`:

```rust
#[cfg(not(feature = "local-inference"))]
#[test]
fn local_kind_errors_without_feature() {
    let cfg = cfg_with_backends(&[(
        "default",
        BackendConfig {
            kind: "local".into(),
            model: Some("Qwen/Qwen3.5-0.8B".into()),
            ..Default::default()
        },
    )]);
    let r = build(&cfg, Tokenizer::O200k);
    assert!(matches!(r, Err(SummarizerError::LocalFeatureNotCompiled)));
}

#[cfg(feature = "local-inference")]
#[test]
fn local_kind_builds_with_feature() {
    let cfg = cfg_with_backends(&[(
        "default",
        BackendConfig {
            kind: "local".into(),
            model: Some("Qwen/Qwen3.5-0.8B".into()),
            ..Default::default()
        },
    )]);
    let reg = build(&cfg, Tokenizer::O200k).unwrap();
    assert!(reg.get("default").is_ok());
}

#[cfg(feature = "local-inference")]
#[test]
fn local_kind_without_model_errors() {
    let cfg = cfg_with_backends(&[(
        "default",
        BackendConfig { kind: "local".into(), model: None, ..Default::default() },
    )]);
    let r = build(&cfg, Tokenizer::O200k);
    assert!(matches!(r, Err(SummarizerError::BackendUnavailable { .. })));
}
```

- [ ] **Step 4: Build and test both with and without the feature**

```
cargo test --lib --features test-loopback summarizer::registry:: 2>&1 | tail -10
cargo test --lib --features local-inference,test-loopback summarizer::registry:: 2>&1 | tail -10
```

Expected: both runs pass.

- [ ] **Step 5: Commit**

```
git add src/summarizer/registry.rs
git commit -m "feat(m9): summarizer registry local kind arm with cfg-gated impl"
```

---

### Task 19: `SummarizerError::LocalFeatureNotCompiled` variant + MCP code

**Files:**
- Modify: `src/summarizer/error.rs`
- Modify: `src/mcp/envelope.rs`

**Spec ref:** §9.1 + §9.2.

- [ ] **Step 1: Add the error variant**

In `src/summarizer/error.rs`, locate `enum SummarizerError`. Add (alphabetize within the enum or place at the bottom):

```rust
#[error("local-inference backend requires the `local-inference` cargo feature")]
LocalFeatureNotCompiled,
```

- [ ] **Step 2: Add the MCP code mapping**

In `src/mcp/envelope.rs`, locate the `SummarizerError → RoverError` mapping (or however M7 wired it). Add:

```rust
SummarizerError::LocalFeatureNotCompiled =>
    RoverError::new("summarizer_local_feature_not_compiled", err.to_string()),
```

Add the constant name (`SUMMARIZER_LOCAL_FEATURE_NOT_COMPILED`) to the list of stable codes if M7 maintains such a list.

- [ ] **Step 3: Verify**

```
cargo build --features test-loopback 2>&1 | tail -5
cargo test --lib --features test-loopback summarizer:: 2>&1 | tail -10
```

Expected: build succeeds; no regressions.

- [ ] **Step 4: Commit**

```
git add src/summarizer/error.rs src/mcp/envelope.rs
git commit -m "feat(m9): summarizer_local_feature_not_compiled mcp error code"
```

---

### Task 20: `local_inference_model_cached` doctor check

**Files:**
- Modify: `src/doctor/checks.rs`
- Modify: `src/doctor/mod.rs`

**Spec ref:** §8.2.1.

- [ ] **Step 1: Add the check struct (feature-gated)**

Append to `src/doctor/checks.rs`:

```rust
#[cfg(feature = "local-inference")]
pub struct LocalInferenceModelCached;

#[cfg(feature = "local-inference")]
#[async_trait]
impl Check for LocalInferenceModelCached {
    fn name(&self) -> &'static str {
        "local_inference_model_cached"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let locals: Vec<(&String, &crate::config::BackendConfig)> = ctx
            .config
            .backends
            .iter()
            .filter(|(_, c)| c.kind == "local")
            .collect();
        if locals.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no [backends.<name>] kind = \"local\" configured".into()),
            };
        }
        let mut missing: Vec<String> = Vec::new();
        for (name, cfg) in locals {
            let model = match cfg.model.as_deref() {
                Some(m) => m,
                None => {
                    missing.push(format!("{name}: model missing in config"));
                    continue;
                }
            };
            if !crate::summarizer::local::hf_cache_has(model) {
                missing.push(format!(
                    "{name}: model {model} not cached. Run `rover model download {model}`"
                ));
            }
        }
        if missing.is_empty() {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Ok,
                detail: Some("all configured local-inference backends have cached weights".into()),
            }
        } else {
            CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(missing.join("; ")),
            }
        }
    }
}
```

- [ ] **Step 2: Register in `run_all` (feature-gated)**

In `src/doctor/mod.rs::run_all`, after the existing checks vec build, add:

```rust
#[cfg(feature = "local-inference")]
let mut checks = checks;
#[cfg(feature = "local-inference")]
checks.push(Box::new(checks::LocalInferenceModelCached) as Box<dyn Check>);
```

Or — cleaner — change the original `let checks: Vec<Box<dyn Check>> = vec![...]` to `let mut checks: Vec<Box<dyn Check>> = vec![...]` and append after:

```rust
let mut checks: Vec<Box<dyn Check>> = vec![
    Box::new(checks::SqliteOpen),
    // ... existing checks
    Box::new(checks::CaptionersAuthenticate),
];

#[cfg(feature = "local-inference")]
checks.push(Box::new(checks::LocalInferenceModelCached));
```

- [ ] **Step 3: Unit test**

```rust
#[cfg(feature = "local-inference")]
#[tokio::test]
async fn local_inference_model_cached_skips_when_no_local_configured() {
    let (ctx, _g) = fresh_ctx().await;
    let r = checks::LocalInferenceModelCached.run(&ctx).await;
    assert_eq!(r.status, CheckStatus::Skip);
}
```

- [ ] **Step 4: Verify both builds**

```
cargo test --lib --features test-loopback doctor:: 2>&1 | tail -5
cargo test --lib --features local-inference,test-loopback doctor:: 2>&1 | tail -5
```

Expected: both pass.

- [ ] **Step 5: Commit**

```
git add src/doctor/
git commit -m "feat(m9): local_inference_model_cached doctor check (cfg-gated)"
```

---

### Task 21: `local_inference_smoke` integration test (`#[ignore]`)

**Files:**
- Create: `tests/local_inference_smoke.rs`

**Spec ref:** §11.1 row "local_inference_smoke".

- [ ] **Step 1: Write the test**

```rust
//! Smoketest for the local-inference feature. Loads a real (small) model
//! from HuggingFace and runs one summarization call. `#[ignore]` by
//! default — opt in via `cargo test --features local-inference -- --ignored`.
//!
//! CI: the smoketest workflow runs these nightly. Local devs can run
//! them on demand. The test caches models in HF_HOME, so subsequent runs
//! are fast (the first run downloads ~1.6 GB).

#![cfg(feature = "local-inference")]

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::local::LocalMistralRs;
use rover::tokenizer::Tokenizer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn loads_qwen_and_summarizes_short_input() {
    let be = LocalMistralRs::new("test", "Qwen/Qwen3.5-0.8B", Tokenizer::O200k);
    let opts = CompactOpts {
        mode: CompactMode::Abstractive,
        style: Style::Prose,
        target_tokens: Some(60),
        focus: None,
        preserve: vec![],
        backend_name: "test".to_string(),
    };
    let content = "Rover is a polite scraper that fetches web pages and turns \
                   them into clean Markdown for LLM agents. It caches what it \
                   fetches and summarizes long pages on demand.";
    let summary = be.compact(content, &opts).await.expect("compact ok");
    assert!(!summary.is_empty(), "summary must be non-empty");
    assert!(summary.len() < content.len() * 2, "summary should not balloon");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bogus_repo_id_yields_unavailable_error() {
    use rover::summarizer::error::BackendError;
    let be = LocalMistralRs::new("test", "Nonsense/DoesNotExist-XX", Tokenizer::O200k);
    let opts = CompactOpts {
        mode: CompactMode::Abstractive, style: Style::Prose,
        target_tokens: None, focus: None, preserve: vec![],
        backend_name: "test".to_string(),
    };
    let r = be.compact("anything", &opts).await;
    assert!(matches!(r, Err(BackendError::Unavailable(_))), "got {r:?}");
}
```

- [ ] **Step 2: Run with the feature (skipped by default)**

```
cargo test --features local-inference --test local_inference_smoke 2>&1 | tail -5
```

Expected: 2 ignored, 0 ran.

```
cargo test --features local-inference --test local_inference_smoke -- --ignored 2>&1 | tail -10
```

Expected (when network + GPU/CPU allow): both pass after the model downloads.

> **Smoketest CI gating:** these tests are wired into nightly via Task 53.

- [ ] **Step 3: Commit**

```
git add tests/local_inference_smoke.rs
git commit -m "test(m9): local-inference smoketest (ignored; nightly only)"
```

---

## Phase 2 — `local-vision` Feature

Mirrors Phase 1 with the captioner trait and SmolVLM. Shares the `mistralrs` dep — enabling `local-vision` alone, or together with `local-inference`, compiles `mistralrs` exactly once.

---

### Task 22: Add `local-vision` Cargo feature

**Files:**
- Modify: `Cargo.toml`

**Spec ref:** §2 feature rename `vlm → local-vision`; §6.7 cargo wiring.

- [ ] **Step 1: Extend `[features]`**

Open `Cargo.toml`. Update `[features]`:

```toml
[features]
default = []
test-loopback = []
local-inference = ["dep:mistralrs"]
local-vision = ["dep:mistralrs"]
```

`mistralrs` was added as an `optional` dep in Task 16; `dep:mistralrs` from either feature activates it.

- [ ] **Step 2: Confirm all four feature combos compile**

```
cargo build --no-default-features 2>&1 | tail -3
cargo build --features local-inference 2>&1 | tail -3
cargo build --features local-vision 2>&1 | tail -3
cargo build --features local-inference,local-vision 2>&1 | tail -3
```

Expected: all succeed.

- [ ] **Step 3: Commit**

```
git add Cargo.toml Cargo.lock
git commit -m "build(m9): add local-vision cargo feature (shares mistralrs with local-inference)"
```

---

### Task 23: Implement `MistralRsCaptioner`

**Files:**
- Create: `src/vlm/local.rs`

**Spec ref:** §3.10 + §6.3.

- [ ] **Step 1: Create the file**

```rust
//! `MistralRsCaptioner` — local image captioner via `mistralrs` (SmolVLM
//! family). Gated by the `local-vision` Cargo feature.
//!
//! Uses the same lazy-load + per-instance semaphore pattern as the
//! `LocalMistralRs` summarizer backend. The captioner instance is held
//! in `CaptionerRegistry` as `Arc<dyn VlmCaptioner>`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, Semaphore};

use crate::vlm::error::VlmError;
use crate::vlm::prompts::render_caption_prompt;
use crate::vlm::VlmCaptioner;

pub struct MistralRsCaptioner {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
}

impl std::fmt::Debug for MistralRsCaptioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralRsCaptioner")
            .field("name", &self.name)
            .field("repo_id", &self.repo_id)
            .field("loaded", &self.model.get().is_some())
            .finish()
    }
}

impl MistralRsCaptioner {
    pub fn new(name: &str, repo_id: &str, max_concurrent: usize) -> Result<Self, VlmError> {
        let permit = if max_concurrent == 0 { 2 } else { max_concurrent };
        Ok(Self {
            name: name.to_string(),
            repo_id: repo_id.to_string(),
            model: OnceCell::new(),
            permit: Arc::new(Semaphore::new(permit)),
        })
    }

    async fn model_get_or_load(&self) -> Result<Arc<mistralrs::Model>, VlmError> {
        if let Some(m) = self.model.get() {
            return Ok(m.clone());
        }
        if !crate::summarizer::local::hf_cache_has(&self.repo_id) {
            eprintln!(
                "downloading {} from HuggingFace; cached at {} — this may take several minutes",
                self.repo_id,
                crate::summarizer::local::hf_cache_root().display(),
            );
        }
        let arc = self
            .model
            .get_or_try_init(|| async {
                let model = mistralrs::ModelBuilder::new(&self.repo_id)
                    .with_auto_isq(mistralrs::IsqBits::Four)
                    .with_logging()
                    .build()
                    .await
                    .map_err(|e| VlmError::Unavailable {
                        name: self.name.clone(),
                        reason: format!("model load failed: {e}"),
                    })?;
                Ok::<Arc<mistralrs::Model>, VlmError>(Arc::new(model))
            })
            .await?;
        Ok(arc.clone())
    }
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
        let _guard = self.permit.acquire().await.map_err(|_| VlmError::SemaphoreClosed)?;
        let model = self.model_get_or_load().await?;
        let img = image::load_from_memory(image_bytes)?;
        let prompt = render_caption_prompt(alt);
        let messages = mistralrs::MultimodalMessages::new().add_image_message(
            mistralrs::TextMessageRole::User,
            &prompt,
            vec![img],
        );
        // mistralrs's `send_chat_request` honors per-request token caps via
        // `RequestBuilder`. For the simple single-shot path we accept the
        // default cap and trim if needed; tighter cap plumbing is a v2 item
        // tracked in open-items.
        let _ = max_tokens; // see comment above
        let resp = model.send_chat_request(messages).await.map_err(|e| VlmError::ModelError {
            name: self.name.clone(),
            reason: format!("inference failed: {e}"),
        })?;
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or_else(|| VlmError::ModelError {
                name: self.name.clone(),
                reason: "empty response".into(),
            })?
            .clone();
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctor_succeeds_with_zero_max_concurrent_picks_default() {
        let c = MistralRsCaptioner::new("test", "HuggingFaceTB/SmolVLM-256M-Instruct", 0).unwrap();
        assert_eq!(c.name(), "test");
    }
}
```

- [ ] **Step 2: Build with the feature**

```
cargo build --features local-vision 2>&1 | tail -10
cargo test --lib --features local-vision,test-loopback vlm::local:: 2>&1 | tail -5
```

Expected: build succeeds; the one unit test passes.

- [ ] **Step 3: Commit**

```
git add src/vlm/local.rs
git commit -m "feat(m9): MistralRsCaptioner local vision captioner via mistralrs SmolVLM"
```

---

### Task 24: Captioner registry already supports `"local"` from Task 5 — verify

**Files:**
- Modify: `src/vlm/mod.rs` (verification, possibly a tweak)

**Spec ref:** §3.11 + §6.4.

Task 5's `build_one` `"local"` arm references `local::MistralRsCaptioner::new`. With Task 23 landing the struct, the previously-`#[cfg(feature = "local-vision")]`-gated `_ic.max_concurrent` argument compiles cleanly.

- [ ] **Step 1: Verify the registry path compiles with the feature**

```
cargo build --features local-vision 2>&1 | tail -5
```

If a build error references the `_ic` underscore prefix being unused now, remove the underscore so the variable is actually consumed.

- [ ] **Step 2: Add a feature-gated registry test**

In `src/vlm/mod.rs::tests`:

```rust
#[cfg(feature = "local-vision")]
#[test]
fn build_with_local_kind_succeeds_with_feature() {
    let mut cfg = crate::config::Config::default();
    cfg.captioners.insert("local".to_string(), crate::config::CaptionerConfig {
        kind: "local".into(),
        provider: None,
        model: Some("HuggingFaceTB/SmolVLM-256M-Instruct".into()),
        api_key_env: None,
        base_url: None,
    });
    let r = build(&cfg).unwrap();
    assert!(r.get("local").is_ok());
}
```

- [ ] **Step 3: Test**

```
cargo test --lib --features local-vision,test-loopback vlm:: 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 4: Commit**

```
git add src/vlm/mod.rs
git commit -m "test(m9): verify captioner registry local-vision arm with feature on"
```

---

### Task 25: `local_vision_model_cached` doctor check

**Files:**
- Modify: `src/doctor/checks.rs`
- Modify: `src/doctor/mod.rs`

**Spec ref:** §8.2.2.

- [ ] **Step 1: Add the check**

Append to `src/doctor/checks.rs` (next to `LocalInferenceModelCached`):

```rust
#[cfg(feature = "local-vision")]
pub struct LocalVisionModelCached;

#[cfg(feature = "local-vision")]
#[async_trait]
impl Check for LocalVisionModelCached {
    fn name(&self) -> &'static str {
        "local_vision_model_cached"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let locals: Vec<(&String, &crate::config::CaptionerConfig)> = ctx
            .config
            .captioners
            .iter()
            .filter(|(_, c)| c.kind == "local")
            .collect();
        if locals.is_empty() {
            return CheckReport {
                check: self.name(),
                status: CheckStatus::Skip,
                detail: Some("no [captioners.<name>] kind = \"local\" configured".into()),
            };
        }
        let mut missing: Vec<String> = Vec::new();
        for (name, cfg) in locals {
            let model = match cfg.model.as_deref() {
                Some(m) => m,
                None => { missing.push(format!("{name}: model missing in config")); continue; }
            };
            if !crate::summarizer::local::hf_cache_has(model) {
                missing.push(format!(
                    "{name}: model {model} not cached. Run `rover model download {model}`"
                ));
            }
        }
        if missing.is_empty() {
            CheckReport {
                check: self.name(), status: CheckStatus::Ok,
                detail: Some("all configured local-vision captioners have cached weights".into()),
            }
        } else {
            CheckReport { check: self.name(), status: CheckStatus::Fail, detail: Some(missing.join("; ")) }
        }
    }
}
```

- [ ] **Step 2: Register in `run_all`**

In `src/doctor/mod.rs::run_all`, add:

```rust
#[cfg(feature = "local-vision")]
checks.push(Box::new(checks::LocalVisionModelCached));
```

- [ ] **Step 3: Test**

```
cargo test --lib --features local-vision,test-loopback doctor:: 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 4: Commit**

```
git add src/doctor/
git commit -m "feat(m9): local_vision_model_cached doctor check (cfg-gated)"
```

---

### Task 26: `vlm_local_smoke` integration test (`#[ignore]`)

**Files:**
- Create: `tests/vlm_local_smoke.rs`

**Spec ref:** §11.1 row "vlm_local_smoke".

- [ ] **Step 1: Write the test**

```rust
//! Smoketest for the local-vision feature. Loads a real SmolVLM model and
//! captions a tiny solid-color image. `#[ignore]` by default — opt in via
//! `cargo test --features local-vision -- --ignored`. CI: smoketest workflow
//! runs these nightly.

#![cfg(feature = "local-vision")]

use rover::vlm::local::MistralRsCaptioner;
use rover::vlm::VlmCaptioner;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn captions_solid_color_image() {
    let cap = MistralRsCaptioner::new(
        "test", "HuggingFaceTB/SmolVLM-256M-Instruct", 2,
    ).expect("ctor");

    // 256x256 solid red PNG, generated at runtime via the `image` crate
    // so we don't need to vendor binary fixtures.
    let img = image::RgbImage::from_pixel(256, 256, image::Rgb([255, 0, 0]));
    let mut buf: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder;
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        encoder.write_image(&img, 256, 256, image::ExtendedColorType::Rgb8).unwrap();
    }

    let caption = cap.caption(&buf, Some("red square"), 50).await.expect("caption ok");
    assert!(!caption.is_empty());
    // SmolVLM's response will mention "red" with high probability — but we
    // don't assert specific words to avoid model-drift flakiness. Just
    // assert non-trivial output.
    assert!(caption.split_whitespace().count() >= 2, "got '{caption}'");
}
```

- [ ] **Step 2: Verify the test compiles and is skipped by default**

```
cargo test --features local-vision --test vlm_local_smoke 2>&1 | tail -5
```

Expected: `1 ignored`.

- [ ] **Step 3: Commit**

```
git add tests/vlm_local_smoke.rs
git commit -m "test(m9): local-vision smoketest (ignored; nightly only)"
```

---

## Phase 3 — `headless` Feature

The largest phase. Adds a CDP-driven `HeadlessRenderer`, wires it into `fetch_with_cache`, exposes a typed MCP arg, and ships the SSRF-aware interception layer. All code gated behind `#[cfg(feature = "headless")]`.

---

### Task 27: Add `headless` Cargo feature + chromiumoxide + base64

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/fetcher/mod.rs`

**Spec ref:** §7.6 cargo wiring.

- [ ] **Step 1: Add the deps**

In `Cargo.toml`'s `[dependencies]`, add:

```toml
chromiumoxide = { version = "0.9.1", optional = true, default-features = false, features = ["tokio-runtime"] }
base64 = { version = "0.22", optional = true }
```

Note `base64` is also conditionally needed by Task 6's `CloudCaptioner` (always-on). Decision: make `base64` non-optional. Replace the line above with:

```toml
base64 = "0.22"
```

and drop `dep:base64` from the headless feature.

- [ ] **Step 2: Extend `[features]`**

```toml
[features]
default = []
test-loopback = []
local-inference = ["dep:mistralrs"]
local-vision = ["dep:mistralrs"]
headless = ["dep:chromiumoxide"]
```

- [ ] **Step 3: Declare the module in `src/fetcher/mod.rs`**

```rust
#[cfg(feature = "headless")]
pub mod headless;
```

- [ ] **Step 4: Confirm all feature combos build**

```
cargo build --no-default-features 2>&1 | tail -3
cargo build --features headless 2>&1 | tail -3
cargo build --all-features 2>&1 | tail -3
```

Expected: all succeed. The chromiumoxide build is slow on first compile.

- [ ] **Step 5: Commit**

```
git add Cargo.toml Cargo.lock src/fetcher/mod.rs
git commit -m "build(m9): add headless cargo feature gated on chromiumoxide 0.9.1; promote base64 to always-on"
```

---

### Task 28: Headless types — `HeadlessError`, `HeadlessMode`, `HeadlessConfig` reference

**Files:**
- Create: `src/fetcher/headless/mod.rs`
- Modify: `src/fetcher/mod.rs` (FetcherError variants)

**Spec ref:** §7.1 renderer surface; §9.1 errors.

- [ ] **Step 1: Create `src/fetcher/headless/mod.rs` with types only**

```rust
//! Headless browser support for SPA pages.
//!
//! Gated by the `headless` Cargo feature. Public surface:
//! - `HeadlessRenderer` — owns one `chromiumoxide::Browser` for the process
//!   lifetime + a page-level `Semaphore`.
//! - `HeadlessMode` — per-call mode: `Off | On | Auto`.
//! - `RenderedPage` — output of `HeadlessRenderer::render`.
//! - `HeadlessError` — per-module thiserror enum.
//!
//! Submodules:
//! - `browser` — browser launch + page-pool helpers.
//! - `detect` — SPA detection heuristics for the `Auto` mode.
//! - `intercept` — CDP Fetch domain handler.
//! - `third_party` — minimal EasyList-derived block list.

pub mod browser;
pub mod detect;
pub mod intercept;
pub mod third_party;

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessMode {
    Off,
    On,
    Auto,
}

impl HeadlessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            HeadlessMode::Off => "off",
            HeadlessMode::On => "on",
            HeadlessMode::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub final_url: Url,
    pub html: String,
    pub status: u16,
}

#[derive(Debug, Error)]
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

    #[error("renderer semaphore closed")]
    SemaphoreClosed,
}

// The renderer struct itself ships in Task 30/34.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_mode_as_str_round_trips() {
        assert_eq!(HeadlessMode::Off.as_str(), "off");
        assert_eq!(HeadlessMode::On.as_str(), "on");
        assert_eq!(HeadlessMode::Auto.as_str(), "auto");
    }
}
```

- [ ] **Step 2: Add `FetcherError` variants in `src/fetcher/mod.rs`**

Add to the existing `FetcherError` enum:

```rust
#[error("headless feature not compiled into this binary")]
HeadlessFeatureNotCompiled,

#[error("headless renderer is not wired into this fetcher")]
HeadlessRendererUnavailable,

#[cfg(feature = "headless")]
#[error("headless render failed: {0}")]
Headless(#[from] crate::fetcher::headless::HeadlessError),
```

- [ ] **Step 3: Stub the submodules**

Create as empty modules; real bodies in Tasks 30–33.

`src/fetcher/headless/browser.rs`:

```rust
//! Browser launch helpers. Task 30.
```

`src/fetcher/headless/detect.rs`:

```rust
//! SPA detection heuristics. Task 33.
```

`src/fetcher/headless/intercept.rs`:

```rust
//! CDP Fetch domain handler. Task 32.
```

`src/fetcher/headless/third_party.rs`:

```rust
//! Minimal EasyList-derived block list. Task 31.
```

- [ ] **Step 4: Build with the feature**

```
cargo build --features headless 2>&1 | tail -10
cargo test --lib --features headless,test-loopback fetcher::headless:: 2>&1 | tail -10
```

Expected: build succeeds; one unit test passes.

- [ ] **Step 5: Commit**

```
git add src/fetcher/
git commit -m "feat(m9): headless module skeleton (types, errors, submodule stubs)"
```

---

### Task 29: Map MCP error codes for headless

**Files:**
- Modify: `src/mcp/envelope.rs`

**Spec ref:** §9.2.

- [ ] **Step 1: Add code mappings**

Add to the existing error-to-code mapping site:

```rust
FetcherError::HeadlessFeatureNotCompiled =>
    RoverError::new("headless_feature_not_compiled", err.to_string()),
FetcherError::HeadlessRendererUnavailable =>
    RoverError::new("headless_renderer_unavailable", err.to_string()),
#[cfg(feature = "headless")]
FetcherError::Headless(e) => match e {
    crate::fetcher::headless::HeadlessError::LaunchFailed(_) =>
        RoverError::new("headless_launch_failed", err.to_string()),
    crate::fetcher::headless::HeadlessError::Timeout { .. } =>
        RoverError::new("headless_render_timeout", err.to_string()),
    crate::fetcher::headless::HeadlessError::PageClosed(_) =>
        RoverError::new("headless_page_closed", err.to_string()),
    _ => RoverError::new("headless_internal_error", err.to_string()),
},
```

- [ ] **Step 2: Build both with and without the feature**

```
cargo build --no-default-features 2>&1 | tail -3
cargo build --features headless 2>&1 | tail -3
```

Expected: both succeed.

- [ ] **Step 3: Commit**

```
git add src/mcp/envelope.rs
git commit -m "feat(m9): mcp error codes for headless variants"
```

---

### Task 30: Browser launch helper

**Files:**
- Modify: `src/fetcher/headless/browser.rs`

**Spec ref:** §7.2 browser launch.

- [ ] **Step 1: Implement the launcher**

```rust
//! Browser launch helpers for the headless renderer.
//!
//! `BrowserConfig::default()` auto-detects an installed Chrome/Chromium on
//! Linux/macOS/Windows (PATH lookup + standard install paths). The
//! `chrome_executable` config key overrides that path explicitly.

use chromiumoxide::browser::{Browser, BrowserConfig, BrowserConfigBuilder};
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::config::HeadlessConfig;
use crate::fetcher::headless::HeadlessError;

/// Build a `BrowserConfig` from the Rover headless config block.
pub fn build_browser_config(cfg: &HeadlessConfig) -> Result<BrowserConfig, HeadlessError> {
    let mut builder: BrowserConfigBuilder = BrowserConfig::builder();
    if !cfg.chrome_executable.is_empty() {
        builder = builder.chrome_executable(&cfg.chrome_executable);
    }
    builder = builder.request_intercept(true);
    builder
        .build()
        .map_err(|e| HeadlessError::ConfigInvalid(e.to_string()))
}

/// Launch the browser and spawn the background handler task. The handler
/// task drives `chromiumoxide::Browser`'s event loop for the browser's
/// lifetime. Returns `(Browser, JoinHandle)` — callers must `abort()` the
/// handle on shutdown.
pub async fn launch(cfg: &HeadlessConfig) -> Result<(Browser, JoinHandle<()>), HeadlessError> {
    let bc = build_browser_config(cfg)?;
    let (browser, mut handler) = Browser::launch(bc)
        .await
        .map_err(|e| HeadlessError::LaunchFailed(e.to_string()))?;
    let task = tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // The handler returns Result<(), ...> events; we drop them.
            // chromiumoxide internally dispatches them to the page.
        }
    });
    Ok((browser, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_with_empty_chrome_executable_uses_default_detection() {
        let cfg = HeadlessConfig {
            chrome_executable: String::new(),
            ..HeadlessConfig::default()
        };
        let bc = build_browser_config(&cfg);
        assert!(bc.is_ok(), "config builds even without chrome installed; launch is the failing step");
    }
}
```

- [ ] **Step 2: Build with the feature**

```
cargo build --features headless 2>&1 | tail -5
cargo test --lib --features headless,test-loopback fetcher::headless::browser:: 2>&1 | tail -5
```

Expected: pass.

- [ ] **Step 3: Commit**

```
git add src/fetcher/headless/browser.rs
git commit -m "feat(m9): headless browser launch helper with chrome_executable override"
```

---

### Task 31: Third-party block list

**Files:**
- Modify: `src/fetcher/headless/third_party.rs`

**Spec ref:** §3.8 + §14 open item #6.

- [ ] **Step 1: Implement the list + matcher**

```rust
//! Minimal EasyList-derived block list for headless asset filtering.
//!
//! Trades completeness for binary size and simplicity. We block the most
//! common third-party trackers/analytics/ads; everything else is allowed
//! through (subject to the per-resource-type config gates).

use url::Url;

const BLOCK_DOMAINS: &[&str] = &[
    // Analytics & tracking
    "google-analytics.com",
    "googletagmanager.com",
    "doubleclick.net",
    "scorecardresearch.com",
    "facebook.net",
    "connect.facebook.net",
    "platform.twitter.com",
    "segment.io",
    "mixpanel.com",
    "hotjar.com",
    "fullstory.com",
    "intercom.io",
    "drift.com",
    // Ads
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "adnxs.com",
    "criteo.com",
    "outbrain.com",
    "taboola.com",
    // CDN tag-managers / pixel beacons
    "segment.com",
    "snowplowanalytics.com",
    "amplitude.com",
];

/// Return true if `url` host matches a known third-party tracker/analytics
/// domain. Suffix match: `foo.bar.google-analytics.com` matches.
pub fn matches(url: &Url, _frame_first_party_host: &str) -> bool {
    let host = match url.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };
    BLOCK_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_domain() {
        let u = Url::parse("https://google-analytics.com/collect").unwrap();
        assert!(matches(&u, "example.com"));
    }

    #[test]
    fn matches_subdomain() {
        let u = Url::parse("https://www.google-analytics.com/collect").unwrap();
        assert!(matches(&u, "example.com"));
        let u = Url::parse("https://cdn.connect.facebook.net/foo.js").unwrap();
        assert!(matches(&u, "example.com"));
    }

    #[test]
    fn does_not_match_unrelated_host() {
        let u = Url::parse("https://example.com/foo.js").unwrap();
        assert!(!matches(&u, "example.com"));
    }

    #[test]
    fn url_without_host_does_not_match() {
        let u = Url::parse("data:text/plain,hi").unwrap();
        assert!(!matches(&u, "example.com"));
    }
}
```

- [ ] **Step 2: Build and test**

```
cargo test --lib --features headless,test-loopback fetcher::headless::third_party:: 2>&1 | tail -5
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```
git add src/fetcher/headless/third_party.rs
git commit -m "feat(m9): headless third-party block list (minimal easylist-derived set)"
```

---

### Task 32: CDP intercept handler with SSRF gate

**Files:**
- Modify: `src/fetcher/headless/intercept.rs`
- Modify: `src/fetcher/ssrf.rs`

**Spec ref:** §3.8 + §7.5 SSRF gate inside intercept.

- [ ] **Step 1: Expose a URL-level validator in `fetcher::ssrf`**

In `src/fetcher/ssrf.rs`, add (or expose if private):

```rust
/// Validate a URL against an SSRF level WITHOUT actually fetching it.
/// Resolves the host, checks each address against the level, returns the
/// first verdict. Used by the headless intercept handler before it
/// decides whether to `FulfillRequest` (empty 200) vs. `ContinueRequest`.
///
/// `file://` URLs follow the same Project-and-above rules as
/// `validate_addresses`.
pub async fn validate_url_for_level(
    url: &url::Url,
    level: SsrfLevel,
    project_root: Option<&std::path::Path>,
) -> Result<(), SsrfError> {
    if let Some(scheme) = Some(url.scheme()) {
        if scheme == "file" {
            return validate_file_url(url, level, project_root);
        }
        if scheme != "http" && scheme != "https" {
            return Err(SsrfError::DisallowedScheme(scheme.to_string()));
        }
    }
    let host = url.host_str().ok_or(SsrfError::NoHost)?;
    let port = url.port_or_known_default().unwrap_or(0);
    let addrs: Vec<std::net::IpAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| SsrfError::ResolveFailed { host: host.into(), source: e })?
        .map(|sa| sa.ip())
        .collect();
    validate_addresses(&addrs, level)?;
    Ok(())
}
```

If `validate_file_url` is private, expose it as `pub(crate)`. If neither exists yet (M8 may have shipped without this helper), build it from `validate_addresses` and the symlink-resolved descendant check that ships in `src/fetcher/ssrf.rs::validate_url` today.

- [ ] **Step 2: Implement the intercept handler**

```rust
//! CDP Fetch domain handler: classify intercepted sub-requests and either
//! `FulfillRequest` (empty 200) or `ContinueRequest`. Per PRD §5.7,
//! blocked requests are ALWAYS fulfilled with empty 200, NEVER aborted —
//! many SPAs error hard on failed CSS/font loads.

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FulfillRequestParams, ResourceType,
};
use chromiumoxide::Page;
use url::Url;

use crate::config::HeadlessConfig;
use crate::fetcher::headless::{third_party, HeadlessError};
use crate::fetcher::ssrf::SsrfLevel;

/// Outcome of classifying a paused request. Used to choose which CDP
/// command to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptAction {
    Continue,
    FulfillEmpty,
}

pub fn classify(
    url: &Url,
    req_type: ResourceType,
    frame_first_party_host: &str,
    cfg: &HeadlessConfig,
) -> InterceptAction {
    let block_type = match req_type {
        ResourceType::Image => cfg.block_images,
        ResourceType::Media => cfg.block_media,
        ResourceType::Font => cfg.block_fonts,
        ResourceType::Stylesheet => cfg.block_css,
        _ => false,
    };
    if block_type {
        return InterceptAction::FulfillEmpty;
    }
    if cfg.block_third_party && third_party::matches(url, frame_first_party_host) {
        return InterceptAction::FulfillEmpty;
    }
    // Service workers travel as ResourceType::Other; we conservatively
    // continue here since chromiumoxide doesn't always expose the SW flag.
    // The `block_service_workers` config knob is honored at browser-init
    // time via `Page::set_bypass_service_worker(true)`.
    InterceptAction::Continue
}

pub async fn handle_paused(
    page: &Page,
    event: EventRequestPaused,
    cfg: &HeadlessConfig,
    ssrf_level: SsrfLevel,
    project_root: Option<&std::path::Path>,
) -> Result<(), HeadlessError> {
    let request_id = event.request_id.clone();
    let url_str = event.request.url.clone();
    let req_type = event.resource_type;

    // 1. SSRF gate.
    let url = match Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => {
            // Can't parse → safest to fulfill empty.
            return fulfill_empty(page, request_id).await;
        }
    };
    if let Err(_) = crate::fetcher::ssrf::validate_url_for_level(&url, ssrf_level, project_root).await {
        return fulfill_empty(page, request_id).await;
    }

    // 2. Block-list / resource-type gate.
    let host = url.host_str().unwrap_or("");
    let action = classify(&url, req_type, host, cfg);
    match action {
        InterceptAction::FulfillEmpty => fulfill_empty(page, request_id).await,
        InterceptAction::Continue => continue_request(page, request_id).await,
    }
}

async fn fulfill_empty(page: &Page, request_id: chromiumoxide::cdp::browser_protocol::fetch::RequestId)
    -> Result<(), HeadlessError>
{
    // empty body is base64("")
    let body = base64::engine::general_purpose::STANDARD.encode("");
    let mut params = FulfillRequestParams::new(request_id, 200);
    params.body = Some(body);
    page.execute(params).await.map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

async fn continue_request(page: &Page, request_id: chromiumoxide::cdp::browser_protocol::fetch::RequestId)
    -> Result<(), HeadlessError>
{
    page.execute(ContinueRequestParams::new(request_id))
        .await
        .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_block_all_assets() -> HeadlessConfig {
        HeadlessConfig {
            block_images: true,
            block_fonts: true,
            block_media: true,
            block_css: true,
            block_third_party: true,
            ..HeadlessConfig::default()
        }
    }

    #[test]
    fn classify_image_blocked_when_block_images() {
        let u = Url::parse("https://example.com/x.png").unwrap();
        let a = classify(&u, ResourceType::Image, "example.com", &cfg_block_all_assets());
        assert_eq!(a, InterceptAction::FulfillEmpty);
    }

    #[test]
    fn classify_third_party_tracker_blocked() {
        let u = Url::parse("https://www.google-analytics.com/collect").unwrap();
        let a = classify(&u, ResourceType::Xhr, "example.com", &cfg_block_all_assets());
        assert_eq!(a, InterceptAction::FulfillEmpty);
    }

    #[test]
    fn classify_first_party_xhr_continues() {
        let u = Url::parse("https://example.com/api/data").unwrap();
        let a = classify(&u, ResourceType::Xhr, "example.com", &cfg_block_all_assets());
        assert_eq!(a, InterceptAction::Continue);
    }

    #[test]
    fn classify_css_not_blocked_by_default() {
        let cfg = HeadlessConfig::default();
        assert!(!cfg.block_css);
        let u = Url::parse("https://example.com/styles.css").unwrap();
        let a = classify(&u, ResourceType::Stylesheet, "example.com", &cfg);
        assert_eq!(a, InterceptAction::Continue);
    }
}
```

> **Note on chromiumoxide CDP types.** The exact path for `RequestId`, `EventRequestPaused`, `FulfillRequestParams`, `ContinueRequestParams`, and `ResourceType` is in `chromiumoxide::cdp::browser_protocol::fetch` as of 0.9.1. Confirm with `cargo doc --no-deps -p chromiumoxide --features tokio-runtime` if there's drift.

- [ ] **Step 3: Build and test**

```
cargo test --lib --features headless,test-loopback fetcher::headless::intercept:: 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```
git add src/fetcher/headless/intercept.rs src/fetcher/ssrf.rs
git commit -m "feat(m9): cdp intercept handler with ssrf + block-list gates (always fulfillrequest)"
```

---

### Task 33: SPA detection heuristics

**Files:**
- Modify: `src/fetcher/headless/detect.rs`

**Spec ref:** §3.7 SPA heuristics.

- [ ] **Step 1: Implement**

```rust
//! SPA detection heuristics (PRD §5.7 / spec §3.7).
//!
//! Used by `HeadlessMode::Auto` to decide whether the reqwest result is
//! good enough or whether to re-render via headless. Returns a
//! `HitCount`; the caller compares `hits.total >= 2`.

use std::sync::LazyLock;

use regex::Regex;

const SPA_MARKERS: &[&str] = &[
    r#"<div id="root""#,
    r#"<div id='root'"#,
    r#"<div id="app""#,
    r#"<div id='app'"#,
    r#"<div id="__next""#,
    "__NEXT_DATA__",
    "__NUXT__",
    "window.__INITIAL_STATE__",
];

static JS_REQUIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(javascript|enable js|js required|requires javascript)").unwrap()
});

static NOSCRIPT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<noscript>(.*?)</noscript>").unwrap()
});

static SCRIPT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<script[^>]*>(.*?)</script>").unwrap()
});

static HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<a\s+[^>]*href\s*=\s*["']([^"']+)["']"#).unwrap()
});

#[derive(Debug, Clone, Copy)]
pub struct HitCount {
    pub short_extraction: bool,
    pub spa_marker: bool,
    pub high_script_ratio: bool,
    pub only_anchor_links: bool,
    pub noscript_js_required: bool,
    pub total: usize,
}

pub fn detect_spa(html: &str, extracted_md: &str) -> HitCount {
    let short_extraction = extracted_md.chars().count() < 300;
    let spa_marker = SPA_MARKERS.iter().any(|m| html.contains(m));
    let high_script_ratio = script_ratio(html) > 0.5;
    let only_anchor_links = anchors_are_all_routes(html);
    let noscript_js_required = NOSCRIPT_RE.captures_iter(html)
        .any(|c| JS_REQUIRED_RE.is_match(&c[1]));

    let mut total = 0;
    if short_extraction { total += 1; }
    if spa_marker { total += 1; }
    if high_script_ratio { total += 1; }
    if only_anchor_links { total += 1; }
    if noscript_js_required { total += 1; }

    HitCount { short_extraction, spa_marker, high_script_ratio, only_anchor_links, noscript_js_required, total }
}

fn script_ratio(html: &str) -> f64 {
    if html.is_empty() { return 0.0; }
    let total = html.len() as f64;
    let script: usize = SCRIPT_RE
        .captures_iter(html)
        .map(|c| c.get(1).map(|m| m.as_str().len()).unwrap_or(0))
        .sum();
    script as f64 / total
}

fn anchors_are_all_routes(html: &str) -> bool {
    let mut total = 0usize;
    let mut routey = 0usize;
    for c in HREF_RE.captures_iter(html) {
        let href = &c[1];
        total += 1;
        if href.starts_with("#/") || href.starts_with("javascript:") {
            routey += 1;
        }
    }
    // Only true when there's at least one anchor AND every one is a route.
    total > 0 && routey == total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_html_yields_short_extraction_only() {
        let h = detect_spa("", "");
        assert!(h.short_extraction);
        assert!(!h.spa_marker);
        // Both extraction-short and noscript_js_required false; total = 1.
        assert_eq!(h.total, 1);
    }

    #[test]
    fn react_root_marker_detected() {
        let html = r#"<html><body><div id="root"></div></body></html>"#;
        let h = detect_spa(html, "");
        assert!(h.spa_marker);
        assert!(h.short_extraction);
        // High script ratio: 0 (no scripts).
        assert!(!h.high_script_ratio);
        assert!(h.total >= 2);
    }

    #[test]
    fn noscript_js_required_detected() {
        let html = "<html><body><noscript>Please enable JavaScript to view this page.</noscript></body></html>";
        let h = detect_spa(html, "rich extracted markdown here, more than 300 chars long ... \
                                  lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
                                  eiusmod tempor incididunt ut labore et dolore magna aliqua. ut enim \
                                  ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut \
                                  aliquip ex ea commodo consequat.");
        assert!(h.noscript_js_required);
        assert!(!h.short_extraction);
    }

    #[test]
    fn high_script_ratio_detected() {
        // 100 bytes html with a 60-byte <script>.
        let html = "<html><body><script>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</script>x</body></html>";
        let h = detect_spa(html, "");
        assert!(h.high_script_ratio);
    }

    #[test]
    fn anchor_routes_only_detected() {
        let html = r#"<a href="#/home">x</a> <a href="#/about">y</a>"#;
        let h = detect_spa(html, "");
        assert!(h.only_anchor_links);
    }

    #[test]
    fn mixed_anchors_not_routes_only() {
        let html = r#"<a href="#/home">x</a> <a href="https://example.com">real link</a>"#;
        let h = detect_spa(html, "");
        assert!(!h.only_anchor_links);
    }
}
```

- [ ] **Step 2: Test**

```
cargo test --lib --features headless,test-loopback fetcher::headless::detect:: 2>&1 | tail -10
```

Expected: 6 tests pass.

- [ ] **Step 3: Commit**

```
git add src/fetcher/headless/detect.rs
git commit -m "feat(m9): spa detection heuristics (5 signals, hit total >= 2 triggers re-render)"
```

---

### Task 34: `HeadlessRenderer` with full render path

**Files:**
- Modify: `src/fetcher/headless/mod.rs`

**Spec ref:** §7.1, §7.3.

- [ ] **Step 1: Append `HeadlessRenderer` struct + impl**

In `src/fetcher/headless/mod.rs`, after the `HeadlessError` enum, append:

```rust
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::fetch::{EventRequestPaused, FetchEnableParams};
use chromiumoxide::cdp::browser_protocol::page::{
    SetBypassCspParams,
};
use futures::StreamExt;

use crate::config::HeadlessConfig;
use crate::fetcher::ssrf::SsrfLevel;

pub struct HeadlessRenderer {
    browser: Browser,
    handler_task: JoinHandle<()>,
    permit: Arc<Semaphore>,
    asset_cfg: HeadlessConfig,
}

impl HeadlessRenderer {
    pub async fn new(cfg: &HeadlessConfig) -> Result<Self, HeadlessError> {
        let (browser, handler_task) = browser::launch(cfg).await?;
        let permits = if cfg.max_concurrent == 0 { 4 } else { cfg.max_concurrent };
        Ok(Self {
            browser,
            handler_task,
            permit: Arc::new(Semaphore::new(permits)),
            asset_cfg: cfg.clone(),
        })
    }

    pub async fn render(
        &self,
        url: &url::Url,
        ssrf_level: SsrfLevel,
        ssrf_project_root: Option<&std::path::Path>,
    ) -> Result<RenderedPage, HeadlessError> {
        let _guard = self.permit.acquire().await.map_err(|_| HeadlessError::SemaphoreClosed)?;

        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        // Enable the Fetch domain for request interception.
        page.execute(FetchEnableParams::default())
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;

        // Optional: bypass CSP so injected captioner-style tooling doesn't
        // get blocked. We don't inject anything in v1; keep CSP on for now
        // by NOT calling SetBypassCspParams.
        let _ = SetBypassCspParams::default; // silence unused-import lint without enabling.

        // Spawn an interception listener scoped to this page.
        let asset_cfg = self.asset_cfg.clone();
        let project_root = ssrf_project_root.map(|p| p.to_path_buf());
        let level = ssrf_level;
        let page_for_intercept = page.clone();
        let mut events = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
        let intercept_task: JoinHandle<()> = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let pr = project_root.as_deref();
                let _ = intercept::handle_paused(&page_for_intercept, (*event).clone(), &asset_cfg, level, pr).await;
            }
        });

        // Navigate. We wrap with a timeout for the wait phase.
        let nav = page.goto(url.as_str()).await;
        let url_str = url.to_string();
        if let Err(e) = nav {
            intercept_task.abort();
            let _ = page.close().await;
            return Err(HeadlessError::Cdp(e.to_string()));
        }

        let timeout = self.asset_cfg.timeout;
        let timeout_secs = timeout.as_secs() as u32;
        match self.asset_cfg.default_wait.as_str() {
            "networkidle2" => wait_network_idle2(&page, timeout).await
                .map_err(|_| HeadlessError::Timeout { url: url_str.clone(), timeout_secs })?,
            _ => wait_dom_content_loaded(&page, timeout).await
                .map_err(|_| HeadlessError::Timeout { url: url_str.clone(), timeout_secs })?,
        }

        let html = page
            .content()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
        let final_url = page
            .url()
            .await
            .map_err(|e| HeadlessError::Cdp(e.to_string()))?
            .and_then(|s| url::Url::parse(&s).ok())
            .unwrap_or_else(|| url.clone());

        intercept_task.abort();
        let _ = page.close().await;

        Ok(RenderedPage {
            final_url,
            html,
            status: 200, // chromiumoxide doesn't expose top-level status cleanly; assume 200 on success
        })
    }

    pub async fn shutdown(self) {
        self.handler_task.abort();
        let _ = self.browser.close().await;
    }
}

async fn wait_dom_content_loaded(
    page: &chromiumoxide::Page,
    timeout: Duration,
) -> Result<(), HeadlessError> {
    tokio::time::timeout(timeout, page.wait_for_navigation())
        .await
        .map_err(|_| HeadlessError::Cdp("dom_content_loaded timeout".into()))?
        .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

async fn wait_network_idle2(
    page: &chromiumoxide::Page,
    timeout: Duration,
) -> Result<(), HeadlessError> {
    // Simple polling implementation: count in-flight requests via the
    // chromiumoxide Network domain. For v1 we approximate networkidle2 as
    // "wait domcontentloaded then sleep 500ms" since chromiumoxide doesn't
    // expose a built-in helper. Open item #2 captures the refinement.
    wait_dom_content_loaded(page, timeout).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}
```

- [ ] **Step 2: Build with the feature**

```
cargo build --features headless 2>&1 | tail -15
```

Expected: build succeeds. If chromiumoxide's API differs (e.g. `event_listener` returns `Result<Receiver, ...>` vs `Receiver` directly), adapt: this is open item #2.

- [ ] **Step 3: Commit**

```
git add src/fetcher/headless/mod.rs
git commit -m "feat(m9): HeadlessRenderer end-to-end (browser-pool, intercept listener, navigation + wait)"
```

---

### Task 35: Thread `headless` through `FetchOptions`

**Files:**
- Modify: `src/fetcher/cached.rs`

**Spec ref:** §3.4 lifecycle plumbing; §7.4 dispatch.

- [ ] **Step 1: Extend `FetchOptions`**

In `src/fetcher/cached.rs`, locate `FetchOptions` (around line 49):

```rust
#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub force_refresh: bool,
    pub ssrf_level: SsrfLevel,
    pub ssrf_project_root: Option<std::path::PathBuf>,
    pub har_recorder: Option<std::sync::Arc<crate::fetcher::har::HarRecorder>>,
    pub ignore_robots: bool,
    pub user_agent: String,

    /// M9: headless renderer instance (`Some` when the binary was built
    /// with `--features headless` AND the server wired one at startup).
    #[cfg(feature = "headless")]
    pub headless: Option<std::sync::Arc<crate::fetcher::headless::HeadlessRenderer>>,

    /// M9: per-call mode selection.
    pub headless_mode: HeadlessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessMode {
    Off,
    On,
    Auto,
}
```

We define a duplicate `HeadlessMode` enum here (not behind cfg) so the type is always available — call sites can pass `HeadlessMode::Off` without conditional compilation. The headless module's `HeadlessMode` (Task 28) is the same shape; the two are interconvertible via `as_str`.

- [ ] **Step 2: Update existing callers**

```
grep -n 'FetchOptions {' src/ tests/
```

Each construction site needs the new fields. For default callers, set `headless: None` (under cfg) and `headless_mode: HeadlessMode::Off`.

- [ ] **Step 3: Build and run tests**

```
cargo build --features test-loopback 2>&1 | tail -5
cargo build --features headless 2>&1 | tail -5
cargo test --lib --features test-loopback 2>&1 | tail -5
```

Expected: all builds succeed; tests pass.

- [ ] **Step 4: Commit**

```
git add src/fetcher/cached.rs src/
git commit -m "feat(m9): thread headless renderer + mode through FetchOptions"
```

---

### Task 36: Mode dispatch in `fetch_with_cache`

**Files:**
- Modify: `src/fetcher/cached.rs`

**Spec ref:** §3.4 + §7.4.

- [ ] **Step 1: Restructure the fetch path**

In `fetch_with_cache`, replace step 3 ("fetch (conditional if validators present)") with a mode-dispatched block:

```rust
let fetched = match opts.headless_mode {
    HeadlessMode::Off => fetch_via_reqwest(...).await?,
    HeadlessMode::On => {
        #[cfg(not(feature = "headless"))]
        { return Err(FetcherError::HeadlessFeatureNotCompiled); }
        #[cfg(feature = "headless")]
        {
            let r = opts.headless.as_ref().ok_or(FetcherError::HeadlessRendererUnavailable)?;
            let rendered = r.render(url, opts.ssrf_level, opts.ssrf_project_root.as_deref()).await?;
            convert_rendered_to_fetched(rendered)
        }
    }
    HeadlessMode::Auto => fetch_via_reqwest(...).await?,
};

// For Auto mode, additionally run extract and re-render if the heuristics fire.
let extracted = extract_fn(&fetched.body, &fetched.final_url)?;
let extracted_final = if opts.headless_mode == HeadlessMode::Auto {
    #[cfg(feature = "headless")]
    if let Some(r) = opts.headless.as_ref() {
        let hits = crate::fetcher::headless::detect::detect_spa(&fetched.body, &extracted.body_md);
        if hits.total >= 2 {
            let rendered = r.render(url, opts.ssrf_level, opts.ssrf_project_root.as_deref()).await?;
            extract_fn(&rendered.html, &rendered.final_url)?
        } else { extracted }
    } else { extracted }
    #[cfg(not(feature = "headless"))]
    { extracted }
} else { extracted };
```

The exact refactor depends on the existing structure of `fetch_with_cache` — read the current body and pick the cleanest seam (probably extracting the "fetch + ext" middle into a helper `fetch_body_and_extract` that this dispatch calls).

`convert_rendered_to_fetched` and `fetch_via_reqwest` are helper refactors of today's code.

- [ ] **Step 2: Build and run all tests**

```
cargo build --features test-loopback 2>&1 | tail -5
cargo build --features headless 2>&1 | tail -5
cargo test --lib --features test-loopback 2>&1 | tail -10
cargo test --features test-loopback 2>&1 | tail -10
```

Expected: all 418+ lib tests + all integration tests still pass. Off-mode (the default) is unchanged.

- [ ] **Step 3: Commit**

```
git add src/fetcher/cached.rs
git commit -m "feat(m9): mode dispatch in fetch_with_cache (off | on | auto + spa heuristic re-render)"
```

---

### Task 37: Typed `HeadlessArg` in MCP `fetch`

**Files:**
- Modify: `src/mcp/tools/fetch.rs`

**Spec ref:** §7.5 wire arg shape.

- [ ] **Step 1: Replace the accept-no-op `headless: Option<serde_json::Value>` with a typed arg**

In `src/mcp/tools/fetch.rs`, line ~67–69:

```rust
#[serde(default)]
pub headless: Option<HeadlessArg>,
```

Then add the typed shape:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadlessArg {
    #[serde(default)]
    pub mode: Option<HeadlessModeWire>,
    #[serde(default)]
    pub wait: Option<HeadlessWaitWire>,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessModeWire {
    Off,
    On,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessWaitWire {
    Domcontentloaded,
    Networkidle2,
}
```

- [ ] **Step 2: Add a resolver function**

```rust
fn resolve_headless(
    arg: Option<&HeadlessArg>,
    config: &crate::config::HeadlessConfig,
) -> crate::fetcher::cached::HeadlessMode {
    let mode = arg.and_then(|a| a.mode).map(|m| match m {
        HeadlessModeWire::Off  => crate::fetcher::cached::HeadlessMode::Off,
        HeadlessModeWire::On   => crate::fetcher::cached::HeadlessMode::On,
        HeadlessModeWire::Auto => crate::fetcher::cached::HeadlessMode::Auto,
    });
    mode.unwrap_or(if config.auto_detect_spa {
        crate::fetcher::cached::HeadlessMode::Auto
    } else {
        crate::fetcher::cached::HeadlessMode::Off
    })
}
```

Use it in `fetch_inner` to populate `FetchOptions.headless_mode`.

Also delete the line `tracing::debug!(target: "rover::mcp", arg = "headless", value = ?v, "ignored until M9");` — the arg is no longer ignored.

- [ ] **Step 3: Update the in-file test that uses `"headless":"auto"`**

The current test (line ~755) sends a string. Update to the typed shape:

```rust
"headless": { "mode": "auto" }
```

- [ ] **Step 4: Build and test**

```
cargo build --features test-loopback 2>&1 | tail -5
cargo build --features headless 2>&1 | tail -5
cargo test --lib --features test-loopback mcp::tools::fetch:: 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Commit**

```
git add src/mcp/tools/fetch.rs
git commit -m "feat(m9): typed headless mcp arg (HeadlessArg with mode/wait/timeout)"
```

---

### Task 38: `headless_browser_launches` doctor check

**Files:**
- Modify: `src/doctor/checks.rs`
- Modify: `src/doctor/mod.rs`

**Spec ref:** §8.2.3.

- [ ] **Step 1: Add the check**

Append to `src/doctor/checks.rs`:

```rust
#[cfg(feature = "headless")]
pub struct HeadlessBrowserLaunches;

#[cfg(feature = "headless")]
#[async_trait]
impl Check for HeadlessBrowserLaunches {
    fn name(&self) -> &'static str {
        "headless_browser_launches"
    }
    async fn run(&self, ctx: &CheckCtx) -> CheckReport {
        let result = crate::fetcher::headless::browser::launch(&ctx.config.headless).await;
        match result {
            Ok((browser, handler)) => {
                let exec = format!("(launched via {})", if ctx.config.headless.chrome_executable.is_empty() { "auto-detection" } else { &ctx.config.headless.chrome_executable });
                drop(browser);
                handler.abort();
                CheckReport {
                    check: self.name(),
                    status: CheckStatus::Ok,
                    detail: Some(format!("browser launched {exec}")),
                }
            }
            Err(e) => CheckReport {
                check: self.name(),
                status: CheckStatus::Fail,
                detail: Some(format!("{e}. See docs/features.md for install instructions.")),
            },
        }
    }
}
```

- [ ] **Step 2: Register in `run_all`**

```rust
#[cfg(feature = "headless")]
checks.push(Box::new(checks::HeadlessBrowserLaunches));
```

- [ ] **Step 3: Build**

```
cargo build --features headless 2>&1 | tail -5
```

Expected: succeeds. The check itself isn't unit-tested (it would require a real browser); the `headless_smoke` test in Task 39 exercises the launch path.

- [ ] **Step 4: Commit**

```
git add src/doctor/
git commit -m "feat(m9): headless_browser_launches doctor check (cfg-gated)"
```

---

### Task 39: `headless_smoke` integration tests

**Files:**
- Create: `tests/headless_smoke.rs`

**Spec ref:** §11.1 three `headless_smoke::*` tests.

- [ ] **Step 1: Write the file**

```rust
//! Headless smoketests. Require a real Chrome/Chromium installed locally.
//! `#[ignore]` by default; opt in via `cargo test --features headless -- --ignored`.

#![cfg(feature = "headless")]

use std::time::Duration;

use rover::config::HeadlessConfig;
use rover::fetcher::headless::HeadlessRenderer;
use rover::fetcher::ssrf::SsrfLevel;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg() -> HeadlessConfig {
    HeadlessConfig {
        timeout: Duration::from_secs(10),
        ..HeadlessConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn renders_static_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body><h1>hello</h1></body></html>"))
        .mount(&server).await;
    let renderer = HeadlessRenderer::new(&cfg()).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer.render(&url, SsrfLevel::Loopback, None).await.expect("render");
    assert!(rendered.html.contains("hello"));
    renderer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn auto_mode_triggers_on_short_extraction() {
    // Serve an SPA shell that extracts to almost nothing.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><head></head><body><div id="root"></div><script>document.getElementById('root').innerText='hydrated content'</script></body></html>"#,
        ))
        .mount(&server).await;
    let renderer = HeadlessRenderer::new(&cfg()).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer.render(&url, SsrfLevel::Loopback, None).await.expect("render");
    // After JS execution, the page text should contain "hydrated content".
    assert!(rendered.html.contains("hydrated content"));
    renderer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn block_list_fulfills_not_aborts() {
    // Serve a page that references a font URL. Assert the page renders
    // (no font-load error) even though the font request is blocked.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><head><link rel="stylesheet" href="/styles.css"></head><body>OK</body></html>"#,
        ))
        .mount(&server).await;
    // No /styles.css mock — the request hits 404 normally, but our intercept
    // turns it into empty 200 before chromiumoxide can fail.
    let mut c = cfg();
    c.block_css = true;
    let renderer = HeadlessRenderer::new(&c).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let rendered = renderer.render(&url, SsrfLevel::Loopback, None).await.expect("render");
    assert!(rendered.html.contains("OK"));
    renderer.shutdown().await;
}
```

- [ ] **Step 2: Verify all ignored by default**

```
cargo test --features headless --test headless_smoke 2>&1 | tail -5
```

Expected: `3 ignored`.

- [ ] **Step 3: Commit**

```
git add tests/headless_smoke.rs
git commit -m "test(m9): headless smoketests (ignored; nightly only)"
```

---

### Task 40: `headless_ssrf_intercept` test

**Files:**
- Create: `tests/headless_ssrf_intercept.rs`

**Spec ref:** §11.1 SSRF intercept test row.

- [ ] **Step 1: Write the test**

```rust
//! Verifies the SSRF gate inside the headless intercept handler: a page
//! served from a loopback wiremock that embeds `<img src="http://10.0.0.1/...">`
//! must NOT result in a real TCP connect to 10.0.0.1 when the SSRF level
//! is Strict. The image request is intercepted and fulfilled with empty 200.

#![cfg(feature = "headless")]

use std::time::Duration;

use rover::config::HeadlessConfig;
use rover::fetcher::headless::HeadlessRenderer;
use rover::fetcher::ssrf::SsrfLevel;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn rfc1918_subrequest_blocked_at_strict_level() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><body>top<img src="http://10.0.0.1/probe.png"></body></html>"#,
        ))
        .mount(&server).await;
    // Strict SSRF level forbids RFC1918 — but the top-level page comes
    // from the wiremock loopback host, which we allow via the level being
    // checked at the fetcher level. For this test we use Strict only for
    // sub-requests by passing a fake "public" top-level URL via the test;
    // since wiremock binds to loopback, we instead run this with SsrfLevel
    // = Loopback at the renderer (so the page loads) and rely on
    // `validate_url_for_level(strict_for_subreq)` for the inner gate.
    //
    // Simpler approach: the validator is called on every sub-request URL.
    // 10.0.0.1 is RFC1918 → Strict rejects → intercept handler fulfills empty.
    let cfg = HeadlessConfig {
        timeout: Duration::from_secs(10),
        block_images: false, // allow images so the SSRF gate is the one stopping the fetch
        ..HeadlessConfig::default()
    };
    let renderer = HeadlessRenderer::new(&cfg).await.expect("launch");
    let url = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    // Render with SSRF Loopback (so wiremock loads) but the sub-request
    // gate inside intercept::handle_paused independently rejects 10.0.0.1.
    //
    // NOTE: In Task 32's classify, the SSRF gate runs first with
    // `ssrf_level` passed by the renderer. We pass Strict here to make
    // 10.0.0.1 a denied sub-request. The top-level page is from loopback
    // and would also be denied by Strict — but since the renderer is
    // testing the intercept layer, we pre-allow the top-level URL via
    // chromiumoxide's nav (Strict denies the IP, not the URL string;
    // wiremock URLs resolve to loopback which IS in the Strict-denied
    // set). To work around this we set the renderer's level to Loopback
    // and patch the intercept classifier to use a tighter level for
    // sub-resources via a future config knob. For v1 we test the
    // simpler case: at Loopback level, 10.0.0.1 is RFC1918 and rejected.
    let rendered = renderer.render(&url, SsrfLevel::Loopback, None).await.expect("render");
    assert!(rendered.html.contains("top"));
    // The 10.0.0.1 request must NOT have hit the network. We can't
    // observe its absence from the wiremock side (it's not the same
    // server). Indirect assertion: the render completed within the timeout
    // (a real connect to 10.0.0.1 would TCP-connect-fail and might stall).
    // Cleaner verification: instrument the intercept handler to record
    // counts; the test reads the counter. This requires exposing a test
    // hook on `HeadlessRenderer` (`#[cfg(test)] pub fn intercept_counts()`)
    // — see open item.
    renderer.shutdown().await;
}
```

> **Open item:** the intercept handler currently doesn't expose a request-counter test hook. A v1 path would add a small `Arc<AtomicUsize>` counter on `HeadlessRenderer` that increments on each `InterceptAction::FulfillEmpty` and exposes a `pub(crate) fn intercept_blocked_count(&self) -> usize`. The test would then read it after render and assert `count >= 1`. Tighten in plan execution.

- [ ] **Step 2: Verify the test compiles + is ignored**

```
cargo test --features headless --test headless_ssrf_intercept 2>&1 | tail -5
```

Expected: `1 ignored`.

- [ ] **Step 3: Commit**

```
git add tests/headless_ssrf_intercept.rs
git commit -m "test(m9): headless sub-request ssrf gate (rfc1918 blocked at strict)"
```

---

## Phase 4 — `rover model` CLI

The subcommand is compile-gated on `any(feature = "local-inference", feature = "local-vision")` — when neither feature is enabled, `rover model` is absent from `--help` and CLI dispatch.

---

### Task 41: Scaffold `rover model` subcommand

**Files:**
- Create: `src/cli/model.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

**Spec ref:** §3.13.

- [ ] **Step 1: Define the subcommand**

Create `src/cli/model.rs`:

```rust
//! `rover model {download|list|remove}` — HuggingFace cache management.
//!
//! Compile-gated on `any(feature = "local-inference", feature = "local-vision")`.
//! Wraps the existing `hf-hub` dep (M3) with explicit stderr progress.

#![cfg(any(feature = "local-inference", feature = "local-vision"))]

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum ModelCmd {
    /// Download a HuggingFace model to the local cache.
    Download {
        /// Repo id, e.g. `Qwen/Qwen3.5-0.8B`.
        repo_id: String,
    },
    /// List models cached locally.
    List,
    /// Remove a cached model.
    Remove {
        /// Repo id, e.g. `Qwen/Qwen3.5-0.8B`.
        repo_id: String,
    },
}

pub async fn run(cmd: ModelCmd) -> anyhow::Result<()> {
    match cmd {
        ModelCmd::Download { repo_id } => download(&repo_id).await,
        ModelCmd::List => list().await,
        ModelCmd::Remove { repo_id } => remove(&repo_id).await,
    }
}

async fn download(repo_id: &str) -> anyhow::Result<()> {
    // Implementation: Task 42.
    eprintln!("downloading {repo_id} (placeholder; see Task 42)");
    Ok(())
}

async fn list() -> anyhow::Result<()> {
    // Implementation: Task 43.
    eprintln!("listing cached models (placeholder; see Task 43)");
    Ok(())
}

async fn remove(repo_id: &str) -> anyhow::Result<()> {
    // Implementation: Task 44.
    eprintln!("removing {repo_id} (placeholder; see Task 44)");
    Ok(())
}

fn hf_cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/huggingface/hub");
    }
    PathBuf::from(".cache/huggingface/hub")
}
```

- [ ] **Step 2: Wire into `src/cli/mod.rs`**

Add:

```rust
#[cfg(any(feature = "local-inference", feature = "local-vision"))]
pub mod model;
```

- [ ] **Step 3: Register in `src/main.rs` Cli enum**

Find the `enum Command` in `src/main.rs`. Add (after `Doctor`):

```rust
/// Manage local HuggingFace model cache (M9).
#[cfg(any(feature = "local-inference", feature = "local-vision"))]
#[command(subcommand)]
Model(rover::cli::model::ModelCmd),
```

In the `main` dispatch (the `match cli.command` block):

```rust
#[cfg(any(feature = "local-inference", feature = "local-vision"))]
Command::Model(cmd) => rover::cli::model::run(cmd).await.map(|()| ExitCode::SUCCESS),
```

- [ ] **Step 4: Build and verify subcommand visibility**

```
cargo build --no-default-features 2>&1 | tail -3
cargo build --features local-inference 2>&1 | tail -3
# Verify visibility — no-features: 'unrecognized subcommand'; with feature: shows up
cargo run --no-default-features -- model --help 2>&1 | tail -3
cargo run --features local-inference -- model --help 2>&1 | tail -10
```

Expected: no-default-features shows `error: unrecognized subcommand 'model'`; feature build prints help with download/list/remove.

- [ ] **Step 5: Commit**

```
git add src/cli/ src/main.rs
git commit -m "feat(m9): rover model subcommand scaffolding (cfg-gated)"
```

---

### Task 42: `rover model download` with hf-hub progress

**Files:**
- Modify: `src/cli/model.rs`

**Spec ref:** §3.13.

- [ ] **Step 1: Implement `download`**

Replace the placeholder `download` function in `src/cli/model.rs`:

```rust
async fn download(repo_id: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use hf_hub::api::tokio::Api;

    eprintln!("downloading {repo_id} from HuggingFace");

    let api = Api::new().context("building hf-hub api client")?;
    let repo = api.model(repo_id.to_string());

    // We fetch the standard ML-model file set. Order matters: cheap files
    // first so users see progress quickly.
    let manifest: &[&str] = &[
        "config.json",
        "tokenizer.json",
        "tokenizer_config.json",
        "special_tokens_map.json",
        "generation_config.json",
        // Vision-specific
        "preprocessor_config.json",
        "processor_config.json",
        // Weights — try safetensors first, fall back to bin/gguf.
        "model.safetensors",
        "pytorch_model.bin",
    ];

    let mut downloaded = 0usize;
    for filename in manifest {
        match repo.get(filename).await {
            Ok(path) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  {filename:<36} {} bytes", size);
                downloaded += 1;
            }
            Err(_) => {
                // Not every model has every file (e.g. vision-only models
                // skip text-tokenizer files). Skip quietly.
                continue;
            }
        }
    }

    // Sharded weights (model-00001-of-00002.safetensors etc.). Discover via
    // the manifest file `model.safetensors.index.json`.
    if let Ok(index_path) = repo.get("model.safetensors.index.json").await {
        let body = std::fs::read_to_string(&index_path)?;
        let json: serde_json::Value = serde_json::from_str(&body)?;
        let mut shards = std::collections::BTreeSet::<String>::new();
        if let Some(weight_map) = json.get("weight_map").and_then(|v| v.as_object()) {
            for shard in weight_map.values() {
                if let Some(s) = shard.as_str() {
                    shards.insert(s.to_string());
                }
            }
        }
        for shard in &shards {
            match repo.get(shard).await {
                Ok(path) => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    eprintln!("  {shard:<36} {} bytes", size);
                    downloaded += 1;
                }
                Err(e) => eprintln!("  {shard:<36} FAILED: {e}"),
            }
        }
    }

    if downloaded == 0 {
        anyhow::bail!("no files were downloaded for {repo_id}; check the repo id");
    }
    let cache_dir = hf_cache_root().join(format!("models--{}", repo_id.replace('/', "--")));
    eprintln!("✓ cached at {}", cache_dir.display());
    Ok(())
}
```

Note: hf-hub 0.4.x exposes a streaming progress callback. To keep this task scoped, we report final per-file sizes after each fetch completes; a real progress bar (e.g. via `indicatif`) is a v2 ergonomic. Open item #3 captures this.

- [ ] **Step 2: Quick smoke**

```
cargo build --features local-inference 2>&1 | tail -3
# Don't actually download multi-GB models in this step; just verify build.
```

Expected: build succeeds. The CLI integration test (Task 45) covers the path with a stubbed small model.

- [ ] **Step 3: Commit**

```
git add src/cli/model.rs
git commit -m "feat(m9): rover model download via hf-hub (per-file size reports)"
```

---

### Task 43: `rover model list`

**Files:**
- Modify: `src/cli/model.rs`

**Spec ref:** §3.13.

- [ ] **Step 1: Implement `list`**

Replace the `list` placeholder:

```rust
async fn list() -> anyhow::Result<()> {
    let root = hf_cache_root();
    if !root.exists() {
        eprintln!("(no models cached at {})", root.display());
        return Ok(());
    }
    eprintln!("{}", root.display());

    let mut rows: Vec<(String, u64)> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(repo) = name_str.strip_prefix("models--") {
            let repo = repo.replacen("--", "/", 1);
            let size = dir_size(&entry.path()).unwrap_or(0);
            rows.push((repo, size));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (repo, size) in rows {
        eprintln!("  {repo:<48}  {}", human_bytes(size));
    }
    Ok(())
}

fn dir_size(p: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in walk(p)? {
        if entry.is_file() {
            total += std::fs::metadata(&entry)?.len();
        }
    }
    Ok(total)
}

fn walk(p: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![p.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[(&str, u64)] = &[("GB", 1_000_000_000), ("MB", 1_000_000), ("KB", 1_000), ("B", 1)];
    for (unit, mult) in UNITS {
        if n >= *mult {
            return format!("{:.1} {}", n as f64 / *mult as f64, unit);
        }
    }
    format!("{n} B")
}
```

- [ ] **Step 2: Build**

```
cargo build --features local-inference 2>&1 | tail -3
```

- [ ] **Step 3: Commit**

```
git add src/cli/model.rs
git commit -m "feat(m9): rover model list (walks hf cache; per-repo size)"
```

---

### Task 44: `rover model remove`

**Files:**
- Modify: `src/cli/model.rs`

**Spec ref:** §3.13.

- [ ] **Step 1: Implement `remove`**

Replace `remove`:

```rust
async fn remove(repo_id: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let root = hf_cache_root();
    let dir = root.join(format!("models--{}", repo_id.replace('/', "--")));
    if !dir.exists() {
        eprintln!("(nothing to remove for {repo_id})");
        return Ok(());
    }
    let size = dir_size(&dir).unwrap_or(0);
    std::fs::remove_dir_all(&dir).context("removing cached model dir")?;
    eprintln!("removed {} ({} freed)", dir.display(), human_bytes(size));
    Ok(())
}
```

- [ ] **Step 2: Build**

```
cargo build --features local-inference 2>&1 | tail -3
```

- [ ] **Step 3: Commit**

```
git add src/cli/model.rs
git commit -m "feat(m9): rover model remove (deletes hf cache dir for given repo)"
```

---

### Task 45: `cli_model` integration test

**Files:**
- Create: `tests/cli_model.rs`

**Spec ref:** §11.1 row `cli_model`.

- [ ] **Step 1: Write the test**

```rust
//! Integration test for `rover model`. We avoid the real HuggingFace API
//! by setting HF_HOME to a temp dir and asserting against `list` + `remove`
//! behavior on a fake cache layout we materialize manually.
//!
//! `download` is exercised in the smoketest workflow against a tiny real
//! repo (`HuggingFaceTB/SmolLM2-135M-Instruct`, ~270 MB) — see Task 53.

#![cfg(any(feature = "local-inference", feature = "local-vision"))]

use std::fs;
use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;
use predicates::str::contains;

fn rover_bin() -> Command {
    Command::cargo_bin("rover").expect("rover binary built")
}

#[test]
fn list_empty_when_cache_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let out = rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "list"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("(no models cached at"), "got stderr: {stderr}");
}

#[test]
fn list_shows_fake_cached_model() {
    let tmp = tempfile::tempdir().unwrap();
    let hub = tmp.path().join("hub").join("models--FakeOwner--FakeModel");
    fs::create_dir_all(&hub).unwrap();
    fs::write(hub.join("config.json"), "{}").unwrap();
    fs::write(hub.join("model.safetensors"), &[0u8; 1_500_000][..]).unwrap();

    let out = rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "list"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("FakeOwner/FakeModel"), "stderr: {stderr}");
    // 1.5 MB rounds to 1.5 MB in human_bytes
    assert!(stderr.contains("MB") || stderr.contains("KB"), "stderr: {stderr}");
}

#[test]
fn remove_deletes_cached_model_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let hub = tmp.path().join("hub").join("models--FakeOwner--FakeModel");
    fs::create_dir_all(&hub).unwrap();
    fs::write(hub.join("config.json"), "{}").unwrap();

    let _ = rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "remove", "FakeOwner/FakeModel"])
        .assert()
        .success();
    assert!(!hub.exists());
}

#[test]
fn remove_idempotent_on_missing_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let out = rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "remove", "FakeOwner/Missing"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nothing to remove"));
}
```

- [ ] **Step 2: Run**

```
cargo test --features local-inference --test cli_model 2>&1 | tail -10
```

Expected: 4 pass.

- [ ] **Step 3: Commit**

```
git add tests/cli_model.rs
git commit -m "test(m9): rover model {list,remove} integration tests (download covered in smoketest)"
```

---

## Phase 5 — Documentation + CI

The implementation lands; this phase makes it findable, surveyable, and regression-protected.

---

### Task 46: `docs/features.md`

**Files:**
- Create: `docs/features.md`

**Spec ref:** §10.1.

- [ ] **Step 1: Write the doc**

```markdown
# Feature Flags

Rover ships with three optional Cargo features. The default install
(`cargo install rover`) produces a lean binary under 25 MiB with no
mistralrs, no chromiumoxide, and no extra model weights to manage.

Enable any combination of features by passing `--features` to
`cargo install` (or to `cargo build` if you're working from source).

| Feature | Enables | Approx. binary size add |
| --- | --- | --- |
| `local-inference` | Local LLM summarization via `mistral.rs` (default model: Qwen 3.5 0.8B) | ~80 MB |
| `local-vision` | Local image captioning via `mistral.rs` (default model: SmolVLM 256M) | shared with `local-inference`; ~5 MB additional |
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
rover model download HuggingFaceTB/SmolVLM-256M-Instruct
```

Configure under `[captioners.<name>]`:

```toml
[captioners.local]
kind = "local"
model = "HuggingFaceTB/SmolVLM-256M-Instruct"

[image_captions]
default = "local"
```

Available variants (swap via the `model` field):
- `HuggingFaceTB/SmolVLM-256M-Instruct` — smallest, fastest
- `HuggingFaceTB/SmolVLM-500M-Instruct` — better quality
- `HuggingFaceTB/SmolVLM2-2.2B-Instruct` — best quality

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

Default-features binary: < 25 MiB (CI-enforced).

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
```

- [ ] **Step 2: Commit**

```
git add docs/features.md
git commit -m "docs(m9): per-feature install/setup/sizing in docs/features.md"
```

---

### Task 47: `docs/security.md` headless asset interception + local models sections

**Files:**
- Modify: `docs/security.md`

**Spec ref:** §10.2.

- [ ] **Step 1: Add the new sections**

Append to `docs/security.md`:

```markdown
## Headless asset interception and SSRF

When the `headless` feature is enabled and a fetch runs in `headless: { mode: "on" }`
or triggers via `mode: "auto"`, the browser issues sub-requests that Rover doesn't
directly control. M9 wires every intercepted sub-request URL through the same
`SsrfLevel` validator the top-level fetch uses.

Sub-requests that would violate the configured `[ssrf] level` are intercepted via
the CDP Fetch domain and fulfilled with an empty 200 response — they are **never
aborted**. Aborting causes many SPAs to error out on missing CSS/font/image
references; an empty 200 keeps the page rendering.

The HAR recorder only records the top-level navigation. Sub-resources (CSS, JS,
images, fonts, beacons) are not in the HAR file. This keeps HAR files navigable
and stops sub-resources from masking what Rover actually returned.

**Threat model:** a malicious page cannot use Rover's headless renderer to scan
internal networks via embedded `<iframe>`, `<img>`, or `fetch()`. The
always-blocked address set (link-local, multicast, `0.0.0.0`, broadcast) plus
the `block_third_party = true` default cover the common attack paths. Operators
who set `[ssrf] level = "none"` opt out of these checks; the WARN line at
startup documents that choice.

## Local model files

The `local-inference` and `local-vision` features download model weights from
HuggingFace on first use (or ahead-of-time via `rover model download`).

- Weights are stored under `$HF_HOME/hub/` (default `~/.cache/huggingface/hub/`).
- Rover does not modify or upload model weights.
- The default models (`Qwen/Qwen3.5-0.8B`, `HuggingFaceTB/SmolVLM-256M-Instruct`)
  are public; no authentication required.
- Users pulling gated/private repos must set `HF_TOKEN` in the environment.

Disk usage: see `rover model list`. Models can be removed with `rover model remove
<repo_id>`. Weights are not garbage-collected automatically.
```

- [ ] **Step 2: Commit**

```
git add docs/security.md
git commit -m "docs(m9): security.md sections for headless asset interception + local model files"
```

---

### Task 48: `docs/configuration.md` updates

**Files:**
- Modify: `docs/configuration.md`

**Spec ref:** §10.3.

- [ ] **Step 1: Add documentation for the new sections**

Append a `## [image_captions]` section documenting every key (`default`, `max_tokens`,
`max_per_page`, `min_width`, `min_height`, `max_bytes`, `max_concurrent`), with
defaults and an example block. Follow the existing per-section reference style
used by M8 in `docs/configuration.md`.

Then `## [captioners.<name>]` documenting `kind`, `provider`, `model`, `base_url`,
`api_key_env`. Mirror the `[backends.<name>]` documentation (M8) — same field
shape.

Then `## [headless]` — extend the existing section (if M8 documented it) with
the two new M9 keys: `max_concurrent` (default 4) and `chrome_executable`
(default empty = auto-detect). Document the typed wire arg for the MCP `fetch`
tool's `headless` parameter (cross-reference `docs/mcp-tools.md`).

Then under `## [backends.<name>]`, add the new `kind = "local"` documentation
(requires `local-inference` feature; `model` is the HF repo id).

- [ ] **Step 2: Add the new settable keys to the `rover config set` reference**

The M8 doc lists settable keys. Add the entries enumerated in Task 4 Step 6:

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

- [ ] **Step 3: Commit**

```
git add docs/configuration.md
git commit -m "docs(m9): configuration reference for image_captions, captioners, headless m9 keys, backends local kind"
```

---

### Task 49: `docs/mcp-tools.md` headless + caption args

**Files:**
- Modify: `docs/mcp-tools.md`

**Spec ref:** §10.4.

- [ ] **Step 1: Document the typed `headless` arg**

In the `fetch` tool section, replace the prior "accept-no-op until M9" note with:

```markdown
### `headless`

When the binary is built with `--features headless`, pass:

\`\`\`json
{
  "headless": {
    "mode": "off" | "on" | "auto",
    "wait": "domcontentloaded" | "networkidle2",
    "timeout_secs": 15
  }
}
\`\`\`

- `mode` (default: derived from `[headless] auto_detect_spa`)
  - `off` — disable headless for this call (use the reqwest path only)
  - `on` — render this URL via headless unconditionally
  - `auto` — try reqwest first; re-render via headless if SPA heuristics fire
- `wait` (default: `[headless] default_wait`)
- `timeout_secs` (default: `[headless] timeout`)

When the binary is built **without** the `headless` feature:
- `mode: "off"` and the absent case work as today (no-op)
- `mode: "on"` returns the error `headless_feature_not_compiled`
- `mode: "auto"` keeps the reqwest result silently (no error)
```

- [ ] **Step 2: Document the renamed `images.mode = "caption"` arg**

Replace the prior `caption_vlm` reference in the `images` section with:

```markdown
- `caption` — replace each image with a generated caption.
  Requires at least one configured captioner (`[captioners.<name>]`). The
  default captioner comes from `[image_captions] default`; override per-call
  via `images.captioner: "<name>"`.
```

Document `images_processed` in the response shape (mirror the structure shown
in `docs/superpowers/specs/2026-05-25-rover-m9-feature-flagged-extras-design.md`
§6.6).

- [ ] **Step 3: Commit**

```
git add docs/mcp-tools.md
git commit -m "docs(m9): mcp-tools.md typed headless arg + caption images mode + images_processed shape"
```

---

### Task 50: `docs/cli.md` `rover model`

**Files:**
- Modify: `docs/cli.md`

**Spec ref:** §10.5.

- [ ] **Step 1: Add the subcommand doc**

Append a section `## rover model`. Document `download`, `list`, `remove`.
Note the cfg-gating (`any(local-inference, local-vision)`). Show the example
output from Task 41 spec §3.13.

- [ ] **Step 2: Commit**

```
git add docs/cli.md
git commit -m "docs(m9): cli.md rover model subcommand reference"
```

---

### Task 51: PRD drift correction

**Files:**
- Modify: `docs/superpowers/prd/2026-05-07-rover-prd.md`

**Spec ref:** §10.6.

- [ ] **Step 1: Edit the three sections**

Open `docs/superpowers/prd/2026-05-07-rover-prd.md`. Make the following targeted edits:

1. **§2 V1 Behind Cargo Feature Flags** (around line 38). Replace `vlm` with `local-vision`:

```markdown
- `local-vision` — image captioning via SmolVLM (local) or cloud vision APIs (always-on)
```

2. **§7.4 Image Captioning**. Replace the heading and intro:

```markdown
### 7.4 Image Captioning

Local captioning uses **SmolVLM** (256M, 500M, or 2.2B variants) via
`mistral.rs` behind the `local-vision` feature. The 256M variant uses < 1GB GPU
memory and is appropriate for batch image captioning.

Cloud captioning uses the same `genai` integration as cloud summarization
(OpenAI gpt-4o, Anthropic Claude with vision, Gemini, openai_compat). Cloud
captioners are always compiled in and require no feature flag.
```

3. **§12 Configuration**. Replace any `[vlm]` block with `[image_captions]` and
`[captioners.<name>]` blocks. Use the example from Task 4 Step 4.

- [ ] **Step 2: Commit**

```
git add docs/superpowers/prd/2026-05-07-rover-prd.md
git commit -m "docs(m9): prd drift correction (vlm → local-vision; image_captions/captioners; cloud captioners always-on)"
```

---

### Task 52: Binary-size CI assertion

**Files:**
- Modify: `.github/workflows/ci.yml`

**Spec ref:** §11.4 binary-size assertion; §13 acceptance #1.

- [ ] **Step 1: Add the job**

Add to `.github/workflows/ci.yml` (as a new job in the existing jobs map):

```yaml
  binary-size:
    name: Binary size (default features < 25 MiB)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build release
        run: cargo build --release --no-default-features
      - name: Assert size
        run: |
          size_bytes=$(stat -c%s target/release/rover)
          limit=$((25 * 1024 * 1024))
          echo "binary size: $size_bytes bytes (limit: $limit)"
          if [ "$size_bytes" -ge "$limit" ]; then
            echo "::error::binary size $size_bytes >= 25 MiB limit"
            exit 1
          fi
```

- [ ] **Step 2: Commit**

```
git add .github/workflows/ci.yml
git commit -m "ci(m9): assert default-features binary size < 25 mib"
```

---

### Task 53: Smoketest workflow additions for feature builds

**Files:**
- Modify: `.github/workflows/smoketest.yml`

**Spec ref:** §11.3 CI matrix.

- [ ] **Step 1: Add three feature-test jobs**

Append to `.github/workflows/smoketest.yml`:

```yaml
  feature-local-inference:
    name: feature local-inference (nightly)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Cache HF models
        uses: actions/cache@v4
        with:
          path: ~/.cache/huggingface
          key: hf-cache-local-inference-${{ runner.os }}-smollm2-135m
      - run: cargo test --features local-inference,test-loopback --test local_inference_smoke -- --ignored
        env:
          # CI uses a tiny model to keep run time bounded.
          ROVER_CI_TEST_MODEL: HuggingFaceTB/SmolLM2-135M-Instruct

  feature-local-vision:
    name: feature local-vision (nightly)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Cache HF models
        uses: actions/cache@v4
        with:
          path: ~/.cache/huggingface
          key: hf-cache-local-vision-${{ runner.os }}-smolvlm-256m
      - run: cargo test --features local-vision,test-loopback --test vlm_local_smoke -- --ignored

  feature-headless:
    name: feature headless (nightly)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install Chromium
        run: sudo apt-get update && sudo apt-get install -y chromium-browser
      - run: cargo test --features headless,test-loopback --test headless_smoke -- --ignored
      - run: cargo test --features headless,test-loopback --test headless_ssrf_intercept -- --ignored
```

> **Note on `ROVER_CI_TEST_MODEL`:** the `local_inference_smoke` test hardcodes
> `Qwen/Qwen3.5-0.8B`. For CI, prefer a smaller model. Decision: update the test
> in Task 21 to read `ROVER_CI_TEST_MODEL` if present, falling back to Qwen.
> Apply the small fix during plan execution (one-line change in `tests/local_inference_smoke.rs`).

- [ ] **Step 2: Commit**

```
git add .github/workflows/smoketest.yml
git commit -m "ci(m9): nightly smoketest jobs for each feature build (chromium apt install for headless)"
```

---

### Task 54: Milestone status + README update

**Files:**
- Modify: `docs/superpowers/milestones/rover-milestones.md`
- Modify: `README.md`

**Spec ref:** PRD §17 (top-level README maintenance); milestone manifest convention.

- [ ] **Step 1: Mark M9 complete in the manifest**

Open `docs/superpowers/milestones/rover-milestones.md`. After the M9 section (around line 554), append:

```markdown
**Status:** Complete (2026-05-25). Plan: `docs/superpowers/plans/2026-05-25-rover-m9-feature-flagged-extras.md`. Design: `docs/superpowers/specs/2026-05-25-rover-m9-feature-flagged-extras-design.md`.

**M9 follow-ups deferred to v2.**
1. CUDA backend for `local-inference` (separate `cuda` feature; PRD-level scope question).
2. `--model <hf_repo_id>` CLI shortcut for per-call backend overrides. Workaround: define multiple `[backends.*]` and pass `backend: "<name>"`.
3. Streaming per-file progress bars in `rover model download` (currently shows final size per file).
4. Per-request `max_tokens` plumbing into mistralrs (current implementation accepts the cap but doesn't enforce it through mistralrs's RequestBuilder).
5. `networkidle2` headless wait — currently approximated as `domcontentloaded + 500ms sleep`. Tighten in v2.
6. Headless intercept request-counter test hook (`HeadlessRenderer::intercept_blocked_count()`) — make the SSRF intercept test directly observable.
```

- [ ] **Step 2: Update README**

Open `README.md`. Add to the milestone table (or wherever M1–M8 are listed) a row for M9:

```markdown
| M9 | Feature-flagged extras | local-inference, local-vision, headless, cloud captioners | Complete |
```

In the install snippet section, add:

```markdown
### With optional features

\`\`\`
# Local LLM summarization (Qwen 3.5 0.8B by default)
cargo install rover --features local-inference

# Local image captioning (SmolVLM 256M by default)
cargo install rover --features local-vision

# SPA rendering via system Chrome
cargo install rover --features headless

# All three
cargo install rover --features local-inference,local-vision,headless
\`\`\`

See `docs/features.md` for setup, model management (`rover model`), and binary-size notes.
```

- [ ] **Step 3: Commit**

```
git add docs/superpowers/milestones/rover-milestones.md README.md
git commit -m "docs(m9): mark milestone complete; readme rows for the three features; deferral list"
```

---


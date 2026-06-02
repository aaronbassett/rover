# Captioner Reliability Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make image captioning work for OpenAI-compatible local servers (Ollama/LM Studio) out of the box, and make `rover doctor` actually validate that path.

**Architecture:** Four targeted fixes against the existing `CloudCaptioner` path. (A) Centralize `openai_compat` base_url normalization in the shared `build_client` so every caller — summarizer, captioner, both doctor probes — gets it. (B) Make the two `rover doctor` auth checks probe keyless `openai_compat` backends instead of skipping them. (C) Give the doctor caption probe a non-degenerate image and a usable token budget. (D) Log caption/download failures and label download failures correctly.

**Tech Stack:** Rust (edition 2024, MSRV 1.96.0), `genai` 0.4.4, `tokio`, `wiremock` (dev), `async-trait`.

**Spec:** `docs/superpowers/specs/2026-06-02-captioner-reliability-fix-design.md`

---

## Environment setup (do this once, before any task)

This machine has a Homebrew `rustc 1.93.1` ahead of rustup on `$PATH` that shadows the
1.96.0 toolchain rover requires. Put the 1.96.0 toolchain first for this shell so every
`cargo`/`rustc`/`git commit` (lefthook runs fmt + clippy) uses it:

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
export RUSTC="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin/rustc"
rustc --version   # must print 1.96.0, NOT 1.93.1
```

All `cargo` commands below assume this is done. Commits run the lefthook pre-commit hook
(fmt + clippy, ~4 min for clippy) — that is expected; let it run.

**Branch:** `fix/captioner-reliability` (already created off `main`, with the spec commit cherry-picked).

---

## File structure

- **Modify** `src/summarizer/cloud.rs` — add `normalize_openai_compat_base_url` (+ its unit tests), call it inside `build_client`.
- **Modify** `src/summarizer/registry.rs` — remove that function, its unit tests, and the redundant pre-normalization block in `build_one`.
- **Modify** `tests/vlm_cloud_smoke.rs` — add the no-trailing-slash regression test (Fix A).
- **Modify** `src/doctor/checks.rs` — probe keyless `openai_compat` in `CaptionersAuthenticate` and `BackendsAuthenticate` (Fix B); replace the probe image + budget (Fix C).
- **Modify** `src/doctor/mod.rs` — add the keyless-captioner doctor test (Fix B/C) to its `mod tests`.
- **Modify** `src/extractor/images.rs` — relabel download failures + add `warn!` logging (Fix D), and add reason-label tests.

---

## Task 1 — Fix A: centralize `openai_compat` base_url normalization in `build_client`

**Files:**
- Modify: `tests/vlm_cloud_smoke.rs` (add regression test)
- Modify: `src/summarizer/cloud.rs:59-95` (add function + call it in `build_client`); add unit tests to its `#[cfg(test)] mod` blocks
- Modify: `src/summarizer/registry.rs:165-182` (remove pre-normalization block), `:229-243` (remove function), `:439-483` (remove moved unit tests)

- [ ] **Step 1: Write the failing regression test**

Add this test function to the end of `tests/vlm_cloud_smoke.rs` (before the final closing — it's a top-level `#[tokio::test]`, same style as the existing tests in that file):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_compat_base_url_without_trailing_slash_is_normalized() {
    // base_url lacks the trailing slash (and the `/v1/`); the captioner must
    // still POST to `/v1/chat/completions`. Before centralizing normalization
    // in build_client this 404'd, because only the summarizer path normalized.
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
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server)
        .await;

    // NOTE: no trailing slash, no `/v1`.
    let cap = CloudCaptioner::new(
        "test",
        ProviderKind::OpenAiCompat,
        "test-model",
        Some(server.uri()),
        Some("dummy".into()),
    )
    .unwrap();

    let caption = cap.caption(PNG, None, 50).await.unwrap();
    assert_eq!(caption, "ok");
    let recv = server.received_requests().await.unwrap();
    assert_eq!(recv.len(), 1);
    assert_eq!(recv[0].url.path(), "/v1/chat/completions");
}
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cargo test --test vlm_cloud_smoke openai_compat_base_url_without_trailing_slash_is_normalized -- --nocapture
```

Expected: FAIL — `caption(...)` returns an error (genai posts to a malformed path; wiremock returns 404), so `.unwrap()` panics, or `recv.len()` is 0.

- [ ] **Step 3: Add the normalizer to `src/summarizer/cloud.rs`**

Insert this function immediately **after** `build_client` (after line 95, before `resolve_request_model`):

```rust
/// Normalize a user-supplied openai_compat base URL so it ends with `/v1/`.
/// Accepts inputs missing the trailing slash, missing the `/v1/` segment, or
/// already-correct. Idempotent.
///
/// Examples:
/// - `http://localhost:1234`        → `http://localhost:1234/v1/`
/// - `http://localhost:1234/`       → `http://localhost:1234/v1/`
/// - `http://localhost:1234/v1`     → `http://localhost:1234/v1/`
/// - `http://localhost:1234/v1/`    → unchanged
/// - `https://api.example.com/custom/v1/` → unchanged
/// - `https://api.example.com/custom/`    → `https://api.example.com/custom/v1/`
fn normalize_openai_compat_base_url(base: &str) -> String {
    let trimmed = base.trim();
    let with_slash = if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    };
    if with_slash.ends_with("/v1/") {
        return with_slash;
    }
    format!("{with_slash}v1/")
}
```

- [ ] **Step 4: Call the normalizer inside `build_client`**

In `src/summarizer/cloud.rs`, replace the `let base = ...` binding in the `OpenAiCompat` branch (currently lines 67-69):

```rust
        let base = base_url
            .ok_or_else(|| "openai_compat requires base_url".to_string())?
            .to_string();
```

with:

```rust
        let base = normalize_openai_compat_base_url(
            base_url.ok_or_else(|| "openai_compat requires base_url".to_string())?,
        );
```

- [ ] **Step 5: Remove the now-redundant pre-normalization from `src/summarizer/registry.rs`**

Replace the block at lines 165-182 (the `let base_url = if provider_kind == ... ` computation **and** the following `if let (Some(orig), Some(norm)) = ... { tracing::info!(...) }`) with just:

```rust
            let base_url = cfg.base_url.clone();
```

The following `CloudBackend::new(name, provider_kind, model, base_url, api_key)` call is unchanged — it now passes the raw base_url, and `build_client` normalizes it.

- [ ] **Step 6: Delete the function and its tests from `src/summarizer/registry.rs`**

- Delete the `normalize_openai_compat_base_url` function and its doc comment (lines 218-243).
- Delete these five test functions from the `#[cfg(test)] mod tests` block: `normalize_base_url_appends_v1_slash_when_missing`, `normalize_base_url_idempotent_on_already_normalized`, `normalize_base_url_leaves_custom_paths_with_v1_alone`, `normalize_base_url_appends_v1_to_custom_paths_without_v1`, `normalize_base_url_trims_whitespace` (lines 439-483).

- [ ] **Step 7: Re-add the moved unit tests in `src/summarizer/cloud.rs`**

Add this test module at the **end** of `src/summarizer/cloud.rs` (it's a new sibling `#[cfg(test)] mod`, alongside the existing `provider_tests` and `cloud_tests`):

```rust
#[cfg(test)]
mod normalize_tests {
    use super::normalize_openai_compat_base_url;

    #[test]
    fn appends_v1_slash_when_missing() {
        assert_eq!(
            normalize_openai_compat_base_url("http://localhost:1234"),
            "http://localhost:1234/v1/"
        );
        assert_eq!(
            normalize_openai_compat_base_url("http://localhost:1234/"),
            "http://localhost:1234/v1/"
        );
        assert_eq!(
            normalize_openai_compat_base_url("http://localhost:1234/v1"),
            "http://localhost:1234/v1/"
        );
    }

    #[test]
    fn idempotent_on_already_normalized() {
        let already = "http://localhost:1234/v1/";
        assert_eq!(normalize_openai_compat_base_url(already), already);
    }

    #[test]
    fn leaves_custom_paths_with_v1_alone() {
        assert_eq!(
            normalize_openai_compat_base_url("https://api.example.com/custom/v1/"),
            "https://api.example.com/custom/v1/"
        );
    }

    #[test]
    fn appends_v1_to_custom_paths_without_v1() {
        assert_eq!(
            normalize_openai_compat_base_url("https://api.example.com/custom/"),
            "https://api.example.com/custom/v1/"
        );
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(
            normalize_openai_compat_base_url("  http://localhost:1234  "),
            "http://localhost:1234/v1/"
        );
    }
}
```

- [ ] **Step 8: Run the affected tests and confirm they pass**

```bash
cargo test --test vlm_cloud_smoke
cargo test --lib summarizer::cloud
cargo test --lib summarizer::registry
```

Expected: PASS. In particular `openai_compat_base_url_without_trailing_slash_is_normalized` now passes, and the moved `normalize_tests::*` pass in their new home.

- [ ] **Step 9: Commit**

```bash
git add src/summarizer/cloud.rs src/summarizer/registry.rs tests/vlm_cloud_smoke.rs
git commit -m "$(cat <<'EOF'
fix(vlm): normalize openai_compat base_url in shared build_client

Captioners passed the raw base_url straight to genai, so the documented
`http://host:11434/v1` (no trailing slash) 404'd. Move
normalize_openai_compat_base_url into summarizer::cloud and call it inside
build_client, the single chokepoint both CloudBackend and CloudCaptioner
(and both doctor probes) route through. Remove the now-redundant
summarizer-side normalization.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — Fix B + C: `rover doctor` probes keyless local captioners with a usable probe

**Files:**
- Modify: `src/doctor/checks.rs:228-318` (`BackendsAuthenticate` filter), `:321-412` (`CaptionersAuthenticate` filter + probe image/budget)
- Modify: `src/doctor/mod.rs` (`#[cfg(test)] mod tests`) — add the keyless-captioner test

- [ ] **Step 1: Write the failing doctor test**

Add this to the `#[cfg(test)] mod tests` block in `src/doctor/mod.rs` (after `captioners_authenticate_skips_when_no_cloud_configured`, ~line 169):

```rust
    #[tokio::test]
    async fn captioners_authenticate_probes_keyless_openai_compat() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // A fake OpenAI-compatible server that answers the caption probe.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "probe",
                "object": "chat.completion",
                "created": 0,
                "model": "probe-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "a small blue square"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path().join("rover.db")).await.unwrap();
        let mut cfg = Config::default();
        cfg.output.dir = Some(tmp.path().to_path_buf());
        // Keyless: no api_key_env. Trailing slash so this test is independent
        // of Fix A.
        cfg.captioners.insert(
            "ollama".to_string(),
            crate::config::CaptionerConfig {
                kind: "cloud".into(),
                provider: Some("openai_compat".into()),
                model: Some("probe-model".into()),
                base_url: Some(format!("{}/v1/", server.uri())),
                api_key_env: None,
            },
        );
        let ctx = CheckCtx {
            config: Arc::new(cfg),
            db,
        };

        let r = checks::CaptionersAuthenticate.run(&ctx).await;
        // Before Fix B this returned Skip (the keyless captioner was filtered
        // out). Now it must be probed and pass.
        assert_eq!(r.status, CheckStatus::Ok, "detail: {:?}", r.detail);
    }
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cargo test --lib doctor::tests::captioners_authenticate_probes_keyless_openai_compat -- --nocapture
```

Expected: FAIL — assertion `Ok` but actual `Skip` (the keyless captioner is filtered out before any probe).

- [ ] **Step 3: Add the probe constants to `src/doctor/checks.rs`**

Add these two module-level constants near the top of `src/doctor/checks.rs` (after the `use` lines, before `pub struct SqliteOpen;`):

```rust
/// Non-degenerate probe image for the caption check: an 8x8 solid-colour PNG.
/// A 1x1 transparent pixel makes some models emit zero tokens, which genai
/// surfaces as an error even though captioning works.
pub(crate) const CAPTION_PROBE_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
    0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x08,
    0x08, 0x02, 0x00, 0x00, 0x00, 0x4b, 0x6d, 0x29, 0xdc, 0x00, 0x00, 0x00,
    0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xd0, 0x88, 0x3a, 0x81,
    0x15, 0x31, 0x0c, 0x2d, 0x09, 0x00, 0x14, 0xa8, 0x52, 0x81, 0xea, 0x01,
    0xcb, 0xb1, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

/// Token budget for the caption probe. Must be > 1: a 1-token budget makes
/// thinking models (e.g. Gemini 2.5) emit no content and report an error.
pub(crate) const CAPTION_PROBE_MAX_TOKENS: usize = 64;
```

- [ ] **Step 4: Update `CaptionersAuthenticate` to probe keyless local captioners and use the new probe**

In `src/doctor/checks.rs`, in `CaptionersAuthenticate::run`:

(a) Delete the local `const PROBE_PNG: &[u8] = &[ ... ];` (lines 329-336).

(b) Replace the second `.filter(...)` (lines 343-349) with:

```rust
            .filter(|(_, c)| {
                let has_key = c
                    .api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                let keyless_local = c.provider.as_deref() == Some("openai_compat")
                    && c.base_url.as_deref().map(|b| !b.is_empty()).unwrap_or(false);
                has_key || keyless_local
            })
```

(c) Update the skip detail string (line 355) from `"no cloud captioners with non-empty api_key_env"` to:

```rust
                detail: Some(
                    "no cloud captioners with credentials or a local base_url".into(),
                ),
```

(d) Replace the probe call (line 389) `cap.caption(PROBE_PNG, None, 1)` with:

```rust
                cap.caption(CAPTION_PROBE_PNG, None, CAPTION_PROBE_MAX_TOKENS),
```

- [ ] **Step 5: Apply the same keyless filter to `BackendsAuthenticate`**

In `src/doctor/checks.rs`, in `BackendsAuthenticate::run`, replace the second `.filter(...)` (lines 241-247) with:

```rust
            .filter(|(_, c)| {
                let has_key = c
                    .api_key_env
                    .as_deref()
                    .and_then(|e| std::env::var(e).ok())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                let keyless_local = c.provider.as_deref() == Some("openai_compat")
                    && c.base_url.as_deref().map(|b| !b.is_empty()).unwrap_or(false);
                has_key || keyless_local
            })
```

And update its skip detail (line 253) from `"no configured cloud backends with non-empty api_key_env"` to:

```rust
                detail: Some(
                    "no configured cloud backends with credentials or a local base_url"
                        .to_string(),
                ),
```

- [ ] **Step 6: Add a constants-sanity test**

Add to the `#[cfg(test)] mod tests` block in `src/doctor/mod.rs`:

```rust
    #[test]
    fn caption_probe_constants_are_sane() {
        // Non-degenerate image and a budget that leaves room for output.
        assert!(checks::CAPTION_PROBE_PNG.len() > 67, "probe image must be larger than the old 1x1");
        assert!(checks::CAPTION_PROBE_MAX_TOKENS > 1, "probe budget must exceed 1 token");
    }
```

(The constants are declared `pub(crate)` in Step 3, so this test can reference them.)

- [ ] **Step 7: Run the doctor tests and confirm they pass**

```bash
cargo test --lib doctor::tests -- --nocapture
```

Expected: PASS — `captioners_authenticate_probes_keyless_openai_compat` is now `Ok`, `caption_probe_constants_are_sane` passes, and the existing `captioners_authenticate_skips_when_no_cloud_configured` / `backends_authenticate_skips_when_no_cloud_configured` still pass (a config with no captioners/backends still skips).

- [ ] **Step 8: Commit**

```bash
git add src/doctor/checks.rs src/doctor/mod.rs
git commit -m "$(cat <<'EOF'
fix(doctor): probe keyless openai_compat captioners with a usable probe

doctor's captioners_authenticate/backends_authenticate skipped any backend
without a non-empty api_key_env, so local Ollama/LM Studio captioners (which
are keyless) were never validated. Probe openai_compat backends that have a
base_url. Also replace the 1x1 transparent probe PNG + max_tokens=1 (which
made Gemini emit no content and error) with an 8x8 PNG and a 64-token budget.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — Fix D: log and correctly label caption/download failures

**Files:**
- Modify: `src/extractor/images.rs:201-217` (download-failure arm), `:254-270` (caption-failure arm); add tests to its `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing reason-label tests**

Add a fake captioner and two tests to the `#[cfg(test)] mod tests` block in `src/extractor/images.rs`. Put the fake captioner near the top of the test module (after the `client()` / `setup_paths()` helpers):

```rust
    use crate::vlm::VlmError;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// A captioner whose `caption()` always errors — for exercising the
    /// caption-failure path.
    struct FailingCaptioner;

    #[async_trait::async_trait]
    impl VlmCaptioner for FailingCaptioner {
        fn name(&self) -> &str {
            "fail"
        }
        fn model_id(&self) -> &str {
            "fail-model"
        }
        async fn caption(
            &self,
            _image_bytes: &[u8],
            _alt: Option<&str>,
            _max_tokens: usize,
        ) -> Result<String, VlmError> {
            Err(VlmError::Unavailable {
                name: "fail".into(),
                reason: "boom".into(),
            })
        }
    }

    fn failing_registry() -> CaptionerRegistry {
        let mut map: HashMap<String, Arc<dyn VlmCaptioner>> = HashMap::new();
        map.insert("fail".to_string(), Arc::new(FailingCaptioner));
        CaptionerRegistry::__test_construct(map, Some("fail".to_string()))
    }

    #[tokio::test]
    async fn download_failure_is_labelled_download_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Every request 500s, so the full download fails (classify falls
        // through to Caption with no dims, then download_image_bytes errors).
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let p = setup_paths();
        let md = format!("Look ![alt]({}/img.png) here.", server.uri());
        let f = ImageCaptionFilters::default();
        let reg = failing_registry();
        let r = apply(
            &md,
            &ImagesMode::Caption,
            &p,
            &client(),
            Some(&reg),
            &f,
            None,
            SsrfLevel::Loopback,
        )
        .await
        .unwrap();

        assert_eq!(r.images_processed.len(), 1);
        assert_eq!(r.images_processed[0].decision, "skipped");
        assert_eq!(r.images_processed[0].reason.as_deref(), Some("download_error"));
    }

    #[tokio::test]
    async fn captioner_failure_is_labelled_captioner_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // 1x1 transparent PNG; min dims set to 0 so it passes the gate.
        let png: [u8; 67] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let server = MockServer::start().await;
        // Serve the PNG for both classify's probe and the full download.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(&png[..]))
            .mount(&server)
            .await;

        let p = setup_paths();
        let md = format!("Look ![alt]({}/img.png) here.", server.uri());
        let f = ImageCaptionFilters {
            min_width: 0,
            min_height: 0,
            ..Default::default()
        };
        let reg = failing_registry();
        let r = apply(
            &md,
            &ImagesMode::Caption,
            &p,
            &client(),
            Some(&reg),
            &f,
            None,
            SsrfLevel::Loopback,
        )
        .await
        .unwrap();

        assert_eq!(r.images_processed.len(), 1);
        assert_eq!(r.images_processed[0].decision, "skipped");
        assert_eq!(r.images_processed[0].reason.as_deref(), Some("captioner_error"));
    }
```

- [ ] **Step 2: Run them and confirm the download test fails**

```bash
cargo test --lib extractor::images::tests::download_failure_is_labelled_download_error -- --nocapture
cargo test --lib extractor::images::tests::captioner_failure_is_labelled_captioner_error -- --nocapture
```

Expected: `download_failure_is_labelled_download_error` FAILS (reason is currently `"captioner_error"`). `captioner_failure_is_labelled_captioner_error` PASSES (guards the unchanged label).

- [ ] **Step 3: Relabel the download failure and add logging**

In `src/extractor/images.rs`, in the download-failure arm (lines 201-217), change `reason: Some("captioner_error".into())` to `reason: Some("download_error".into())`, and add a `warn!` before the `processed.push`. The arm becomes:

```rust
                Err(e) => {
                    *images_failed += 1;
                    tracing::warn!(
                        target: "rover::extractor",
                        url = %src,
                        err = %e,
                        "image download failed during captioning; keeping alt text"
                    );
                    processed.push(ImageProcessed {
                        src: src.to_string(),
                        decision: "skipped".into(),
                        reason: Some("download_error".into()),
                        captioner: Some(captioner.name().to_string()),
                        caption: None,
                        dimensions: dims.map(|(w, h)| ImageDims {
                            width: w,
                            height: h,
                        }),
                        bytes: None,
                        error: Some(format!("download: {e}")),
                    });
                    return alt.to_string();
                }
```

- [ ] **Step 4: Add logging to the caption-failure arm**

In `src/extractor/images.rs`, in the caption-failure arm (lines 254-270), add a `warn!` before the `processed.push`. The arm becomes:

```rust
                    Err(e) => {
                        *images_failed += 1;
                        tracing::warn!(
                            target: "rover::extractor",
                            url = %src,
                            err = %e,
                            "captioner failed; keeping alt text"
                        );
                        processed.push(ImageProcessed {
                            src: src.to_string(),
                            decision: "skipped".into(),
                            reason: Some("captioner_error".into()),
                            captioner: Some(captioner.name().to_string()),
                            caption: None,
                            dimensions: dims.map(|(w, h)| ImageDims {
                                width: w,
                                height: h,
                            }),
                            bytes: None,
                            error: Some(e.to_string()),
                        });
                        return alt.to_string();
                    }
```

- [ ] **Step 5: Run the tests and confirm both pass**

```bash
cargo test --lib extractor::images::tests -- --nocapture
```

Expected: PASS — both reason-label tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/extractor/images.rs
git commit -m "$(cat <<'EOF'
fix(extractor): log and correctly label caption/download failures

Caption and image-download failures degraded to a silent skipped entry with
no log, and download failures were mislabelled reason=captioner_error. Add a
warn! at both failure sites and label download failures download_error so
they are distinguishable from genai/captioner failures.

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — Full verification and docs check

**Files:** none changed unless the docs check turns something up.

- [ ] **Step 1: Run the whole suite**

```bash
cargo test
```

Expected: PASS (all tests, including the new ones).

- [ ] **Step 2: Lint and format check**

```bash
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Expected: clean (no warnings, no diffs). Fix any issues and re-run.

- [ ] **Step 3: Verify the docs claim now holds for captioners**

Read `docs/configuration.md` around the `[captioners.<name>]` / `base_url` rows (the "auto-normalized to end in `/v1/`" note). Confirm the wording does not imply summarizer-only. If it does, adjust it to state the normalization applies to both `[backends.*]` and `[captioners.*]`. Commit only if a wording change was needed:

```bash
git add docs/configuration.md
git commit -m "docs: note openai_compat base_url normalization applies to captioners

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

- [ ] **Step 4 (optional, manual): live re-confirm against Ollama**

With Ollama running and `moondream` pulled, and using `./rover-confirm.toml` but with `base_url = "http://localhost:11434/v1"` (no trailing slash, to prove Fix A):

```bash
{ printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"m","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fetch_tool","arguments":{"url":"https://en.wikipedia.org/wiki/Cat","force_refresh":true,"images":{"mode":"caption","captioner":"ollama"}}}}' ; \
  sleep 120 ; } | ./target/release/rover --config ./rover-confirm.toml mcp
```

Expected: images come back `decision: captioned` (not `skipped`/`captioner_error`) with no trailing slash in `base_url`. (Rebuild first with the forced toolchain: `cargo build --release`.)

---

## Notes

- `CaptionerRegistry::__test_construct` is gated behind `#[cfg(any(test, feature = "test-loopback"))]`; the `images.rs` and `doctor` tests run under `#[cfg(test)]` in the same crate, so it is available.
- Out of scope (per spec): moondream caption quality; Wikimedia `429`s on rapid image downloads.
- `./rover-confirm.toml` is an untracked local file — do not commit it.

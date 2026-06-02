//! Image transformation modes.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use url::Url;

use crate::extractor::frontmatter::{ImageDims, ImageProcessed};
use crate::extractor::options::ImageCaptionFilters;
use crate::extractor::options::ImagesMode;
use crate::extractor::output::OutputPaths;
use crate::extractor::pipeline::ExtractorError;
use crate::fetcher::dns::SSRF_LEVEL;
use crate::fetcher::ssrf::{SsrfLevel, validate_url_for_level};
use crate::storage::Db;
use crate::vlm::{CaptionerRegistry, VlmCaptioner};

static INLINE_IMG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!\[(?P<alt>[^\]]*)\]\((?P<src>[^)\s]+)(?P<rest>[^)]*)\)").unwrap()
});

#[derive(Debug, Default, Clone)]
pub struct ImagesApplied {
    pub markdown: String,
    pub images_seen: usize,
    pub images_downloaded: usize,
    pub images_failed: usize,
    /// One annotation per `<img>` processed in `Caption` mode. Empty for
    /// every other `ImagesMode`. Surfaced via the `images_processed:`
    /// frontmatter sidecar in Task 11.
    pub images_processed: Vec<ImageProcessed>,
}

#[allow(clippy::too_many_arguments)]
pub async fn apply(
    markdown: &str,
    mode: &ImagesMode,
    output_paths: &OutputPaths,
    http: &reqwest::Client,
    captioners: Option<&CaptionerRegistry>,
    filters: &ImageCaptionFilters,
    db: Option<&Db>,
    ssrf_level: SsrfLevel,
) -> Result<ImagesApplied, ExtractorError> {
    let mut images_seen = 0usize;
    let mut images_downloaded = 0usize;
    let mut images_failed = 0usize;
    let mut images_processed: Vec<ImageProcessed> = Vec::new();

    // Resolve a captioner up front when we're in Caption mode so we can fail
    // fast (CaptionerNotConfigured) before fetching any images.
    let captioner: Option<std::sync::Arc<dyn VlmCaptioner>> = if matches!(mode, ImagesMode::Caption)
    {
        let reg = captioners.ok_or(ExtractorError::CaptionerNotConfigured)?;
        let name = filters
            .captioner_override
            .as_deref()
            .or_else(|| reg.default_name())
            .ok_or(ExtractorError::CaptionerNotConfigured)?;
        Some(reg.get(name).map_err(|e| ExtractorError::CaptionerCall {
            name: name.to_string(),
            source: Box::new(e),
        })?)
    } else {
        None
    };
    let mut captioned_so_far = 0usize;

    // Two-step: enumerate matches, then transform. Async download requires
    // we can't use `replace_all` directly. `filter_map` guards against any
    // theoretical Captures shape where a named group is absent — the regex
    // makes every group required, but skipping (vs. panicking) on malformed
    // input from an adversarial page is the safer default.
    let matches: Vec<(usize, usize, String, String, String)> = INLINE_IMG
        .captures_iter(markdown)
        .filter_map(|c| {
            let m = c.get(0)?;
            let alt = c.name("alt")?.as_str().to_string();
            let src = c.name("src")?.as_str().to_string();
            let rest = c.name("rest")?.as_str().to_string();
            Some((m.start(), m.end(), alt, src, rest))
        })
        .collect();

    let mut out = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for (start, end, alt, src, rest) in matches {
        images_seen += 1;
        out.push_str(&markdown[cursor..start]);
        cursor = end;
        let replacement: String = match mode {
            ImagesMode::Keep => markdown[start..end].to_string(),
            ImagesMode::Drop => String::new(),
            ImagesMode::AltTextOnly => alt.clone(),
            ImagesMode::Download => {
                match download_one(http, &src, output_paths, ssrf_level).await {
                    Ok(local) => {
                        images_downloaded += 1;
                        format!("![{alt}]({local}{rest})")
                    }
                    Err(e) => {
                        images_failed += 1;
                        tracing::warn!(
                            target: "rover::extractor",
                            url = %src,
                            err = %e,
                            "image download failed; keeping original"
                        );
                        markdown[start..end].to_string()
                    }
                }
            }
            ImagesMode::Caption => match captioner.as_ref() {
                Some(cap) => {
                    caption_one_image(
                        cap.as_ref(),
                        http,
                        db,
                        filters,
                        &alt,
                        &src,
                        &rest,
                        &mut captioned_so_far,
                        &mut images_failed,
                        &mut images_processed,
                        ssrf_level,
                    )
                    .await
                }
                None => {
                    // Unreachable while the resolution block above stays in
                    // sync with this match arm; we fall back to keeping the
                    // original markdown rather than panicking on what would
                    // be an internal invariant slip.
                    tracing::error!(
                        target: "rover::extractor",
                        "internal: captioner missing in Caption mode; keeping original image"
                    );
                    images_failed += 1;
                    markdown[start..end].to_string()
                }
            },
        };
        out.push_str(&replacement);
    }
    out.push_str(&markdown[cursor..]);

    Ok(ImagesApplied {
        markdown: out,
        images_seen,
        images_downloaded,
        images_failed,
        images_processed,
    })
}

/// Caption a single image. Returns the replacement markdown for the image
/// (either the freshly-captioned `![caption](src)` form or a fallback to
/// the alt text when the image was skipped or the captioner errored).
/// Pushes one `ImageProcessed` annotation into `processed` per call.
#[allow(clippy::too_many_arguments)]
async fn caption_one_image(
    captioner: &dyn VlmCaptioner,
    http: &reqwest::Client,
    db: Option<&Db>,
    filters: &ImageCaptionFilters,
    alt: &str,
    src: &str,
    rest: &str,
    captioned_so_far: &mut usize,
    images_failed: &mut usize,
    processed: &mut Vec<ImageProcessed>,
    ssrf_level: SsrfLevel,
) -> String {
    let decision = classify(src, rest, http, *captioned_so_far, filters, ssrf_level).await;
    match decision {
        CaptionDecision::Skip {
            reason,
            dims,
            bytes,
        } => {
            processed.push(ImageProcessed {
                src: src.to_string(),
                decision: "skipped".into(),
                reason: Some(skip_reason_to_str(&reason).to_string()),
                captioner: None,
                caption: None,
                dimensions: dims.map(|(w, h)| ImageDims {
                    width: w,
                    height: h,
                }),
                bytes,
                error: None,
            });
            alt.to_string()
        }
        CaptionDecision::Caption { dims } => {
            let bytes = match download_image_bytes(http, src, ssrf_level).await {
                Ok(b) => b,
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
            };
            // Cache lookup.
            let cached = if let Some(db) = db {
                crate::vlm::cache::lookup(
                    db,
                    &bytes,
                    captioner.name(),
                    captioner.model_id(),
                    filters.max_tokens,
                )
                .await
                .unwrap_or(None)
            } else {
                None
            };
            let alt_hint = if alt.is_empty() { None } else { Some(alt) };
            let caption = match cached {
                Some(c) => c,
                None => match captioner
                    .caption(&bytes, alt_hint, filters.max_tokens)
                    .await
                {
                    Ok(c) => {
                        if let Some(db) = db {
                            let _ = crate::vlm::cache::insert(
                                db,
                                &bytes,
                                captioner.name(),
                                captioner.model_id(),
                                filters.max_tokens,
                                &c,
                            )
                            .await;
                        }
                        c
                    }
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
                },
            };
            *captioned_so_far += 1;
            processed.push(ImageProcessed {
                src: src.to_string(),
                decision: "captioned".into(),
                reason: None,
                captioner: Some(captioner.name().to_string()),
                caption: Some(caption.clone()),
                dimensions: dims.map(|(w, h)| ImageDims {
                    width: w,
                    height: h,
                }),
                bytes: None,
                error: None,
            });
            format!("![{caption}]({src}{rest})")
        }
    }
}

fn skip_reason_to_str(r: &SkipReason) -> &'static str {
    match r {
        SkipReason::BelowMinDimensions => "below_min_dimensions",
        SkipReason::AboveMaxBytes => "above_max_bytes",
        SkipReason::PerPageBudget => "per_page_budget",
        SkipReason::CaptionerError => "captioner_error",
        SkipReason::DimensionsIndeterminate => "dimensions_indeterminate",
    }
}

/// Pre-flight SSRF check for an image URL, mirroring the primary fetch path
/// (`fetcher::fetch_url_conditional`): resolve the host and validate every
/// address before connecting. This is what blocks literal-IP targets such as
/// cloud-metadata `169.254.169.254` — reqwest skips the custom dial-time
/// resolver for literal IPs, so the `SSRF_LEVEL.scope` wrapping alone would
/// not catch them. The scope wrapping is still applied at each send so that
/// hostname targets are re-validated at dial time (DNS-rebinding TOCTOU).
async fn ssrf_preflight(url: &Url, src: &str, level: SsrfLevel) -> Result<(), ExtractorError> {
    validate_url_for_level(url, level, None)
        .await
        .map_err(|source| ExtractorError::ImageSsrf {
            url: src.to_string(),
            source,
        })
}

async fn download_image_bytes(
    http: &reqwest::Client,
    src: &str,
    ssrf_level: SsrfLevel,
) -> Result<Vec<u8>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    ssrf_preflight(&url, src, ssrf_level).await?;
    let resp = SSRF_LEVEL
        .scope(ssrf_level, http.get(url.clone()).send())
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?
        .error_for_status()
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    Ok(resp
        .bytes()
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?
        .to_vec())
}

async fn download_one(
    http: &reqwest::Client,
    src: &str,
    output_paths: &OutputPaths,
    ssrf_level: SsrfLevel,
) -> Result<String, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    ssrf_preflight(&url, src, ssrf_level).await?;
    let resp = SSRF_LEVEL
        .scope(ssrf_level, http.get(url.clone()).send())
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    let resp = resp
        .error_for_status()
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    let ext = sniff_ext(&resp, &url);
    let bytes = resp
        .bytes()
        .await
        .map_err(|source| ExtractorError::ImageDownload {
            url: src.to_string(),
            source,
        })?;
    let path = output_paths.image_path(&url, &ext);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExtractorError::ImageWrite {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&path, &bytes).map_err(|source| ExtractorError::ImageWrite {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path.canonicalize().unwrap_or(path).display().to_string())
}

static IMG_WIDTH_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bwidth\s*=\s*"?(\d+)"?"#).unwrap());
static IMG_HEIGHT_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bheight\s*=\s*"?(\d+)"?"#).unwrap());

/// Extract `<img width=… height=…>` from the markdown image's `rest`
/// capture (the tail between the URL and the closing paren). Returns
/// `(width, height)` when both are present and parse as positive integers.
pub(crate) fn html_attr_dims(rest: &str) -> Option<(u32, u32)> {
    let w = IMG_WIDTH_ATTR
        .captures(rest)?
        .get(1)?
        .as_str()
        .parse::<u32>()
        .ok()?;
    let h = IMG_HEIGHT_ATTR
        .captures(rest)?
        .get(1)?
        .as_str()
        .parse::<u32>()
        .ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

/// Fetch the first 2 KiB of an image and decode the header for dimensions.
/// Uses `Range: bytes=0-2047` to avoid pulling the full image. Returns
/// `None` when the server doesn't support range requests, the dimensions
/// live past the first 2 KiB (rare for web formats), or the response is
/// not a recognizable image. Errors propagate as `Err`.
pub(crate) async fn partial_fetch_dimensions(
    http: &reqwest::Client,
    src: &str,
    ssrf_level: SsrfLevel,
) -> Result<Option<(u32, u32)>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    ssrf_preflight(&url, src, ssrf_level).await?;
    let resp = SSRF_LEVEL
        .scope(
            ssrf_level,
            http.get(url.clone())
                .header(reqwest::header::RANGE, "bytes=0-2047")
                .send(),
        )
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
    Ok(crate::extractor::image_dims::peek_dimensions(&bytes[..]))
}

/// Fetch a `Content-Length` header without downloading the body. Returns
/// `None` when the server doesn't expose `Content-Length` (e.g. chunked
/// transfer). HEAD request; falls back to range-GET if HEAD is rejected.
pub(crate) async fn fetch_content_length(
    http: &reqwest::Client,
    src: &str,
    ssrf_level: SsrfLevel,
) -> Result<Option<u64>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    ssrf_preflight(&url, src, ssrf_level).await?;
    let resp = SSRF_LEVEL
        .scope(ssrf_level, http.head(url.clone()).send())
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            // reqwest's content_length() returns 0 for HEAD responses (no body).
            // Read the raw Content-Length header instead so the size gate works
            // correctly against the declared resource size.
            let from_header = r
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            Ok(from_header)
        }
        _ => {
            let r = SSRF_LEVEL
                .scope(
                    ssrf_level,
                    http.get(url)
                        .header(reqwest::header::RANGE, "bytes=0-0")
                        .send(),
                )
                .await
                .map_err(|source| ExtractorError::ImageDownload {
                    url: src.to_string(),
                    source,
                })?;
            Ok(r.content_length())
        }
    }
}

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
    Caption {
        dims: Option<(u32, u32)>,
    },
    Skip {
        reason: SkipReason,
        dims: Option<(u32, u32)>,
        bytes: Option<u64>,
    },
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
    ssrf_level: SsrfLevel,
) -> CaptionDecision {
    // Step 1: dimensions.
    let dims = match html_attr_dims(rest) {
        Some(d) => Some(d),
        None => match partial_fetch_dimensions(http, src, ssrf_level).await {
            Ok(Some(d)) => Some(d),
            Ok(None) => None,
            Err(_) => None,
        },
    };
    if let Some((w, h)) = dims
        && (w < filters.min_width || h < filters.min_height)
    {
        return CaptionDecision::Skip {
            reason: SkipReason::BelowMinDimensions,
            dims: Some((w, h)),
            bytes: None,
        };
    }

    // Step 2: size.
    let bytes: Option<u64> = fetch_content_length(http, src, ssrf_level)
        .await
        .unwrap_or_default();
    if let Some(n) = bytes
        && n > filters.max_bytes
    {
        return CaptionDecision::Skip {
            reason: SkipReason::AboveMaxBytes,
            dims,
            bytes: Some(n),
        };
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

fn sniff_ext(resp: &reqwest::Response, url: &Url) -> String {
    if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE)
        && let Ok(s) = ct.to_str()
    {
        let mime = s.split(';').next().unwrap_or("").trim();
        if let Some(ext) = mime_guess::get_mime_extensions_str(mime).and_then(|exts| exts.first()) {
            return (*ext).to_string();
        }
    }
    if let Some(path_seg) = url.path_segments().and_then(|mut s| s.next_back())
        && let Some((_, ext)) = path_seg.rsplit_once('.')
        && !ext.is_empty()
        && ext.len() <= 5
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return ext.to_lowercase();
    }
    "bin".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::OUTPUT_DIR_TEST_MUTEX as TEST_MUTEX;

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
        assert_eq!(
            r.images_processed[0].reason.as_deref(),
            Some("download_error")
        );
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
        assert_eq!(
            r.images_processed[0].reason.as_deref(),
            Some("captioner_error")
        );
    }

    fn paths() -> OutputPaths {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        // SAFETY: serialized by TEST_MUTEX in each test
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
        OutputPaths::resolve(None).unwrap()
    }

    fn client() -> reqwest::Client {
        crate::fetcher::client::install_ring_provider();
        reqwest::Client::new()
    }

    fn setup_paths() -> OutputPaths {
        let g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let p = paths();
        drop(g);
        p
    }

    #[tokio::test]
    async fn keep_passes_through_unchanged() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(
            md,
            &ImagesMode::Keep,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 1);
        assert_eq!(r.images_downloaded, 0);
    }

    #[tokio::test]
    async fn alt_text_only_substitutes_alt() {
        let p = setup_paths();
        let md = "Look ![hello](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(
            md,
            &ImagesMode::AltTextOnly,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap();
        assert_eq!(r.markdown, "Look hello at this.");
    }

    #[tokio::test]
    async fn alt_text_only_with_empty_alt_removes_image() {
        let p = setup_paths();
        let md = "Look ![](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(
            md,
            &ImagesMode::AltTextOnly,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn drop_removes_image_syntax_entirely() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(
            md,
            &ImagesMode::Drop,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn no_images_in_input_yields_empty_counters() {
        let p = setup_paths();
        let md = "No images here.";
        let f = ImageCaptionFilters::default();
        let r = apply(
            md,
            &ImagesMode::Download,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 0);
    }

    #[tokio::test]
    async fn caption_mode_without_registry_errors() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let err = apply(
            md,
            &ImagesMode::Caption,
            &p,
            &client(),
            None,
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExtractorError::CaptionerNotConfigured));
    }

    #[tokio::test]
    async fn caption_mode_with_empty_registry_errors() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let reg = CaptionerRegistry::empty();
        let err = apply(
            md,
            &ImagesMode::Caption,
            &p,
            &client(),
            Some(&reg),
            &f,
            None,
            SsrfLevel::Strict,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExtractorError::CaptionerNotConfigured));
    }

    #[test]
    fn html_attr_dims_extracts_width_height() {
        assert_eq!(
            html_attr_dims(r#" width="200" height="150""#),
            Some((200, 150))
        );
        assert_eq!(html_attr_dims(r#" width=200 height=150"#), Some((200, 150)));
        assert_eq!(html_attr_dims(r#" width="200""#), None);
        assert_eq!(html_attr_dims(""), None);
        assert_eq!(html_attr_dims(r#" width="0" height="100""#), None);
    }

    #[tokio::test]
    async fn classify_skips_below_min_dimensions_via_html_attrs() {
        crate::fetcher::client::install_ring_provider();
        let client = reqwest::Client::new();
        let f = ImageCaptionFilters {
            min_width: 200,
            min_height: 200,
            ..Default::default()
        };
        let d = classify(
            "https://example.com/icon.svg",
            r#" width="24" height="24""#,
            &client,
            0,
            &f,
            SsrfLevel::Strict,
        )
        .await;
        assert!(matches!(
            d,
            CaptionDecision::Skip {
                reason: SkipReason::BelowMinDimensions,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn classify_skips_per_page_budget() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        // Need a real URL that passes dimension+size checks; provide a small mocked image with no Content-Length headache.
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(&[0u8; 100][..]))
            .mount(&server)
            .await;
        crate::fetcher::client::install_ring_provider();
        let client = reqwest::Client::new();
        let f = ImageCaptionFilters {
            max_per_page: 3,
            ..Default::default()
        };
        let url = format!("{}/photo.png", server.uri());
        // captioned_so_far == max_per_page → skip
        let d = classify(
            &url,
            r#" width="500" height="500""#,
            &client,
            3,
            &f,
            SsrfLevel::Loopback,
        )
        .await;
        assert!(matches!(
            d,
            CaptionDecision::Skip {
                reason: SkipReason::PerPageBudget,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn partial_fetch_dimensions_reads_png_header() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // 1x1 transparent PNG bytes.
        let png: [u8; 67] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(&png[..]))
            .mount(&server)
            .await;
        crate::fetcher::client::install_ring_provider();
        let client = reqwest::Client::new();
        let url = format!("{}/img.png", server.uri());
        let dims = partial_fetch_dimensions(&client, &url, SsrfLevel::Loopback)
            .await
            .unwrap();
        assert_eq!(dims, Some((1, 1)));
    }

    /// The image-fetch helpers must enforce the active `SsrfLevel` before
    /// connecting. `localhost` resolves to loopback, which `Strict` rejects
    /// — the pre-flight (`validate_url_for_level`) catches it and no dial is
    /// attempted. Closes the gap where these helpers issued un-policed
    /// requests (see `docs/security.md` §"DNS rebinding").
    #[tokio::test]
    async fn download_one_blocks_loopback_under_strict() {
        use crate::fetcher::ssrf::SsrfError;

        let p = setup_paths();
        let err = download_one(&client(), "http://localhost:9/x.png", &p, SsrfLevel::Strict)
            .await
            .expect_err("strict must reject the loopback target");
        assert!(
            matches!(
                err,
                ExtractorError::ImageSsrf {
                    source: SsrfError::Address { .. },
                    ..
                }
            ),
            "expected ImageSsrf(Address), got: {err:?}",
        );
    }
}

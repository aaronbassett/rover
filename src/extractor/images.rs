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

pub async fn apply(
    markdown: &str,
    mode: &ImagesMode,
    output_paths: &OutputPaths,
    http: &reqwest::Client,
    captioners: Option<&CaptionerRegistry>,
    filters: &ImageCaptionFilters,
    db: Option<&Db>,
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
            reason: e.to_string(),
        })?)
    } else {
        None
    };
    let mut captioned_so_far = 0usize;

    // Two-step: enumerate matches, then transform. Async download requires
    // we can't use `replace_all` directly.
    let matches: Vec<(usize, usize, String, String, String)> = INLINE_IMG
        .captures_iter(markdown)
        .map(|c| {
            let m = c.get(0).unwrap();
            (
                m.start(),
                m.end(),
                c["alt"].to_string(),
                c["src"].to_string(),
                c["rest"].to_string(),
            )
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
            ImagesMode::Download => match download_one(http, &src, output_paths).await {
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
            },
            ImagesMode::Caption => {
                // SAFETY: resolved above when mode == Caption.
                let cap = captioner
                    .as_ref()
                    .expect("captioner resolved when mode == Caption");
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
                )
                .await
            }
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
) -> String {
    let decision = classify(src, rest, http, *captioned_so_far, filters).await;
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
            let bytes = match download_image_bytes(http, src).await {
                Ok(b) => b,
                Err(e) => {
                    *images_failed += 1;
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

async fn download_image_bytes(
    http: &reqwest::Client,
    src: &str,
) -> Result<Vec<u8>, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    let resp = http
        .get(url.clone())
        .send()
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
) -> Result<String, ExtractorError> {
    let url = Url::parse(src).map_err(|source| ExtractorError::ImageUrlInvalid {
        url: src.to_string(),
        source,
    })?;
    let resp =
        http.get(url.clone())
            .send()
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
    let bytes: Option<u64> = fetch_content_length(http, src).await.unwrap_or_default();
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

fn sniff_ext(resp: &reqwest::Response, url: &Url) -> String {
    if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(s) = ct.to_str() {
            let mime = s.split(';').next().unwrap_or("").trim();
            if let Some(ext) =
                mime_guess::get_mime_extensions_str(mime).and_then(|exts| exts.first())
            {
                return (*ext).to_string();
            }
        }
    }
    if let Some(path_seg) = url.path_segments().and_then(|mut s| s.next_back()) {
        if let Some((_, ext)) = path_seg.rsplit_once('.') {
            if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return ext.to_lowercase();
            }
        }
    }
    "bin".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn paths() -> OutputPaths {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        // SAFETY: serialized by TEST_MUTEX in each test
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
        OutputPaths::resolve(None).unwrap()
    }

    fn client() -> reqwest::Client {
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
        let r = apply(md, &ImagesMode::Keep, &p, &client(), None, &f, None)
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
        let r = apply(md, &ImagesMode::AltTextOnly, &p, &client(), None, &f, None)
            .await
            .unwrap();
        assert_eq!(r.markdown, "Look hello at this.");
    }

    #[tokio::test]
    async fn alt_text_only_with_empty_alt_removes_image() {
        let p = setup_paths();
        let md = "Look ![](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(md, &ImagesMode::AltTextOnly, &p, &client(), None, &f, None)
            .await
            .unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn drop_removes_image_syntax_entirely() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let f = ImageCaptionFilters::default();
        let r = apply(md, &ImagesMode::Drop, &p, &client(), None, &f, None)
            .await
            .unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn no_images_in_input_yields_empty_counters() {
        let p = setup_paths();
        let md = "No images here.";
        let f = ImageCaptionFilters::default();
        let r = apply(md, &ImagesMode::Download, &p, &client(), None, &f, None)
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
        let err = apply(md, &ImagesMode::Caption, &p, &client(), None, &f, None)
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
        let client = reqwest::Client::new();
        let f = ImageCaptionFilters {
            max_per_page: 3,
            ..Default::default()
        };
        let url = format!("{}/photo.png", server.uri());
        // captioned_so_far == max_per_page → skip
        let d = classify(&url, r#" width="500" height="500""#, &client, 3, &f).await;
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
        let client = reqwest::Client::new();
        let url = format!("{}/img.png", server.uri());
        let dims = partial_fetch_dimensions(&client, &url).await.unwrap();
        assert_eq!(dims, Some((1, 1)));
    }
}

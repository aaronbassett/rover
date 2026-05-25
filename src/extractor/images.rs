//! Image transformation modes.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::extractor::options::ImagesMode;
use crate::extractor::output::OutputPaths;
use crate::extractor::pipeline::ExtractorError;

static INLINE_IMG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!\[(?P<alt>[^\]]*)\]\((?P<src>[^)\s]+)(?P<rest>[^)]*)\)").unwrap()
});

#[derive(Debug, Default, Clone)]
pub struct ImagesApplied {
    pub markdown: String,
    pub images_seen: usize,
    pub images_downloaded: usize,
    pub images_failed: usize,
}

pub async fn apply(
    markdown: &str,
    mode: &ImagesMode,
    output_paths: &OutputPaths,
    http: &reqwest::Client,
) -> Result<ImagesApplied, ExtractorError> {
    let mut images_seen = 0usize;
    let mut images_downloaded = 0usize;
    let mut images_failed = 0usize;

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
            // Caption mode wiring added in Task 10; fall back to alt-text for now.
            ImagesMode::Caption => alt.clone(),
        };
        out.push_str(&replacement);
    }
    out.push_str(&markdown[cursor..]);

    Ok(ImagesApplied {
        markdown: out,
        images_seen,
        images_downloaded,
        images_failed,
    })
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

#[allow(dead_code)]
static IMG_WIDTH_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bwidth\s*=\s*"?(\d+)"?"#).unwrap());
#[allow(dead_code)]
static IMG_HEIGHT_ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bheight\s*=\s*"?(\d+)"?"#).unwrap());

/// Extract `<img width=… height=…>` from the markdown image's `rest`
/// capture (the tail between the URL and the closing paren). Returns
/// `(width, height)` when both are present and parse as positive integers.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        let r = apply(md, &ImagesMode::Keep, &p, &client()).await.unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 1);
        assert_eq!(r.images_downloaded, 0);
    }

    #[tokio::test]
    async fn alt_text_only_substitutes_alt() {
        let p = setup_paths();
        let md = "Look ![hello](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::AltTextOnly, &p, &client())
            .await
            .unwrap();
        assert_eq!(r.markdown, "Look hello at this.");
    }

    #[tokio::test]
    async fn alt_text_only_with_empty_alt_removes_image() {
        let p = setup_paths();
        let md = "Look ![](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::AltTextOnly, &p, &client())
            .await
            .unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn drop_removes_image_syntax_entirely() {
        let p = setup_paths();
        let md = "Look ![alt](https://x/img.png) at this.";
        let r = apply(md, &ImagesMode::Drop, &p, &client()).await.unwrap();
        assert_eq!(r.markdown, "Look  at this.");
    }

    #[tokio::test]
    async fn no_images_in_input_yields_empty_counters() {
        let p = setup_paths();
        let md = "No images here.";
        let r = apply(md, &ImagesMode::Download, &p, &client())
            .await
            .unwrap();
        assert_eq!(r.markdown, md);
        assert_eq!(r.images_seen, 0);
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

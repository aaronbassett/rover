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
}

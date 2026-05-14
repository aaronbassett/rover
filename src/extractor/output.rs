//! Output paths for table CSVs and downloaded images.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use url::Url;

use crate::extractor::pipeline::ExtractorError;

#[derive(Debug, Clone)]
pub struct OutputPaths {
    root: PathBuf,
}

impl OutputPaths {
    /// Resolve the output root. Precedence: `ROVER_OUTPUT_DIR` env var,
    /// then the supplied path (if `Some`), then `dirs::data_local_dir()
    /// .join("rover").join("output")`. Creates the root if missing.
    pub fn resolve(configured: Option<&Path>) -> Result<Self, ExtractorError> {
        let root: PathBuf = if let Ok(env_dir) = std::env::var("ROVER_OUTPUT_DIR") {
            PathBuf::from(env_dir)
        } else if let Some(p) = configured {
            p.to_path_buf()
        } else {
            dirs::data_local_dir()
                .ok_or_else(|| ExtractorError::Output {
                    path: "<data_local_dir>".to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no platform data dir",
                    ),
                })?
                .join("rover")
                .join("output")
        };
        std::fs::create_dir_all(&root).map_err(|source| ExtractorError::Output {
            path: root.display().to_string(),
            source,
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn table_path(&self, url: &Url, table_ordinal: usize) -> PathBuf {
        let host = url.host_str().unwrap_or("unknown");
        let key = format!("{}#{}", url.as_str(), table_ordinal);
        self.root
            .join("tables")
            .join(host)
            .join(format!("{}.csv", sha8(&key)))
    }

    pub fn image_path(&self, url: &Url, ext: &str) -> PathBuf {
        let host = url.host_str().unwrap_or("unknown");
        let ext = ext.trim_start_matches('.');
        let ext = if ext.is_empty() { "bin" } else { ext };
        self.root
            .join("images")
            .join(host)
            .join(format!("{}.{ext}", sha8(url.as_str())))
    }
}

pub fn sha8(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    out.iter()
        .take(4)
        .fold(String::with_capacity(8), |mut s, b| {
            s.push_str(&format!("{b:02x}"));
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes env-var-touching tests so they don't trample each other
    /// when cargo runs tests in parallel within this module.
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn url() -> Url {
        Url::parse("https://example.com/article").unwrap()
    }

    #[test]
    fn sha8_is_deterministic_and_eight_chars() {
        let a = sha8("https://example.com/x");
        let b = sha8("https://example.com/x");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn table_path_includes_ordinal() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: env access is serialized by TEST_MUTEX above.
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let paths = OutputPaths::resolve(None).unwrap();
        let p0 = paths.table_path(&url(), 0);
        let p1 = paths.table_path(&url(), 1);
        assert_ne!(p0, p1);
        assert!(p0.to_string_lossy().ends_with(".csv"));
        assert!(p0.to_string_lossy().contains("example.com"));
        unsafe { std::env::remove_var("ROVER_OUTPUT_DIR") };
    }

    #[test]
    fn image_path_uses_sha8_of_url_and_ext() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let paths = OutputPaths::resolve(None).unwrap();
        let p = paths.image_path(&Url::parse("https://x/img.png").unwrap(), "png");
        assert!(p.to_string_lossy().ends_with(".png"));
        assert!(p.to_string_lossy().contains(&sha8("https://x/img.png")));
        unsafe { std::env::remove_var("ROVER_OUTPUT_DIR") };
    }

    #[test]
    fn resolve_honors_env_then_config_then_default() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
        let p = OutputPaths::resolve(Some(Path::new("/ignored"))).unwrap();
        assert_eq!(p.root, tmp.path());
        unsafe { std::env::remove_var("ROVER_OUTPUT_DIR") };

        let tmp2 = tempfile::tempdir().unwrap();
        let p2 = OutputPaths::resolve(Some(tmp2.path())).unwrap();
        assert_eq!(p2.root, tmp2.path());
    }
}

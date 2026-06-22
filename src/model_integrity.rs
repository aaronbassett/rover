//! Integrity verification for locally-cached HuggingFace model files (M9
//! hardening; red-team finding #6).
//!
//! Compile-gated on `feature = "local-inference"`.
//!
//! After a model is downloaded, Rover records the SHA256 of every file in the
//! resolved snapshot to a sidecar manifest (`.rover-integrity.toml`) inside the
//! snapshot directory. Before a cached model is loaded, every recorded file is
//! re-hashed and compared; a mismatch aborts the load with a typed error so a
//! tampered weight file can never be silently fed to the inference engine.
//!
//! ## Trust-on-first-bootstrap
//!
//! A cache populated before this feature existed (or one populated by
//! `mistralrs`' own internal downloader, which Rover does not intercept) has no
//! manifest. On first encounter Rover hashes the files *in place* and writes
//! the manifest, emitting a `warn`. This is a deliberate trust-on-first-use
//! moment: Rover cannot retroactively know whether those bytes were already
//! tampered with, only that they will not change afterwards.
//!
//! ## Escape hatch
//!
//! Setting `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK=1` (or passing the
//! `--unsafe-disable-model-integrity-check` global flag, which sets the same
//! env var) skips verification entirely. The name is intentionally long and
//! ugly — it is documentation.
//!
//! ## Structure
//!
//! The public functions resolve the cache root from the environment
//! (`$HF_HOME/hub` or `~/.cache/huggingface/hub`) and delegate to private
//! `*_at(root, …)` helpers. Tests drive the `*_at` helpers against a temp
//! directory so they never mutate the process-global `HF_HOME`.

#![cfg(any(feature = "local-inference", feature = "injection-model"))]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Sidecar manifest filename, written inside a model's snapshot directory.
pub const MANIFEST_NAME: &str = ".rover-integrity.toml";

const SCHEMA_VERSION: u32 = 1;

/// Environment variable that disables integrity verification when truthy.
pub const DISABLE_ENV: &str = "ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK";

#[derive(Debug, Error)]
pub enum IntegrityError {
    #[error("model file {file} failed integrity check (expected {expected}, got {actual})")]
    ModelIntegrityFailure {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("no snapshot found for {repo} under {dir}")]
    NoSnapshot { repo: String, dir: String },

    #[error("could not read manifest at {path}: {source}")]
    ManifestRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse manifest at {path}: {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not write manifest at {path}: {source}")]
    ManifestWrite {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// The sidecar manifest, serialised as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    repo: String,
    revision: String,
    files: BTreeMap<String, String>,
}

/// Per-file verdict for a verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Mismatch { expected: String, actual: String },
    Missing { expected: String },
}

/// Outcome of verifying one repo, suitable for reporting (doctor /
/// `rover model verify`). Distinguishes logical states from I/O failures,
/// which surface as `Err(IntegrityError)`.
#[derive(Debug, Clone)]
pub enum RepoStatus {
    /// Verified against an existing manifest; every file matched.
    Ok { revision: String, files: usize },
    /// At least one file did not match (or went missing).
    Mismatch {
        revision: String,
        files: Vec<(String, FileStatus)>,
    },
    /// No manifest yet — a bootstrap will write one on next load.
    NoManifest,
    /// The model is not present in the cache.
    NotCached,
}

/// True when integrity verification is disabled via the env var.
pub fn check_disabled() -> bool {
    std::env::var(DISABLE_ENV)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Root of the HuggingFace hub cache (`$HF_HOME/hub` or `~/.cache/huggingface/hub`).
pub fn hf_cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/huggingface/hub");
    }
    PathBuf::from(".cache/huggingface/hub")
}

/// `<root>/models--owner--repo` for a repo id like `owner/repo`.
pub fn model_dir(repo_id: &str) -> PathBuf {
    model_dir_at(&hf_cache_root(), repo_id)
}

fn model_dir_at(root: &Path, repo_id: &str) -> PathBuf {
    root.join(format!("models--{}", repo_id.replace('/', "--")))
}

/// Locate the resolved snapshot directory and its revision for a model.
///
/// Prefers the commit recorded in `refs/main`; falls back to the sole snapshot
/// directory when there is exactly one.
fn resolve_snapshot_at(root: &Path, repo_id: &str) -> Result<(PathBuf, String), IntegrityError> {
    let dir = model_dir_at(root, repo_id);
    let snapshots = dir.join("snapshots");

    // Preferred: refs/main names the commit sha.
    let refs_main = dir.join("refs").join("main");
    if let Ok(rev) = std::fs::read_to_string(&refs_main) {
        let rev = rev.trim().to_string();
        let snap = snapshots.join(&rev);
        if snap.is_dir() {
            return Ok((snap, rev));
        }
    }

    // Fallback: a single snapshot directory.
    if let Ok(entries) = std::fs::read_dir(&snapshots) {
        let dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        if dirs.len() == 1 {
            let rev = dirs[0]
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            return Ok((dirs[0].clone(), rev));
        }
    }

    Err(IntegrityError::NoSnapshot {
        repo: repo_id.to_string(),
        dir: dir.display().to_string(),
    })
}

/// SHA256 a file, returning `sha256:<lowercase-hex>`. Opening the path follows
/// the HF cache's `snapshots/<rev>/file -> ../../blobs/<hash>` symlinks.
fn sha256_file(path: &Path) -> Result<String, IntegrityError> {
    let mut f = std::fs::File::open(path).map_err(|source| IntegrityError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|source| IntegrityError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        write!(hex, "{b:02x}").expect("write to String never fails");
    }
    Ok(format!("sha256:{hex}"))
}

/// Hash every file under `snapshot` (recursively), keyed by `/`-joined path
/// relative to the snapshot root. Skips the manifest itself.
fn hash_snapshot(snapshot: &Path) -> Result<BTreeMap<String, String>, IntegrityError> {
    let mut out = BTreeMap::new();
    let mut stack = vec![snapshot.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| IntegrityError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| IntegrityError::Io {
                path: dir.display().to_string(),
                source,
            })?;
            let path = entry.path();
            // `metadata` (not `symlink_metadata`) follows symlinks, so HF's
            // snapshot symlinks resolve to the real blob.
            let meta = std::fs::metadata(&path).map_err(|source| IntegrityError::Io {
                path: path.display().to_string(),
                source,
            })?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(snapshot)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if rel == MANIFEST_NAME {
                continue;
            }
            out.insert(rel, sha256_file(&path)?);
        }
    }
    Ok(out)
}

fn read_manifest(snapshot: &Path) -> Result<Option<Manifest>, IntegrityError> {
    let path = snapshot.join(MANIFEST_NAME);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let m = toml::from_str(&s).map_err(|source| IntegrityError::ManifestParse {
                path: path.display().to_string(),
                source,
            })?;
            Ok(Some(m))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(IntegrityError::ManifestRead {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn is_cached_at(root: &Path, repo_id: &str) -> bool {
    let dir = model_dir_at(root, repo_id);
    dir.exists()
        && dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

fn cached_repos_at(root: &Path) -> Result<Vec<String>, IntegrityError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(root).map_err(|source| IntegrityError::Io {
        path: root.display().to_string(),
        source,
    })?;
    let mut repos = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| IntegrityError::Io {
            path: root.display().to_string(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(repo) = name.strip_prefix("models--") {
            repos.push(repo.replacen("--", "/", 1));
        }
    }
    repos.sort();
    Ok(repos)
}

fn bootstrap_at(root: &Path, repo_id: &str) -> Result<RepoStatus, IntegrityError> {
    let (snapshot, revision) = resolve_snapshot_at(root, repo_id)?;
    let files = hash_snapshot(&snapshot)?;
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        repo: repo_id.to_string(),
        revision: revision.clone(),
        files,
    };
    let path = snapshot.join(MANIFEST_NAME);
    let toml = toml::to_string_pretty(&manifest).expect("manifest serialises");
    std::fs::write(&path, toml).map_err(|source| IntegrityError::ManifestWrite {
        path: path.display().to_string(),
        source,
    })?;
    tracing::warn!(
        target: "rover::model_integrity",
        repo = %repo_id,
        revision = %revision,
        files = manifest.files.len(),
        manifest = %path.display(),
        "trust-on-first-bootstrap: recorded model integrity manifest from existing files; \
         these bytes are now trusted and changes will be detected on subsequent loads"
    );
    Ok(RepoStatus::Ok {
        revision,
        files: manifest.files.len(),
    })
}

fn verify_repo_at(root: &Path, repo_id: &str) -> Result<RepoStatus, IntegrityError> {
    if !is_cached_at(root, repo_id) {
        return Ok(RepoStatus::NotCached);
    }
    let (snapshot, _) = resolve_snapshot_at(root, repo_id)?;
    let Some(manifest) = read_manifest(&snapshot)? else {
        return Ok(RepoStatus::NoManifest);
    };
    let mut failures: Vec<(String, FileStatus)> = Vec::new();
    for (file, expected) in &manifest.files {
        let path = snapshot.join(file);
        match std::fs::metadata(&path) {
            Ok(_) => {
                let actual = sha256_file(&path)?;
                if &actual != expected {
                    failures.push((
                        file.clone(),
                        FileStatus::Mismatch {
                            expected: expected.clone(),
                            actual,
                        },
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                failures.push((
                    file.clone(),
                    FileStatus::Missing {
                        expected: expected.clone(),
                    },
                ));
            }
            Err(source) => {
                return Err(IntegrityError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        }
    }
    if failures.is_empty() {
        Ok(RepoStatus::Ok {
            revision: manifest.revision,
            files: manifest.files.len(),
        })
    } else {
        Ok(RepoStatus::Mismatch {
            revision: manifest.revision,
            files: failures,
        })
    }
}

fn enforce_at(root: &Path, repo_id: &str) -> Result<(), IntegrityError> {
    match verify_repo_at(root, repo_id)? {
        RepoStatus::Ok { .. } | RepoStatus::NotCached => Ok(()),
        RepoStatus::NoManifest => {
            bootstrap_at(root, repo_id)?;
            Ok(())
        }
        RepoStatus::Mismatch { files, .. } => {
            let (file, status) = files.into_iter().next().expect("mismatch is non-empty");
            let (expected, actual) = match status {
                FileStatus::Mismatch { expected, actual } => (expected, actual),
                FileStatus::Missing { expected } => (expected, "missing".to_string()),
            };
            Err(IntegrityError::ModelIntegrityFailure {
                file,
                expected,
                actual,
            })
        }
    }
}

// ---- Public, env-resolving API used by the loaders, CLI, and doctor ----

/// Is the model present in the cache (snapshot directory with at least one
/// entry)? Mirrors the existing `hf_cache_has` helpers.
pub fn is_cached(repo_id: &str) -> bool {
    is_cached_at(&hf_cache_root(), repo_id)
}

/// Repo ids for every `models--owner--repo` directory in the cache. Returns an
/// empty list when the cache root does not exist.
pub fn cached_repos() -> Result<Vec<String>, IntegrityError> {
    cached_repos_at(&hf_cache_root())
}

/// Hash the model's current files in place and write the manifest, recording
/// the resolved revision. Emits a trust-on-first-bootstrap `warn`.
pub fn bootstrap(repo_id: &str) -> Result<RepoStatus, IntegrityError> {
    bootstrap_at(&hf_cache_root(), repo_id)
}

/// Verify a repo against its manifest, returning a reportable status. I/O
/// failures (not mismatches) surface as `Err`.
pub fn verify_repo(repo_id: &str) -> Result<RepoStatus, IntegrityError> {
    verify_repo_at(&hf_cache_root(), repo_id)
}

/// Gate a model load. Verifies an existing manifest (erroring on the first
/// mismatch) or bootstraps one if absent. A no-op when the check is disabled
/// or the model is not cached. Intended to run *before* the inference engine
/// reads any weights.
pub fn enforce(repo_id: &str) -> Result<(), IntegrityError> {
    if check_disabled() {
        return Ok(());
    }
    enforce_at(&hf_cache_root(), repo_id)
}

/// Record a manifest for a freshly downloaded model (trust-on-first-use).
/// Logs on failure rather than propagating — a missing manifest only weakens
/// future verification, it does not break the current load.
pub fn record_fresh_download(repo_id: &str) {
    if check_disabled() {
        return;
    }
    if let Err(e) = bootstrap(repo_id) {
        tracing::warn!(
            target: "rover::model_integrity",
            repo = %repo_id,
            error = %e,
            "could not record integrity manifest after download; \
             verification will bootstrap on next load"
        );
    }
}

/// Serialises tests that mutate the process-global `HF_HOME`. The unit tests
/// in this module avoid `HF_HOME` entirely (they drive the `*_at` helpers), but
/// integration-style tests elsewhere (the doctor check, the summarizer/vlm
/// cache helpers) must exercise the env-resolving public API; they share this
/// lock so they cannot stomp each other's `HF_HOME` when run in parallel.
#[cfg(all(test, feature = "local-inference"))]
pub(crate) static HF_HOME_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal HF-style cache under `root` (the `hub` dir) and return
    /// the snapshot path. No environment is touched, so these tests are safe
    /// to run in parallel.
    fn make_cache(root: &Path, repo_id: &str, rev: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let model = root.join(format!("models--{}", repo_id.replace('/', "--")));
        std::fs::create_dir_all(model.join("refs")).unwrap();
        std::fs::write(model.join("refs").join("main"), rev).unwrap();
        let snap = model.join("snapshots").join(rev);
        std::fs::create_dir_all(&snap).unwrap();
        for (name, bytes) in files {
            std::fs::write(snap.join(name), bytes).unwrap();
        }
        snap
    }

    #[test]
    fn bootstrap_then_verify_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = "Acme/tiny";
        let snap = make_cache(
            root,
            repo,
            "abc123",
            &[("config.json", b"{}"), ("model.safetensors", b"weights")],
        );

        let status = bootstrap_at(root, repo).unwrap();
        assert!(matches!(status, RepoStatus::Ok { files: 2, .. }));
        assert!(snap.join(MANIFEST_NAME).exists());

        assert!(matches!(
            verify_repo_at(root, repo).unwrap(),
            RepoStatus::Ok { .. }
        ));
        enforce_at(root, repo).unwrap();
    }

    #[test]
    fn tamper_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = "Acme/tiny";
        let snap = make_cache(
            root,
            repo,
            "rev1",
            &[("model.safetensors", b"original-weights")],
        );
        bootstrap_at(root, repo).unwrap();

        std::fs::write(snap.join("model.safetensors"), b"backdoored").unwrap();

        match verify_repo_at(root, repo).unwrap() {
            RepoStatus::Mismatch { files, .. } => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].0, "model.safetensors");
                assert!(matches!(files[0].1, FileStatus::Mismatch { .. }));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }

        let err = enforce_at(root, repo).unwrap_err();
        assert!(matches!(
            err,
            IntegrityError::ModelIntegrityFailure { ref file, .. } if file == "model.safetensors"
        ));
    }

    #[test]
    fn missing_file_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = "Acme/tiny";
        let snap = make_cache(root, repo, "rev1", &[("a.json", b"1"), ("b.json", b"2")]);
        bootstrap_at(root, repo).unwrap();
        std::fs::remove_file(snap.join("b.json")).unwrap();
        let err = enforce_at(root, repo).unwrap_err();
        assert!(matches!(
            err,
            IntegrityError::ModelIntegrityFailure { ref actual, .. } if actual == "missing"
        ));
    }

    #[test]
    fn no_manifest_bootstraps_on_enforce() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = "Acme/tiny";
        let snap = make_cache(root, repo, "rev1", &[("config.json", b"{}")]);
        assert!(matches!(
            verify_repo_at(root, repo).unwrap(),
            RepoStatus::NoManifest
        ));
        enforce_at(root, repo).unwrap();
        assert!(snap.join(MANIFEST_NAME).exists());
    }

    #[test]
    fn not_cached_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(matches!(
            verify_repo_at(root, "Nope/missing").unwrap(),
            RepoStatus::NotCached
        ));
        enforce_at(root, "Nope/missing").unwrap();
    }

    #[test]
    fn cached_repos_lists_models() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_cache(root, "Acme/one", "r", &[("a", b"1")]);
        make_cache(root, "Beta/two", "r", &[("a", b"1")]);
        let repos = cached_repos_at(root).unwrap();
        assert_eq!(repos, vec!["Acme/one".to_string(), "Beta/two".to_string()]);
    }

    #[test]
    fn revision_recorded_in_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = "Acme/tiny";
        let snap = make_cache(root, repo, "deadbeefcafe", &[("config.json", b"{}")]);
        bootstrap_at(root, repo).unwrap();
        let body = std::fs::read_to_string(snap.join(MANIFEST_NAME)).unwrap();
        assert!(body.contains("revision = \"deadbeefcafe\""));
        assert!(body.contains("schema_version = 1"));
    }

    #[test]
    fn disable_env_truthiness() {
        for (val, want) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("Yes", true),
            ("on", true),
            ("0", false),
            ("false", false),
            ("", false),
        ] {
            // SAFETY: serialised within this single test; restored each iter.
            unsafe { std::env::set_var(DISABLE_ENV, val) };
            assert_eq!(check_disabled(), want, "value {val:?}");
        }
        unsafe { std::env::remove_var(DISABLE_ENV) };
    }
}

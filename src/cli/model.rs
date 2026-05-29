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
    /// Verify cached model files against their integrity manifest.
    Verify {
        /// Repo id to verify. Omit to verify every cached model.
        repo_id: Option<String>,
    },
}

pub async fn run(cmd: ModelCmd) -> anyhow::Result<()> {
    match cmd {
        ModelCmd::Download { repo_id } => download(&repo_id).await,
        ModelCmd::List => list().await,
        ModelCmd::Remove { repo_id } => remove(&repo_id).await,
        ModelCmd::Verify { repo_id } => verify(repo_id.as_deref()).await,
    }
}

async fn download(repo_id: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use hf_hub::api::tokio::ApiBuilder;

    eprintln!("downloading {repo_id} from HuggingFace");

    // `with_progress(true)` makes hf-hub render a per-file indicatif progress
    // bar (bytes downloaded / total, labelled with the filename) during each
    // `get()`. indicatif draws to stderr and degrades gracefully when stderr
    // is not a TTY, so piped/redirected output stays clean.
    let api = ApiBuilder::new()
        .with_progress(true)
        .build()
        .context("building hf-hub api client")?;
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
            // The progress bar already reported this file; just count it.
            Ok(_) => downloaded += 1,
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
                Ok(_) => downloaded += 1,
                Err(e) => eprintln!("  {shard:<36} FAILED: {e}"),
            }
        }
    }

    if downloaded == 0 {
        anyhow::bail!("no files were downloaded for {repo_id}; check the repo id");
    }
    let cache_dir = hf_cache_root().join(format!("models--{}", repo_id.replace('/', "--")));
    eprintln!("cached at {}", cache_dir.display());

    // Record the integrity manifest so later loads can detect tampering. This
    // resolves and records the downloaded revision (the snapshot commit sha).
    match crate::model_integrity::bootstrap(repo_id) {
        Ok(crate::model_integrity::RepoStatus::Ok { revision, files }) => {
            eprintln!("recorded integrity manifest: {files} files at revision {revision}");
        }
        Ok(_) => {}
        Err(e) => eprintln!("warning: could not record integrity manifest: {e}"),
    }
    Ok(())
}

/// Verify cached model files against their `.rover-integrity.toml` manifests.
/// With a repo id, verifies that one model; without, verifies every cached
/// model. Exits non-zero (via `bail`) if any file fails.
async fn verify(repo_id: Option<&str>) -> anyhow::Result<()> {
    use crate::model_integrity::{FileStatus, RepoStatus, verify_repo};

    let repos: Vec<String> = match repo_id {
        Some(r) => vec![r.to_string()],
        None => crate::model_integrity::cached_repos()?,
    };
    if repos.is_empty() {
        eprintln!("(no cached models to verify)");
        return Ok(());
    }

    let mut any_failed = false;
    for repo in &repos {
        match verify_repo(repo)? {
            RepoStatus::Ok { revision, files } => {
                eprintln!("OK    {repo}  ({files} files, revision {revision})");
            }
            RepoStatus::NoManifest => {
                eprintln!("?     {repo}  (no integrity manifest; will bootstrap on next load)");
            }
            RepoStatus::NotCached => {
                eprintln!("?     {repo}  (not cached)");
            }
            RepoStatus::Mismatch { revision, files } => {
                any_failed = true;
                eprintln!("FAIL  {repo}  (revision {revision})");
                for (file, status) in files {
                    match status {
                        FileStatus::Mismatch { expected, actual } => {
                            eprintln!(
                                "        {file}: modified (expected {expected}, got {actual})"
                            );
                        }
                        FileStatus::Missing { expected } => {
                            eprintln!("        {file}: missing (expected {expected})");
                        }
                    }
                }
            }
        }
    }
    if any_failed {
        anyhow::bail!("model integrity verification failed");
    }
    Ok(())
}

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

fn hf_cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/huggingface/hub");
    }
    PathBuf::from(".cache/huggingface/hub")
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
    const UNITS: &[(&str, u64)] = &[
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
        ("B", 1),
    ];
    for (unit, mult) in UNITS {
        if n >= *mult {
            return format!("{:.1} {}", n as f64 / *mult as f64, unit);
        }
    }
    format!("{n} B")
}

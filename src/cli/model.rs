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
    use anyhow::Context as _;
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
    eprintln!("cached at {}", cache_dir.display());
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

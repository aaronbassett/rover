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

#[allow(dead_code)]
fn hf_cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/huggingface/hub");
    }
    PathBuf::from(".cache/huggingface/hub")
}

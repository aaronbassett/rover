//! Shared helpers for integration tests.
//!
//! In Cargo's integration-test model, each file under `tests/` is compiled as
//! its own crate. `tests/common/` is the conventional name for a module shared
//! between test crates via `mod common;` declarations at the top of each test
//! file.

#![allow(dead_code)]

use std::path::Path;

use rmcp::ServiceExt;
use rmcp::transport::child_process::TokioChildProcess;
use tokio::process::Command;

/// Copy the bundled fixture tokenizer.json into the per-test data dir so the
/// spawned `rover mcp` child can hit the on-disk short-circuit in
/// `tokenizer::download::ensure_on_disk` and skip the HuggingFace download.
pub fn seed_default_tokenizer(data_dir: &Path) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer/tiny.json");
    let dest_dir = data_dir.join("tokenizers").join("o200k");
    std::fs::create_dir_all(&dest_dir).unwrap();
    let dest = dest_dir.join("tokenizer.json");
    std::fs::copy(&fixture, &dest).unwrap();
}

pub fn bin_path() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("rover")
}

/// Spawn `rover mcp` as a child process and return a connected rmcp client.
/// The child reads `ROVER_DATA_DIR` (set per-test) and is configured via the
/// generated `rover.toml`, which sets `[ssrf] level = "loopback"` so wiremock
/// servers bound to 127.0.0.1 satisfy SSRF.
///
/// The same `rover.toml` disables `robots.respect`, since the wiremock
/// servers used by tests don't speak HTTPS and would otherwise produce
/// robots fetch failures → DisallowAll.
/// Build a minimal in-process `SummarizerService` for tests that construct
/// a `RoverHandler` directly. Uses a single offline extractive backend so
/// no network I/O happens.
pub async fn make_summarizer_service(
    db: &rover::storage::Db,
) -> std::sync::Arc<rover::summarizer::SummarizerService> {
    let mut map: std::collections::HashMap<
        String,
        std::sync::Arc<dyn rover::summarizer::backend::SummarizerBackend>,
    > = Default::default();
    map.insert(
        "default".into(),
        std::sync::Arc::new(rover::summarizer::extractive::ExtractiveBackend::new(
            "default",
            rover::tokenizer::Tokenizer::O200k,
        )),
    );
    let reg = std::sync::Arc::new(
        rover::summarizer::registry::SummarizerRegistry::__test_construct(
            map,
            "default".into(),
            Some("default".into()),
        ),
    );
    std::sync::Arc::new(rover::summarizer::SummarizerService::new(
        db.clone(),
        reg,
        true,
    ))
}

pub async fn spawn_client(data_dir: &Path) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let cfg_path = data_dir.join("rover.toml");
    if !cfg_path.exists() {
        std::fs::write(
            &cfg_path,
            "[robots]\nrespect = false\n\n[ssrf]\nlevel = \"loopback\"\n",
        )
        .unwrap();
    }
    let mut cmd = Command::new(bin_path());
    cmd.arg("--config").arg(&cfg_path).arg("mcp");
    cmd.env("ROVER_DATA_DIR", data_dir);
    cmd.env("RUST_LOG", "info,rover=debug");
    let proc = TokioChildProcess::new(cmd).expect("spawn rover mcp");
    ().serve(proc).await.expect("client handshake")
}

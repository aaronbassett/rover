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

/// Like [`spawn_client`], but writes the caller-supplied `config_toml` to
/// `rover.toml` first (overwriting any prior file). Use when a test needs
/// non-default config sections (e.g. `[debug] har_path`).
pub async fn spawn_client_with_config(
    data_dir: &Path,
    config_toml: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    std::fs::write(data_dir.join("rover.toml"), config_toml).unwrap();
    spawn_client(data_dir).await
}

/// Build an `HttpState` for router-level tests. `token` of `None` means no
/// bearer auth. Uses a real on-disk `Db` in `data_dir` and an offline
/// extractive summarizer so nothing touches the network.
pub async fn http_state(data_dir: &Path, token: Option<&str>) -> rover::mcp::http::HttpState {
    seed_default_tokenizer(data_dir);
    let db = rover::storage::Db::open(data_dir.join("rover.db"))
        .await
        .expect("open db");
    let config = std::sync::Arc::new(rover::config::Config::default());
    let summarizer = make_summarizer_service(&db).await;
    let client =
        rover::fetcher::client::build_http_client(&config.fetch.user_agent, config.fetch.timeout());
    let handler = rover::mcp::handler::RoverHandler::new(
        db.clone(),
        config.clone(),
        client,
        rover::fetcher::ssrf::SsrfLevel::Loopback,
        None,
        None,
        std::sync::Arc::new(rover::fetcher::concurrency::Pacer::new(&config.rate_limit)),
        summarizer,
        // `CaptionerRegistry` derives only `Clone` and has private fields —
        // there is no `Default`. `build()` is the real constructor; an empty
        // `[captioners]` table yields an empty registry.
        std::sync::Arc::new(rover::vlm::build(&config).unwrap()),
        std::sync::Arc::new(rover::guard::Guard::from_config(&config.prompt_injection).unwrap()),
        rover::mcp::TransportKind::Http,
        #[cfg(feature = "headless")]
        std::sync::Arc::new(tokio::sync::OnceCell::new()),
    );
    // A NON-loopback bind on purpose. `resolve_allowed_hosts` derives the
    // loopback allow-list from a loopback bind, and rmcp then rejects any
    // request whose `Host` header is missing — which is every request
    // `Request::builder()` produces. Passing 0.0.0.0 disables Host validation,
    // matching the container case these tests exist to cover. The derivation
    // itself is covered by the pure unit tests in `src/mcp/http.rs` and by the
    // real-socket test in Task 9.
    rover::mcp::http::HttpState::new(
        handler,
        db,
        token,
        &config.http,
        "0.0.0.0:0".parse().unwrap(),
    )
}

/// A minimal, well-formed MCP request with the headers rmcp requires.
/// `Accept` MUST list both `application/json` and `text/event-stream` or
/// rmcp answers `406` before anything under test is reached.
///
/// `method` is the HTTP method (e.g. `"POST"`); `auth` is an optional raw
/// `Authorization` header value (e.g. `"Bearer <token>"`).
pub fn mcp_request(
    method: &str,
    auth: Option<&str>,
    body: axum::body::Body,
) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method(method)
        .uri("/mcp")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::ACCEPT,
            "application/json, text/event-stream",
        );
    if let Some(a) = auth {
        b = b.header(axum::http::header::AUTHORIZATION, a);
    }
    b.body(body).unwrap()
}

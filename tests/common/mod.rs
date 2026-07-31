//! Shared helpers for integration tests.
//!
//! In Cargo's integration-test model, each file under `tests/` is compiled as
//! its own crate. `tests/common/` is the conventional name for a module shared
//! between test crates via `mod common;` declarations at the top of each test
//! file.

#![allow(dead_code)]

use std::net::SocketAddr;
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

/// Shared builder behind [`http_state_with_bind`] and [`http_state_with`].
/// Uses a real on-disk `Db` in `data_dir` and an offline extractive
/// summarizer so nothing touches the network, and installs the given
/// `http_cfg` on the handler's `Arc<Config>` — not just on `HttpState` —
/// since `RoverHandler::fetch_inner`'s server-path guard reads
/// `self.config.http.allow_server_paths` from that `Arc`, not from
/// `HttpState`.
async fn build_http_state(
    data_dir: &Path,
    token: Option<&str>,
    bind: SocketAddr,
    http_cfg: rover::config::HttpConfig,
) -> rover::mcp::http::HttpState {
    seed_default_tokenizer(data_dir);
    let db = rover::storage::Db::open(data_dir.join("rover.db"))
        .await
        .expect("open db");
    let config = std::sync::Arc::new(rover::config::Config {
        http: http_cfg,
        ..Default::default()
    });
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
    rover::mcp::http::HttpState::new(handler, db, token, &config.http, bind)
}

/// Build an `HttpState` for router-level tests, bound to a caller-chosen
/// address, with a default `HttpConfig`. `token` of `None` means no bearer
/// auth.
///
/// Most tests want [`http_state`] instead, which fixes `bind` to a
/// non-loopback placeholder that disables rmcp's `Host` allow-list
/// entirely. Call this directly when a test needs the allow-list itself
/// under test — e.g. a loopback bind, where `resolve_allowed_hosts` derives
/// rmcp's real loopback list and a disallowed `Host` header must 403.
pub async fn http_state_with_bind(
    data_dir: &Path,
    token: Option<&str>,
    bind: SocketAddr,
) -> rover::mcp::http::HttpState {
    build_http_state(data_dir, token, bind, rover::config::HttpConfig::default()).await
}

/// [`http_state`] with a caller-supplied `HttpConfig` — e.g. to set
/// `allow_server_paths = true` and verify a call that would otherwise be
/// refused now goes through. Binds to the same `0.0.0.0:0` placeholder as
/// [`http_state`], so Host validation stays disabled.
pub async fn http_state_with(
    data_dir: &Path,
    token: Option<&str>,
    http_cfg: rover::config::HttpConfig,
) -> rover::mcp::http::HttpState {
    build_http_state(data_dir, token, "0.0.0.0:0".parse().unwrap(), http_cfg).await
}

/// [`http_state_with_bind`] with a NON-loopback bind, on purpose.
/// `resolve_allowed_hosts` derives the loopback allow-list from a loopback
/// bind, and rmcp then rejects any request whose `Host` header isn't in that
/// list. Passing `0.0.0.0:0` disables Host validation entirely, so any
/// well-formed `Host` (see [`mcp_request`]) is accepted — matching the
/// container case most of these tests exist to cover. The allow-list
/// derivation itself is covered by the pure unit tests in `src/mcp/http.rs`,
/// the enforcement itself by `http_state_with_bind` callers that pass a
/// loopback address, and the real-socket case by Task 9.
pub async fn http_state(data_dir: &Path, token: Option<&str>) -> rover::mcp::http::HttpState {
    http_state_with(data_dir, token, rover::config::HttpConfig::default()).await
}

/// Install the ring crypto provider for this test process. Rover pins reqwest
/// with `rustls-no-provider`, so any in-test reqwest client panics without it.
/// `install_ring_provider` is already idempotent (its own `OnceLock`,
/// `src/fetcher/client.rs:18`), so this needs no extra guard.
pub fn init_crypto() {
    rover::fetcher::client::install_ring_provider();
}

/// Spawn `rover mcp --http --bind 127.0.0.1:0` and return the child plus the
/// resolved base URL. The port is read from the server's own startup log
/// rather than pre-binding a socket, which avoids a bind race.
pub async fn spawn_http_server(
    data_dir: &Path,
    config_toml: &str,
    token: Option<&str>,
) -> (tokio::process::Child, String) {
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    seed_default_tokenizer(data_dir);
    let cfg_path = data_dir.join("rover.toml");
    std::fs::write(&cfg_path, config_toml).unwrap();

    let mut cmd = Command::new(bin_path());
    cmd.arg("--config")
        .arg(&cfg_path)
        .arg("mcp")
        .arg("--http")
        .arg("--bind")
        .arg("127.0.0.1:0");
    cmd.env("ROVER_DATA_DIR", data_dir);
    cmd.env("RUST_LOG", "info,rover=debug");
    if let Some(t) = token {
        cmd.env("ROVER_HTTP_TOKEN", t);
    } else {
        cmd.env_remove("ROVER_HTTP_TOKEN");
    }
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().expect("spawn rover mcp --http");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut lines = BufReader::new(stderr).lines();

    // Parse `addr=127.0.0.1:PORT` out of the listening line.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let addr = loop {
        let line = tokio::time::timeout_at(deadline, lines.next_line())
            .await
            .expect("timed out waiting for the listening line")
            .expect("read stderr")
            .expect("server exited before logging its address");
        if let Some(rest) = line.split("addr=").nth(1) {
            let addr = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            if !addr.is_empty() {
                break addr;
            }
        }
    };

    // Keep draining stderr so the child never blocks on a full pipe.
    tokio::spawn(async move { while lines.next_line().await.ok().flatten().is_some() {} });

    (child, format!("http://{addr}"))
}

/// A minimal, well-formed MCP request with the headers rmcp requires.
///
/// `Accept` MUST list both `application/json` and `text/event-stream` or
/// rmcp answers `406` before anything under test is reached.
///
/// `Host` is required too: rmcp's `validate_dns_rebinding_headers` demands a
/// `Host` header (or an HTTP/2 `:authority`) on every request before it even
/// consults the allow-list — confirmed against
/// `rmcp-1.7.0/src/transport/streamable_http_server/tower.rs`'s
/// `parse_host_header`. In-process `oneshot` testing builds the `Request`
/// directly in Rust, skipping the HTTP/1.1 wire parsing that would normally
/// populate `Host` from a real client, so without this every request 400s
/// with `Bad Request: missing Host header` regardless of `allowed_hosts`.
/// `"localhost"` is in rmcp's default loopback allow-list, so this passes
/// both under `http_state` (validation disabled) and under a loopback
/// `http_state_with_bind` (validation enabled, `localhost` allowed).
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
        .header(axum::http::header::HOST, "localhost")
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

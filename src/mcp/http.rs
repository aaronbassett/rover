//! Streamable HTTP transport for `rover mcp`.
//!
//! The message-handling core is rmcp's `StreamableHttpService`, configured
//! stateless with JSON responses: Rover has no server-initiated messages, so
//! SSE framing buys nothing and every POST is self-contained — which is what
//! N agents sharing one instance want.
//!
//! This module owns only the outer framing: bearer auth, health probes, the
//! body limit, and Host-policy resolution.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tower_http::limit::RequestBodyLimitLayer;

use crate::config::{Config, HttpConfig};
use crate::fetcher::ssrf::SsrfLevel;
use crate::mcp::TransportKind;
use crate::mcp::handler::RoverHandler;
use crate::mcp::runtime::build_runtime;
use crate::storage::Db;

/// Cap on a single request body. MCP tool arguments are small except
/// `count_tokens`, which accepts inline text.
///
/// Uses tower-http's layer, NOT axum's `DefaultBodyLimit`: the latter only
/// applies to `FromRequest` impls that opt in, and `StreamableHttpService`
/// consumes the body itself via `body.collect()`, so `DefaultBodyLimit`
/// would be a silent no-op and an unbounded POST would be buffered in full.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Router state shared by every handler.
#[derive(Clone)]
pub struct HttpState {
    pub(crate) handler: RoverHandler,
    pub(crate) db: Db,
    /// SHA-256 of the configured bearer token, or `None` for no auth.
    pub(crate) token_digest: Option<[u8; 32]>,
    /// Resolved `Host` allow-list: `None` means validation is disabled.
    pub(crate) allowed_hosts: Option<Vec<String>>,
    pub(crate) allowed_origins: Vec<String>,
    /// Throttles the failed-auth rejection log line. Shared via `Arc` so
    /// every clone of `HttpState` (one per request, via `State` extraction)
    /// counts against the same window.
    pub(crate) rejection_throttle: Arc<RejectionThrottle>,
}

impl HttpState {
    #[must_use]
    pub fn new(
        handler: RoverHandler,
        db: Db,
        token: Option<&str>,
        cfg: &HttpConfig,
        bind: SocketAddr,
    ) -> Self {
        Self {
            handler,
            db,
            token_digest: token.filter(|t| !t.is_empty()).map(digest),
            allowed_hosts: resolve_allowed_hosts(&cfg.allowed_hosts, bind),
            allowed_origins: cfg.allowed_origins.clone(),
            rejection_throttle: Arc::new(RejectionThrottle::new()),
        }
    }
}

/// Throttles the failed-auth rejection log line to at most one line per
/// second, folding any rejections observed inside that window into a
/// `suppressed` count carried on the next line that does get emitted.
///
/// Exists because this branch ships with no rate limiting on failed auth —
/// accepted because the deployment target is a trusted container network,
/// but that acceptance only holds if an operator can see abuse happening.
/// An unthrottled `tracing::warn!` per rejection would itself be the
/// amplification vector: a caller with no valid token can still drive
/// hundreds of rejections per second, and hundreds of log lines per second
/// is exactly what `docker-compose.yml`'s `logging:` limits exist to bound
/// (see its comment) — better to never generate that volume in the first
/// place.
pub(crate) struct RejectionThrottle {
    /// Rejections observed since the last emitted line, including the one
    /// about to trigger emission — so `total - 1` on the line that fires is
    /// the count of OTHER rejections folded into it.
    pending: AtomicU64,
    last_emit: Mutex<Instant>,
}

impl RejectionThrottle {
    fn new() -> Self {
        Self {
            pending: AtomicU64::new(0),
            // Set far enough in the past that the very first rejection this
            // process ever sees emits immediately, rather than silently
            // waiting up to a second for a window that only just opened.
            last_emit: Mutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    /// Record one rejection. Returns `Some(suppressed)` exactly when the
    /// caller should emit a log line — `suppressed` is how many OTHER
    /// rejections landed inside the same one-second window — or `None` when
    /// a line already went out within the last second and this rejection is
    /// being counted silently instead.
    fn record(&self) -> Option<u64> {
        self.pending.fetch_add(1, Ordering::Relaxed);
        let mut last = self.last_emit.lock().unwrap();
        if last.elapsed() < Duration::from_secs(1) {
            return None;
        }
        *last = Instant::now();
        drop(last);
        let total = self.pending.swap(0, Ordering::Relaxed);
        Some(total.saturating_sub(1))
    }
}

/// SHA-256 a byte string. Hashing first keeps the constant-time comparison
/// fixed-length regardless of token length.
pub(crate) fn digest(s: &str) -> [u8; 32] {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&out);
    buf
}

/// Resolve the effective `Host` allow-list.
///
/// `None` means validation is disabled. rmcp defaults to a loopback-only
/// list, which would 403 an agent connecting to `http://rover:7683/mcp` —
/// the only deployment shape this transport exists for. DNS rebinding is an
/// attack on a *localhost* service via a victim's browser, so on a
/// deliberately exposed port the check prevents nothing.
#[must_use]
pub fn resolve_allowed_hosts(configured: &[String], bind: SocketAddr) -> Option<Vec<String>> {
    if configured.iter().any(|h| h == "*") {
        return None;
    }
    if !configured.is_empty() {
        return Some(configured.to_vec());
    }
    if bind.ip().is_loopback() {
        Some(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ])
    } else {
        None
    }
}

async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        format!("ok {}\n", env!("CARGO_PKG_VERSION")),
    )
}

async fn readyz(State(state): State<HttpState>) -> impl IntoResponse {
    match state.db.schema_version().await {
        Ok(v) => (StatusCode::OK, format!("ready schema_version={v}\n")).into_response(),
        Err(e) => {
            tracing::warn!(target: "rover::mcp", error = ?e, "readyz probe failed");
            (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response()
        }
    }
}

/// Strip a `Bearer ` prefix from an `Authorization` header value.
///
/// The scheme name is matched case-insensitively — RFC 9110 §11.1 and RFC
/// 6750 §2.1 both specify that the auth-scheme token is case-insensitive, so
/// a conforming client sending `bearer <token>` or `BEARER <token>` must
/// still be accepted. The separating space and the token itself are matched
/// exactly, same as before: this only relaxes the scheme's case, nothing
/// else (no tolerance added for extra whitespace).
fn strip_bearer_prefix(v: &str) -> Option<&str> {
    let (scheme, token) = v.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(token)
    } else {
        None
    }
}

/// The failed-auth rejection log message.
///
/// Deliberately avoids the words "Bearer"/"Basic" followed by whitespace and
/// another token: `src/telemetry/redact.rs`'s `AUTH_HEADER_VALUE` regex
/// (`(?i)\b(Bearer|Basic)\s+\S+`) matches that shape anywhere in a field
/// value, credential or not, and previously ate the word "token" out of the
/// line this replaced (`"rejected request: invalid bearer token"` →
/// `"...invalid bearer <redacted>"` in the shipped log). Named as a constant,
/// rather than inlined at both `tracing::warn!` call sites below, specifically
/// so `rejection_message_survives_auth_redaction` can assert against the
/// exact production string instead of a copy that could drift from it.
const REJECTION_MESSAGE: &str = "rejected request: missing or invalid Authorization header";

/// Reject requests without a valid bearer token, before any dispatch work.
///
/// The response does not distinguish "absent" from "wrong", and the presented
/// token is never logged.
async fn require_bearer(State(state): State<HttpState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.token_digest else {
        return next.run(req).await; // auth disabled
    };

    // `ConnectInfo` is populated by `serve_http` via
    // `into_make_service_with_connect_info::<SocketAddr>()`, which only a
    // real `axum::serve` over a bound socket provides. In-process
    // `tower::ServiceExt::oneshot` tests (the whole `mcp_http_*` router
    // suite) build the `Request` directly and never go through that make
    // service, so this extension is absent there — read it as an `Option`
    // via `extensions().get()` rather than as a required extractor
    // parameter, so its absence is tolerated rather than failing the
    // extraction (which would 500 every oneshot test in that suite).
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);

    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(strip_bearer_prefix)
        .map(digest);

    // SECURITY: compare as slices, not arrays, and via `ConstantTimeEq`, not
    // `==`. subtle 2.6.1 implements `ConstantTimeEq` for `[T]` but NOT for
    // `[T; N]`, so `p.ct_eq(&expected)` would rely on deref coercion finding
    // the slice impl — be explicit instead. A plain `==` here would return as
    // soon as it finds the first differing byte, leaking (via response
    // timing) how many leading bytes of a guess matched the real digest —
    // exactly the oracle a constant-time comparison exists to deny.
    let ok = presented.is_some_and(|p| {
        use subtle::ConstantTimeEq as _;
        p.as_slice().ct_eq(expected.as_slice()).into()
    });

    if ok {
        return next.run(req).await;
    }

    // Throttled to at most one line per second (`RejectionThrottle`): with
    // no rate limiting on failed auth, an unthrottled line per rejection
    // would itself be a disk-filling amplification vector for an
    // unauthenticated caller. `record()` returns `Some(suppressed)` only on
    // the call that should actually emit.
    if let Some(suppressed) = state.rejection_throttle.record() {
        // `message = REJECTION_MESSAGE` (a named field), not a trailing
        // format-string literal: both compile to the same `message` field
        // key, but this form passes the `&'static str` through `Value`'s
        // string impl untouched — the same code path
        // `rejection_message_survives_auth_redaction` exercises directly —
        // rather than through `Arguments`'s `Debug` impl, which this test
        // suite has no independent way to pin as behaviourally identical.
        match peer {
            Some(addr) => tracing::warn!(
                target: "rover::mcp",
                peer = %addr,
                suppressed,
                message = REJECTION_MESSAGE
            ),
            None => tracing::warn!(
                target: "rover::mcp",
                peer = "unknown",
                suppressed,
                message = REJECTION_MESSAGE
            ),
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        "unauthorized\n",
    )
        .into_response()
}

/// Log each request that reaches this layer with method, status, and
/// duration. Layered inside auth (outermost) but outside the body-limit
/// layer: an oversize-body rejection (`413`) happens on the inner side of
/// this middleware's `next.run`, so it is captured here. An auth rejection
/// (`401`) happens on the outer side — `require_bearer` never calls `next`
/// for a rejected request, so this layer never runs for one — but that case
/// already has its own `tracing::warn!` at the point of rejection. Tool-level
/// detail (the tool name, buried in the JSON-RPC body) is not logged here;
/// parsing the body at this layer would mean buffering it twice.
async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let started = std::time::Instant::now();
    let res = next.run(req).await;
    tracing::debug!(
        target: "rover::mcp",
        %method,
        status = res.status().as_u16(),
        duration_ms = started.elapsed().as_millis() as u64,
        "http request"
    );
    res
}

/// Build the `StreamableHttpServerConfig` for the `/mcp` service from the
/// resolved `HttpState`.
fn mcp_service_config(state: &HttpState) -> StreamableHttpServerConfig {
    let mut cfg = StreamableHttpServerConfig::default()
        // Rover has no server-initiated messages — no sampling, no roots,
        // `list_changed: false` — so SSE framing buys nothing and every POST
        // is self-contained.
        .with_stateful_mode(false)
        .with_json_response(true);
    cfg = match &state.allowed_hosts {
        None => cfg.disable_allowed_hosts(),
        Some(hosts) => cfg.with_allowed_hosts(hosts.clone()),
    };
    if !state.allowed_origins.is_empty() {
        cfg = cfg.with_allowed_origins(state.allowed_origins.clone());
    }
    cfg
}

/// Build the router. Pure — no I/O, no sockets — so tests drive it in-process
/// via `tower::ServiceExt::oneshot`. Returns `Router<()>` because axum
/// implements `Service` only for a router with its state already applied.
///
/// No `#[must_use]` here: `Router<()>` is already `#[must_use]` in axum, and
/// stacking our own triggers `clippy::double_must_use`.
pub fn router(state: HttpState) -> Router<()> {
    let public = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state.clone());

    let handler = state.handler.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(handler.clone()),
        // `NeverSessionManager` is correct, not a placeholder: the
        // `M: SessionManager` bound is on the impl block regardless of
        // stateful mode, and the stateless path never calls it — GET/DELETE
        // are answered 405 before the manager is touched, and POST uses
        // `serve_directly` + `OneshotTransport`.
        Arc::new(NeverSessionManager::default()),
        mcp_service_config(&state),
    );

    let protected = Router::new()
        // `route_service`, not `nest_service`: it registers the service for
        // ALL methods, so GET/DELETE reach rmcp and get its 405 rather than
        // axum's, and it avoids `nest`'s path-stripping, which one fixed
        // route has no use for.
        //
        // CRITICAL ordering: `.route_service` must come BEFORE `.layer`.
        // axum applies each layer only to routes already registered, so
        // appending the route after the layers would leave `/mcp`
        // completely unauthenticated.
        .route_service("/mcp", mcp)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(axum::middleware::from_fn(log_request))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));

    public.merge(protected)
}

/// Serve MCP over Streamable HTTP until SIGINT/SIGTERM, then drain.
///
/// # Errors
///
/// Returns an error if the listener cannot bind or the runtime cannot be
/// built. JSON-RPC and tool errors become wire responses and do not bubble.
pub async fn serve_http(
    db: Db,
    config: Arc<Config>,
    ssrf_level: SsrfLevel,
    ssrf_project_root: Option<std::path::PathBuf>,
    har_recorder: Option<Arc<crate::fetcher::har::HarRecorder>>,
    bind: SocketAddr,
) -> anyhow::Result<()> {
    let token = std::env::var("ROVER_HTTP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    warn_on_posture(bind, token.as_deref(), &config, ssrf_level);

    // Bind BEFORE build_runtime. `build_runtime` upserts the `servers` row and
    // spawns the scheduler; if the bind then fails (port in use) we would
    // return early without `runtime.shutdown()`, leaving a live row until the
    // reaper catches it.
    let listener = tokio::net::TcpListener::bind(bind).await?;

    let runtime = build_runtime(
        db.clone(),
        config.clone(),
        ssrf_level,
        ssrf_project_root,
        har_recorder,
        TransportKind::Http,
    )
    .await?;

    let cancel = runtime.cancel.clone();
    let state = HttpState::new(
        runtime.handler.clone(),
        db,
        token.as_deref(),
        &config.http,
        bind,
    );
    let app = router(state);

    // Report the *resolved* address, not the requested one, so a `:0`
    // ephemeral bind logs the port actually assigned — tests parse this line
    // to discover the port.
    let local_addr = listener.local_addr()?;
    tracing::info!(
        target: "rover::mcp",
        addr = %local_addr,
        "rover mcp HTTP listening (POST /mcp, GET /healthz, GET /readyz)"
    );

    // `into_make_service_with_connect_info` is what populates the
    // `ConnectInfo<SocketAddr>` request extension `require_bearer` reads for
    // the rejection warn's `peer` field. Plain `axum::serve(listener, app)`
    // (the prior wiring) never inserts it — the peer address would silently
    // never appear in that log line no matter what the middleware did with
    // it.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { cancel.cancelled().await })
    .await?;

    tracing::info!(target: "rover::mcp", "HTTP server drained; shutting down");
    // `axum::serve` has returned, so the router and its service factory (and
    // therefore their `RoverHandler` clones) are dropped. Only now is it safe
    // to run the shutdown tail — see `Runtime::shutdown`.
    runtime.shutdown().await
}

/// State the security posture at boot. Never silently.
fn warn_on_posture(bind: SocketAddr, token: Option<&str>, config: &Config, ssrf_level: SsrfLevel) {
    let public = !bind.ip().is_loopback();

    match token {
        None => {
            if public {
                tracing::warn!(
                    target: "rover::mcp",
                    %bind,
                    "binding a NON-LOOPBACK address with no ROVER_HTTP_TOKEN configured: \
                     every caller can spend your configured cloud API keys, read the entire \
                     cache database, and fetch the web under your IP and User-Agent. Set \
                     ROVER_HTTP_TOKEN."
                );
            }
        }
        Some(t) => {
            if t.len() < 16 {
                tracing::warn!(
                    target: "rover::mcp",
                    len = t.len(),
                    "ROVER_HTTP_TOKEN is shorter than 16 characters; a leaked digest of a \
                     short token is offline-crackable. Generate one with: openssl rand -hex 32"
                );
            }
            tracing::info!(
                target: "rover::mcp",
                "ROVER_HTTP_TOKEN configured; POST /mcp requires it"
            );
        }
    }

    if resolve_allowed_hosts(&config.http.allowed_hosts, bind).is_none() {
        tracing::info!(
            target: "rover::mcp",
            "Host validation DISABLED (non-loopback bind or explicit \"*\"): DNS-rebinding \
             defence protects localhost services from browsers and adds nothing here"
        );
    }

    if public && token.is_none() && matches!(ssrf_level, SsrfLevel::Lan | SsrfLevel::None) {
        tracing::warn!(
            target: "rover::mcp",
            level = ?ssrf_level,
            "UNAUTHENTICATED INTERNAL-NETWORK FETCH PROXY: a public bind with no token and \
             ssrf.level = lan|none lets any caller reach RFC1918 and ULA addresses on this \
             network. Set ROVER_HTTP_TOKEN or lower ssrf.level."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// Pins the exact defect this module's history warns about: verified
    /// real output once showed `message=rejected request: invalid bearer
    /// <redacted>` because `src/telemetry/redact.rs`'s `AUTH_HEADER_VALUE`
    /// regex (`(?i)\b(Bearer|Basic)\s+\S+`) matched the prose "bearer
    /// token" in the old message and ate the word "token". This test runs
    /// the CURRENT `REJECTION_MESSAGE` through the real redactor —
    /// `redact_authorization`, the exact function `RedactingFormatEvent`
    /// calls on every field value including the implicit `message` field —
    /// and asserts it comes back byte-for-byte unchanged. It targets the
    /// production string directly (not a copy that could drift from it),
    /// so a future edit that reintroduces a `Bearer`/`Basic`-shaped
    /// substring fails here, deterministically, without needing to spin up
    /// a real tracing subscriber or a live server.
    #[test]
    fn rejection_message_survives_auth_redaction() {
        let redacted = crate::telemetry::redact::redact_authorization(REJECTION_MESSAGE);
        assert_eq!(
            redacted, REJECTION_MESSAGE,
            "the rejection message is no longer immune to the Bearer/Basic \
             redaction regex — got: {redacted:?}"
        );
    }

    /// A fresh throttle emits on its very first rejection — the "far enough
    /// in the past" initial `last_emit` matters: a naive `Instant::now()`
    /// would make the first caller wait up to a second before anything
    /// logged at all.
    #[test]
    fn rejection_throttle_emits_immediately_on_first_rejection() {
        let t = RejectionThrottle::new();
        assert_eq!(
            t.record(),
            Some(0),
            "first rejection should emit with 0 suppressed"
        );
    }

    /// Deterministic without sleeping: a whole burst of `record()` calls
    /// back-to-back completes in microseconds, comfortably inside the
    /// one-second window, so only the very first call can possibly emit —
    /// every call after it must be folded into `pending` instead of
    /// producing its own line.
    #[test]
    fn rejection_throttle_folds_a_burst_into_one_suppressed_count() {
        let t = RejectionThrottle::new();
        assert_eq!(t.record(), Some(0));
        for _ in 0..99 {
            assert_eq!(
                t.record(),
                None,
                "a rejection inside the same one-second window must not emit its own line"
            );
        }
        // The window has not elapsed yet (this test runs in microseconds),
        // so nothing has drained `pending` — confirm the count silently
        // accumulated rather than being dropped.
        assert_eq!(t.pending.load(Ordering::Relaxed), 99);
    }

    #[test]
    fn empty_config_on_loopback_bind_uses_rmcp_default_list() {
        let got = resolve_allowed_hosts(&[], addr("127.0.0.1:7683"));
        assert_eq!(
            got,
            Some(vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "::1".to_string()
            ])
        );
    }

    #[test]
    fn empty_config_on_non_loopback_bind_disables_validation() {
        assert_eq!(resolve_allowed_hosts(&[], addr("0.0.0.0:7683")), None);
    }

    #[test]
    fn star_disables_validation_even_on_loopback() {
        let star = vec!["*".to_string()];
        assert_eq!(resolve_allowed_hosts(&star, addr("127.0.0.1:7683")), None);
    }

    #[test]
    fn explicit_list_passes_through_on_any_bind() {
        let list = vec!["rover:7683".to_string()];
        assert_eq!(
            resolve_allowed_hosts(&list, addr("0.0.0.0:7683")),
            Some(list.clone())
        );
        assert_eq!(
            resolve_allowed_hosts(&list, addr("127.0.0.1:7683")),
            Some(list)
        );
    }

    /// Guards the constant-time comparison itself, not just its observable
    /// pass/fail outcome. Replacing `p.as_slice().ct_eq(expected.as_slice())`
    /// with `presented == Some(expected)` still makes every behavioural test
    /// in `tests/mcp_http_auth.rs` pass — a plain `==` is functionally
    /// correct, it just leaks timing information proportional to how many
    /// leading bytes of a guess match the real digest, which is exactly the
    /// class of bug those tests cannot see. So this test reads the source
    /// directly and checks the comparison operator that ships, not just what
    /// it computes.
    ///
    /// Comments are stripped before searching: the `// SECURITY:` comment
    /// right above the real comparison mentions `ct_eq` in prose, so a naive
    /// substring search over the raw body would still pass even if the code
    /// were reverted to a plain `==` underneath an unchanged comment — this
    /// was caught by hand-testing the guard (temporarily swapping in `==` and
    /// confirming the *first* version of this test kept passing) before this
    /// version shipped, not assumed safe.
    #[test]
    fn require_bearer_uses_constant_time_comparison() {
        let src = include_str!("http.rs");
        let body = function_body(src, "async fn require_bearer")
            .expect("require_bearer not found in src/mcp/http.rs — did it get renamed?");
        let code_only = strip_line_comments(body);
        assert!(
            code_only.contains("ct_eq"),
            "require_bearer no longer calls `ct_eq` (subtle::ConstantTimeEq) on the \
             token digests — a plain `==` comparison is not constant-time and \
             reintroduces a timing side-channel that leaks the correct bearer \
             token one byte at a time"
        );
    }

    /// Drop everything from the first `//` to the end of each line. Good
    /// enough for `require_bearer`'s body specifically: none of its string
    /// literals contain `//`, so this can't misfire by truncating a real
    /// string value — it only removes actual comment text.
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract the brace-balanced body of the first function in `src` whose
    /// text contains `signature`, from the signature's opening `{` through
    /// its matching closing `}`. Used only by the source-scraping guard
    /// above; not a general-purpose parser (no awareness of braces inside
    /// string/char literals or comments — `require_bearer` doesn't contain
    /// any, so this is sufficient here).
    fn function_body<'a>(src: &'a str, signature: &str) -> Option<&'a str> {
        let start = src.find(signature)?;
        let open = src[start..].find('{')? + start;
        let mut depth = 0usize;
        for (i, ch) in src[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&src[open..=open + i]);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

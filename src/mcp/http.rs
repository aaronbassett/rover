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
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tower_http::limit::RequestBodyLimitLayer;

use crate::config::HttpConfig;
use crate::mcp::handler::RoverHandler;
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
        }
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

/// Reject requests without a valid bearer token, before any dispatch work.
///
/// The response does not distinguish "absent" from "wrong", and the presented
/// token is never logged.
async fn require_bearer(State(state): State<HttpState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.token_digest else {
        return next.run(req).await; // auth disabled
    };

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

    tracing::warn!(target: "rover::mcp", "rejected request: invalid bearer token");
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        "unauthorized\n",
    )
        .into_response()
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));

    public.merge(protected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
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

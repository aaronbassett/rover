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

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::config::HttpConfig;
use crate::mcp::handler::RoverHandler;
use crate::storage::Db;

/// Router state shared by every handler.
///
/// `handler`, `token_digest`, `allowed_hosts`, and `allowed_origins` are
/// unread until the bearer-auth middleware (Task 5) and the `/mcp` route
/// (Task 6) land; `#[allow(dead_code)]` keeps `warnings = deny` happy in the
/// meantime, matching `RoverHandler::transport`'s precedent.
#[derive(Clone)]
pub struct HttpState {
    #[allow(dead_code)]
    pub(crate) handler: RoverHandler,
    pub(crate) db: Db,
    /// SHA-256 of the configured bearer token, or `None` for no auth.
    #[allow(dead_code)]
    pub(crate) token_digest: Option<[u8; 32]>,
    /// Resolved `Host` allow-list: `None` means validation is disabled.
    #[allow(dead_code)]
    pub(crate) allowed_hosts: Option<Vec<String>>,
    #[allow(dead_code)]
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

/// Build the router. Pure — no I/O, no sockets — so tests drive it in-process
/// via `tower::ServiceExt::oneshot`. Returns `Router<()>` because axum
/// implements `Service` only for a router with its state already applied.
///
/// No `#[must_use]` here: `Router<()>` is already `#[must_use]` in axum, and
/// stacking our own triggers `clippy::double_must_use`.
pub fn router(state: HttpState) -> Router<()> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
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
}

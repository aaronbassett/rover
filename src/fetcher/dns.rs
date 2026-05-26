//! DNS resolution with dial-time SSRF enforcement.
//!
//! Closes the TOCTOU window between Rover's pre-flight `validate_addresses`
//! and reqwest's internal dial-time resolution (`docs/security.md` §"DNS
//! rebinding"). A malicious authoritative resolver can return a public IP
//! to our pre-flight lookup and a loopback/RFC1918 address to reqwest's
//! later lookup; the connection then targets the unsafe IP even though the
//! request was authorised.
//!
//! Fix: install a custom [`reqwest::dns::Resolve`] on the shared client that
//! re-runs the same address validator at the moment of dial. The per-request
//! `SsrfLevel` is carried via the [`SSRF_LEVEL`] task-local, populated by
//! [`crate::fetcher::fetch`] (and any other module that needs validated
//! outbound DNS — see `robots.rs`, `headless/intercept.rs`).
//!
//! Requests issued without setting `SSRF_LEVEL` fall through to a plain
//! `tokio::net::lookup_host` with no policy check. This is intentional: the
//! resolver is shared by every consumer of the client (including test code
//! and the cloud captioner/summariser paths that don't go through our SSRF
//! gate), and silently rejecting their lookups would be surprising. Every
//! caller that should be policed sets the task-local explicitly.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::net::lookup_host;

use crate::fetcher::ssrf::{SsrfError, SsrfLevel, validate_addresses};

tokio::task_local! {
    /// Per-request SSRF level consulted by [`SsrfValidatingResolver`].
    ///
    /// Set via `SSRF_LEVEL.scope(level, fut).await` around any call into
    /// `reqwest::Client` that should be policed (so redirects to a new host
    /// inside a single request are also covered).
    pub static SSRF_LEVEL: SsrfLevel;
}

/// Wrapper error returned by the resolver when SSRF policy rejects an
/// address at dial time. Carried inside `reqwest::Error`'s source chain so
/// the retry classifier can promote it to a fatal failure rather than
/// looping on what looks like a transient connect error.
#[derive(Debug)]
pub struct DialBlocked(pub SsrfError);

impl std::fmt::Display for DialBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ssrf policy blocked dial-time address resolution: {}",
            self.0
        )
    }
}

impl std::error::Error for DialBlocked {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A `reqwest::dns::Resolve` implementation that enforces the SSRF address
/// policy on every resolution.
#[derive(Default)]
pub struct SsrfValidatingResolver;

impl Resolve for SsrfValidatingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // reqwest sets the real destination port on the SocketAddr after
            // it gets back from the resolver, so port 0 here is fine.
            let target = format!("{host}:0");
            let resolved: Vec<SocketAddr> = lookup_host(target.as_str())
                .await
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?
                .collect();

            if let Ok(level) = SSRF_LEVEL.try_with(|l| *l) {
                let ips: Vec<IpAddr> = resolved.iter().map(|s| s.ip()).collect();
                if let Err(e) = validate_addresses(&ips, level) {
                    return Err(
                        Box::new(DialBlocked(e)) as Box<dyn std::error::Error + Send + Sync>
                    );
                }
            }

            let iter: Addrs = Box::new(resolved.into_iter());
            Ok(iter)
        })
    }
}

/// Convenience: an `Arc` wrapper suitable for `ClientBuilder::dns_resolver`.
pub fn shared_resolver() -> Arc<SsrfValidatingResolver> {
    Arc::new(SsrfValidatingResolver)
}

/// Walk a `reqwest::Error`'s source chain looking for a [`DialBlocked`].
///
/// reqwest wraps resolver errors in its own `reqwest::Error` (typically with
/// `is_connect() == true`), so callers that want to distinguish "SSRF blocked
/// the dial" from "the server is down" need to inspect the chain. Used by
/// `retry.rs` to keep retries from re-trying a forbidden destination.
pub fn dial_blocked_cause<'a>(
    err: &'a (dyn std::error::Error + 'static),
) -> Option<&'a DialBlocked> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(blocked) = e.downcast_ref::<DialBlocked>() {
            return Some(blocked);
        }
        current = e.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn resolver_passes_through_when_no_context_set() {
        // No SSRF_LEVEL scope active → the resolver should not consult the
        // policy at all. Use a name that resolves locally on every platform.
        let r = SsrfValidatingResolver;
        let name: Name = "localhost".parse().unwrap();
        let result = r.resolve(name).await;
        // localhost should resolve; we don't care which addresses.
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn resolver_blocks_loopback_under_strict() {
        let r = SsrfValidatingResolver;
        let name: Name = "localhost".parse().unwrap();
        let result = SSRF_LEVEL
            .scope(SsrfLevel::Strict, async { r.resolve(name).await })
            .await;
        let Err(err) = result else {
            panic!("strict must reject loopback");
        };
        assert!(
            dial_blocked_cause(&*err).is_some(),
            "expected DialBlocked in source chain, got: {err}",
        );
    }

    #[tokio::test]
    async fn resolver_allows_loopback_under_loopback_level() {
        let r = SsrfValidatingResolver;
        let name: Name = "localhost".parse().unwrap();
        let result = SSRF_LEVEL
            .scope(SsrfLevel::Loopback, async { r.resolve(name).await })
            .await;
        assert!(result.is_ok(), "loopback level must accept localhost");
    }

    #[test]
    fn dial_blocked_walks_source_chain() {
        let inner = SsrfError::Address {
            address: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            level: SsrfLevel::Strict,
            reason: "loopback IPv4",
        };
        let dial = DialBlocked(inner);
        // Wrap in another error layer to exercise the chain walk.
        #[derive(Debug)]
        struct Wrap(DialBlocked);
        impl std::fmt::Display for Wrap {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "wrap")
            }
        }
        impl std::error::Error for Wrap {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let wrapped = Wrap(dial);
        assert!(dial_blocked_cause(&wrapped).is_some());
    }
}

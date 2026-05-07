//! SSRF policy enforcement.
//!
//! M1 implements only the `Strict` level (PRD §5.5):
//! - Public IPs only (no loopback, no private, no link-local, no multicast,
//!   no broadcast, no unspecified)
//! - `http://` or `https://` schemes only
//!
//! Per design supplement §2.4, DNS-rebinding-resistant fetching is deferred
//! to v2: we validate the addresses returned from initial resolution but do
//! not pin them through the connection.

use std::net::IpAddr;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfLevel {
    /// Public IPs only, http/https only.
    Strict,

    /// **Test-only.** Strict + loopback. Used by integration tests against
    /// wiremock. Not exposed in the production CLI/config surface.
    #[cfg(any(test, feature = "test-loopback"))]
    TestLoopback,
}

#[derive(Debug, Error)]
pub enum SsrfError {
    #[error("scheme `{scheme}` is not allowed (Strict level requires http or https)")]
    Scheme { scheme: String },

    #[error("URL has no host")]
    NoHost,

    #[error("address {address} is not allowed under SSRF level {level:?} ({reason})")]
    Address {
        address: IpAddr,
        level: SsrfLevel,
        reason: &'static str,
    },
}

/// Validate the URL itself (scheme, presence of host).
///
/// Call this *before* DNS resolution — it's cheap and rules out bad URLs early.
pub fn validate_url(url: &Url, level: SsrfLevel) -> Result<(), SsrfError> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(SsrfError::Scheme {
                scheme: other.to_string(),
            });
        }
    }
    if url.host_str().is_none() {
        return Err(SsrfError::NoHost);
    }
    let _ = level; // currently no scheme variation across levels
    Ok(())
}

/// Validate every resolved address against the policy.
///
/// Pass the `IpAddr`s returned from a DNS lookup. If *any* address violates
/// the policy, this returns an error and the request must not proceed.
pub fn validate_addresses(addrs: &[IpAddr], level: SsrfLevel) -> Result<(), SsrfError> {
    for &addr in addrs {
        let strict_reject = strict_reject_reason(addr);
        match level {
            SsrfLevel::Strict => {
                if let Some(reason) = strict_reject {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            #[cfg(any(test, feature = "test-loopback"))]
            SsrfLevel::TestLoopback => {
                if let Some(reason) = strict_reject {
                    if !addr.is_loopback() {
                        return Err(SsrfError::Address {
                            address: addr,
                            level,
                            reason,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn strict_reject_reason(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return Some("loopback IPv4");
            }
            if v4.is_private() {
                return Some("private IPv4 (RFC1918)");
            }
            if v4.is_link_local() {
                return Some("link-local IPv4");
            }
            if v4.is_multicast() {
                return Some("multicast IPv4");
            }
            if v4.is_broadcast() {
                return Some("broadcast IPv4");
            }
            if v4.is_unspecified() {
                return Some("unspecified IPv4 (0.0.0.0)");
            }
            // 100.64.0.0/10 — CGN. Not in std as a method, check by hand.
            let octets = v4.octets();
            if octets[0] == 100 && (octets[1] & 0xC0) == 0x40 {
                return Some("carrier-grade NAT (100.64.0.0/10)");
            }
            None
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return Some("loopback IPv6");
            }
            if v6.is_multicast() {
                return Some("multicast IPv6");
            }
            if v6.is_unspecified() {
                return Some("unspecified IPv6 (::)");
            }
            // Unique local fc00::/7
            let segs = v6.segments();
            if (segs[0] & 0xfe00) == 0xfc00 {
                return Some("unique-local IPv6 (fc00::/7)");
            }
            // Link-local fe80::/10
            if (segs[0] & 0xffc0) == 0xfe80 {
                return Some("link-local IPv6 (fe80::/10)");
            }
            // IPv4-mapped/embedded — reject too; check by mapping back
            if let Some(v4) = v6.to_ipv4_mapped() {
                if let Some(reason) = strict_reject_reason(IpAddr::V4(v4)) {
                    return Some(reason);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn http_https_allowed_strict() {
        assert!(
            validate_url(
                &Url::parse("http://example.com/").unwrap(),
                SsrfLevel::Strict
            )
            .is_ok()
        );
        assert!(
            validate_url(
                &Url::parse("https://example.com/").unwrap(),
                SsrfLevel::Strict
            )
            .is_ok()
        );
    }

    #[test]
    fn file_scheme_rejected_strict() {
        let err = validate_url(
            &Url::parse("file:///etc/passwd").unwrap(),
            SsrfLevel::Strict,
        )
        .unwrap_err();
        assert!(matches!(err, SsrfError::Scheme { .. }));
    }

    #[test]
    fn ftp_scheme_rejected_strict() {
        let err = validate_url(
            &Url::parse("ftp://example.com/").unwrap(),
            SsrfLevel::Strict,
        )
        .unwrap_err();
        assert!(matches!(err, SsrfError::Scheme { .. }));
    }

    #[test]
    fn loopback_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn private_rejected_strict() {
        for addr in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
        ] {
            assert!(
                validate_addresses(&[addr], SsrfLevel::Strict).is_err(),
                "{addr}"
            );
        }
    }

    #[test]
    fn link_local_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv6_loopback_rejected_strict() {
        assert!(validate_addresses(&[IpAddr::V6(Ipv6Addr::LOCALHOST)], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv6_ula_rejected_strict() {
        let addr: IpAddr = "fd00::1".parse().unwrap();
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn ipv4_mapped_loopback_rejected_strict() {
        let addr: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn cgn_rejected_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_err());
    }

    #[test]
    fn public_ipv4_allowed_strict() {
        let addr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert!(validate_addresses(&[addr], SsrfLevel::Strict).is_ok());
    }

    #[test]
    fn any_violator_in_set_rejects() {
        let addrs = [
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ];
        assert!(validate_addresses(&addrs, SsrfLevel::Strict).is_err());
    }
}

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

    /// Strict + 127.0.0.0/8 + ::1.
    Loopback,

    /// Loopback + `file://` URLs descendant of `[ssrf] project_root` after
    /// symlink resolution. File scheme handling lives in `validate_url`
    /// (see also `src/fetcher/cached.rs` for the dispatch).
    Project,

    /// Project + RFC1918 + IPv6 ULAs (`fc00::/7`).
    Lan,

    /// Trust the user. Link-local, multicast, broadcast, `0.0.0.0`, and
    /// `255.255.255.255` are still blocked — the always-floor.
    None,
}

impl SsrfLevel {
    pub fn parse(s: &str) -> Result<Self, SsrfError> {
        match s {
            "strict" => Ok(Self::Strict),
            "loopback" => Ok(Self::Loopback),
            "project" => Ok(Self::Project),
            "lan" => Ok(Self::Lan),
            "none" => Ok(Self::None),
            other => Err(SsrfError::UnknownLevel {
                level: other.to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Loopback => "loopback",
            Self::Project => "project",
            Self::Lan => "lan",
            Self::None => "none",
        }
    }
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

    #[error("unknown ssrf level `{level}` (expected one of: strict, loopback, project, lan, none)")]
    UnknownLevel { level: String },

    #[error("file:// URLs are not allowed at level {level:?}")]
    FileSchemeNotAllowed { level: SsrfLevel },

    #[error("file path {path:?} is not a descendant of project_root {root:?}")]
    FileOutsideProjectRoot {
        path: std::path::PathBuf,
        root: std::path::PathBuf,
    },

    #[error("file path {path:?} could not be canonicalized: {source}")]
    FileCanonicalize {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("project_root is required when ssrf.level = project")]
    ProjectRootMissing,
}

/// Validate the URL itself (scheme, presence of host, file:// rules).
///
/// For `Project` level, callers must pass the canonicalized `project_root`
/// via `validate_url_with_project_root`. Calling `validate_url` (no root)
/// with `level == Project` yields `SsrfError::ProjectRootMissing` for any
/// `file://` URL.
pub fn validate_url(url: &Url, level: SsrfLevel) -> Result<(), SsrfError> {
    validate_url_with_project_root(url, level, None)
}

/// Validate the URL itself (scheme, presence of host, file:// rules).
///
/// For `http`/`https`, this checks the scheme and that a host is present.
/// For `file://`, the URL must be allowed at `level` (Project, Lan, None),
/// canonicalized successfully (resolving symlinks), and the resulting path
/// must be a descendant of the pre-canonicalized `project_root`.
///
/// Call this *before* DNS resolution — it's cheap and rules out bad URLs early.
pub fn validate_url_with_project_root(
    url: &Url,
    level: SsrfLevel,
    project_root: Option<&std::path::Path>,
) -> Result<(), SsrfError> {
    match url.scheme() {
        "http" | "https" => {
            if url.host_str().is_none() {
                return Err(SsrfError::NoHost);
            }
            Ok(())
        }
        "file" => {
            if !matches!(level, SsrfLevel::Project | SsrfLevel::Lan | SsrfLevel::None) {
                return Err(SsrfError::FileSchemeNotAllowed { level });
            }
            let root = project_root.ok_or(SsrfError::ProjectRootMissing)?;
            let raw_path = url
                .to_file_path()
                .map_err(|()| SsrfError::FileCanonicalize {
                    path: std::path::PathBuf::from(url.path()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "file:// URL has no local path",
                    ),
                })?;
            let canon =
                std::fs::canonicalize(&raw_path).map_err(|source| SsrfError::FileCanonicalize {
                    path: raw_path.clone(),
                    source,
                })?;
            if !canon.starts_with(root) {
                return Err(SsrfError::FileOutsideProjectRoot {
                    path: canon,
                    root: root.to_path_buf(),
                });
            }
            Ok(())
        }
        other => Err(SsrfError::Scheme {
            scheme: other.to_string(),
        }),
    }
}

/// Validate every resolved address against the policy.
///
/// Pass the `IpAddr`s returned from a DNS lookup. If *any* address violates
/// the policy, this returns an error and the request must not proceed.
pub fn validate_addresses(addrs: &[IpAddr], level: SsrfLevel) -> Result<(), SsrfError> {
    for &addr in addrs {
        // Always-floor: every level blocks these.
        if let Some(reason) = always_floor_reason(addr) {
            return Err(SsrfError::Address {
                address: addr,
                level,
                reason,
            });
        }
        match level {
            SsrfLevel::Strict => {
                if let Some(reason) = strict_reject_reason(addr) {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::Loopback | SsrfLevel::Project => {
                if let Some(reason) = strict_reject_reason(addr)
                    && !addr.is_loopback()
                {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::Lan => {
                if let Some(reason) = strict_reject_reason(addr)
                    && !(addr.is_loopback() || is_rfc1918(addr) || is_ipv6_ula(addr))
                {
                    return Err(SsrfError::Address {
                        address: addr,
                        level,
                        reason,
                    });
                }
            }
            SsrfLevel::None => {
                // Already passed the always-floor; allow everything else.
            }
        }
    }
    Ok(())
}

fn always_floor_reason(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => {
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
        }
        IpAddr::V6(v6) => {
            if v6.is_multicast() {
                return Some("multicast IPv6");
            }
            if v6.is_unspecified() {
                return Some("unspecified IPv6 (::)");
            }
            let segments = v6.segments();
            if (segments[0] & 0xffc0) == 0xfe80 {
                return Some("link-local IPv6 (fe80::/10)");
            }
        }
    }
    None
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
            // shared address space (CGNAT) 100.64.0.0/10
            if v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64 {
                return Some("shared CGNAT IPv4 (100.64.0.0/10)");
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return Some("loopback IPv6");
            }
            if is_ipv6_ula(IpAddr::V6(v6)) {
                return Some("unique-local IPv6 (fc00::/7)");
            }
            // IPv4-mapped/embedded — reject too; check by mapping back.
            if let Some(v4) = v6.to_ipv4_mapped()
                && let Some(reason) = strict_reject_reason(IpAddr::V4(v4))
            {
                return Some(reason);
            }
        }
    }
    None
}

fn is_rfc1918(addr: IpAddr) -> bool {
    matches!(addr, IpAddr::V4(v4) if v4.is_private())
}

fn is_ipv6_ula(addr: IpAddr) -> bool {
    matches!(
        addr,
        IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00,
    )
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
        assert!(matches!(err, SsrfError::FileSchemeNotAllowed { .. }));
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

    #[test]
    fn loopback_accepts_127_block_and_v6_localhost() {
        use std::net::Ipv4Addr;
        assert!(
            validate_addresses(
                &[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
                SsrfLevel::Loopback
            )
            .is_ok()
        );
        assert!(
            validate_addresses(&[IpAddr::V6(Ipv6Addr::LOCALHOST)], SsrfLevel::Loopback).is_ok()
        );
    }

    #[test]
    fn loopback_still_rejects_rfc1918() {
        use std::net::Ipv4Addr;
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Loopback).is_err());
    }

    #[test]
    fn lan_accepts_rfc1918_and_ulas() {
        use std::net::{Ipv4Addr, Ipv6Addr};
        for v4 in &[
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(192, 168, 0, 1),
        ] {
            assert!(
                validate_addresses(&[IpAddr::V4(*v4)], SsrfLevel::Lan).is_ok(),
                "expected {v4} to be accepted at Lan",
            );
        }
        let ula = IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1));
        assert!(validate_addresses(&[ula], SsrfLevel::Lan).is_ok());
    }

    #[test]
    fn lan_still_rejects_link_local() {
        use std::net::Ipv4Addr;
        let addr = IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1));
        assert!(validate_addresses(&[addr], SsrfLevel::Lan).is_err());
    }

    #[test]
    fn project_level_inherits_loopback_ip_rules() {
        use std::net::Ipv4Addr;
        assert!(
            validate_addresses(
                &[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
                SsrfLevel::Project
            )
            .is_ok()
        );
        assert!(
            validate_addresses(
                &[IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
                SsrfLevel::Project
            )
            .is_err()
        );
    }

    #[test]
    fn none_accepts_arbitrary_public_ip() {
        use std::net::Ipv4Addr;
        let addr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert!(validate_addresses(&[addr], SsrfLevel::None).is_ok());
    }

    #[test]
    fn none_still_blocks_zero_address() {
        use std::net::Ipv4Addr;
        let addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        assert!(
            validate_addresses(&[addr], SsrfLevel::None).is_err(),
            "0.0.0.0 must be blocked at every level",
        );
    }

    #[test]
    fn ssrf_level_parses_from_str() {
        assert_eq!(SsrfLevel::parse("strict").unwrap(), SsrfLevel::Strict);
        assert_eq!(SsrfLevel::parse("loopback").unwrap(), SsrfLevel::Loopback);
        assert_eq!(SsrfLevel::parse("project").unwrap(), SsrfLevel::Project);
        assert_eq!(SsrfLevel::parse("lan").unwrap(), SsrfLevel::Lan);
        assert_eq!(SsrfLevel::parse("none").unwrap(), SsrfLevel::None);
        assert!(SsrfLevel::parse("bogus").is_err());
    }
}

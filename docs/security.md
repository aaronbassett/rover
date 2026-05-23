# Rover Security

Explicit security boundaries, threat-model decisions, and known v1 limitations. Updated alongside each milestone that changes the security surface.

## SSRF protection

The SSRF policy is governed by `[ssrf] level`. Every outbound URL is checked twice: scheme/host at parse time (`validate_url`), and every resolved address before the connection is opened (`validate_addresses`).

| Level | Allows |
| --- | --- |
| `strict` (default) | Public IPs only; `http` / `https` only. |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs descendant of `[ssrf] project_root` after symlink resolution. |
| `lan` | Project + RFC1918 + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. The always-floor (below) is still enforced. |

### Always-floor — blocked at every level

| Address class | Range |
| --- | --- |
| IPv4 link-local | `169.254.0.0/16` |
| IPv4 multicast | `224.0.0.0/4` |
| IPv4 broadcast | `255.255.255.255` |
| IPv4 unspecified | `0.0.0.0` |
| IPv6 multicast | `ff00::/8` |
| IPv6 unspecified | `::` |
| IPv6 link-local | `fe80::/10` |

`strict` adds rejections for loopback, RFC1918, the CGNAT range (`100.64.0.0/10`), IPv6 ULAs (`fc00::/7`), and IPv4-mapped IPv6 addresses whose mapped form would itself be rejected (e.g. `::ffff:127.0.0.1`).

If *any* address in the resolution set fails the policy, the entire request is rejected with a typed `SsrfError::Address`. Code: `ssrf_denied`.

## DNS rebinding (v2 limitation)

Per design supplement §2.4: Rover resolves a hostname, validates the resulting addresses against the active SSRF policy, then performs the actual HTTPS connection via the system resolver. **A TOCTOU window exists between validation and the connection.** A malicious authoritative DNS server that returns different addresses on subsequent queries (low TTL, then a private/loopback answer) could route a "safe" hostname's later requests to an unsafe IP.

**Mitigation in v2:** pin the resolution through `reqwest::ClientBuilder::resolve` so the validated addresses are the addresses dialled. Until then, deploy Rover behind a trusted recursive resolver in adversarial environments, or operate at the most restrictive `level` your workflow allows.

## `file://` symlink handling

When `[ssrf] level` is `project`, `lan`, or `none`, `file://` URLs are allowed. The path is canonicalized via `std::fs::canonicalize` (which resolves every symlink in the path) before being checked against the canonicalized `project_root`. A symlink whose target lives outside `project_root` is rejected after resolution with `SsrfError::FileOutsideProjectRoot`. URLs at `strict` / `loopback` are rejected with `SsrfError::FileSchemeNotAllowed` without ever touching the filesystem.

## Secret redaction

Rover redacts URL query-string values whose key name contains any of the following substrings (case-insensitive): `api_key`, `token`, `secret`, `password`. Redaction runs on every field value written by the custom tracing formatter (`RedactingFormatEvent`), so any URL logged via `tracing` is filtered before it hits the console or log file.

**Not redacted:**
- Authorization headers (`Authorization: Bearer ...`) — Rover does not currently inspect headers in the tracing layer.
- Request and response bodies in HAR files (`[debug] har_path`) — HAR is intended for debugging private traffic; protect the file with filesystem permissions.
- Environment variables. The `api_key_env` config field is a pointer; the resolved value is held in memory and never logged.

## Cache poisoning

Per PRD §16. The cache key is `(url, params)` — same URL with different upstream content produces different `content_hash` values, so an attacker who controls the upstream cannot serve poisoned data to a different URL's consumer. **However, the cache itself does not validate authenticity.** If an upstream is compromised and serves malicious content while Rover's cache entry is still fresh, that content is served from the cache on subsequent requests until the TTL expires.

Operators handling adversarial upstreams should:
1. Lower `[cache] default_ttl` (and possibly `max_ttl`) to bound the staleness window.
2. Use `force_refresh` on the MCP tool calls or `--force-refresh` on the CLI for traffic that must hit origin.
3. Avoid `[cache] override_no_store` for any host that legitimately sends `no-store`.

## Per-process rate limit scope (M5)

The rate limiter and concurrency semaphores live in process memory, not SQLite. Two concurrent `rover mcp` processes each maintain their own buckets; a tight shell loop of `rover fetch` invocations is not paced across process boundaries. This is acceptable for v1's single-user-local target. v2 may introduce cross-process state if profiling justifies it.

## Robots.txt fail-closed cache window (M5)

When a robots.txt fetch returns 5xx or times out, Rover caches a `disallow_all` sentinel for `[robots] failure_ttl` (default `5m`). During that window, all fetches to that host are refused with `robots_fetch_failed` / `robots_disallowed`. The short TTL ensures recovered servers are picked up quickly; for hosts whose robots endpoint is chronically broken, raise `failure_ttl` or list the host in `[robots] ignore_domains`.

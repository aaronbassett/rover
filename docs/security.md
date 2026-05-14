# Rover Security Notes (v1)

This document lists explicit security boundaries and known v1 limitations.
Updated alongside each milestone that changes the security surface.

## Known v1 Limitations

### DNS rebinding window during fetch
Per design supplement §2.4: Rover resolves a hostname, validates the IPs
against the active SSRF policy, then performs the actual HTTPS connection
via the system resolver. A TOCTOU window exists between the validation
and the connection. v2 will close this via `reqwest::ClientBuilder::resolve`.

### Per-process rate limit scope (M5)
The rate limiter and concurrency semaphores live in process memory, not
SQLite. Two concurrent `rover mcp` processes each maintain their own
buckets, and a tight shell loop of `rover fetch` invocations is not paced
across process boundaries. This is acceptable for v1's single-user-local
target. v2 may introduce cross-process state if profiling justifies it.

### Robots.txt fail-closed cache window (M5)
When a robots.txt fetch returns 5xx or times out, Rover caches a
`disallow_all` sentinel for `[robots] failure_ttl` (default 5 minutes).
During that window, all fetches to that host are refused with
`robots_fetch_failed` / `robots_disallowed`. The short TTL ensures
recovered servers are picked up quickly; for hosts whose robots endpoint
is chronically broken, raise `failure_ttl` or list the host in
`[robots] ignore_domains`.

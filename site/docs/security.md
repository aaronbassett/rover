---
id: security
title: Security & threat model
---

# Security & threat model

Rover treats the web as hostile by default. Every outbound request is policed before a byte leaves the host, every page is fenced as untrusted data before it reaches your agent, and credentials are scrubbed before anything lands in a log. This page documents the boundaries Rover enforces, the typed errors it raises when those boundaries are crossed, and the deliberate tradeoffs in the design.

## SSRF protection

The SSRF policy is governed by `[ssrf] level`. Every outbound URL is checked twice: scheme and host at parse time (`validate_url`), and every resolved address before the connection is opened (`validate_addresses`). The second check catches the address that only appears after DNS resolves.

| Level | Allows |
| --- | --- |
| `strict` (default) | Public IPs only; `http` / `https` only. |
| `loopback` | Strict + `127.0.0.0/8` + `::1`. |
| `project` | Loopback + `file://` URLs descendant of `[ssrf] project_root` after symlink resolution. |
| `lan` | Project + RFC1918 + IPv6 ULAs (`fc00::/7`). |
| `none` | Trust the user. The always-floor (below) is still enforced. |

### Always-floor — blocked at every level

These address classes are rejected at every level, including `none`. There is no configuration that allows them.

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

## DNS rebinding

A public address at resolution time and a private one at connection time is the classic SSRF bypass. Rover closes that window at dial time. A pre-flight in `fetcher::fetch` rejects obviously-bad addresses before TLS is set up, but the enforcement that closes the TOCTOU window lives in `fetcher::dns::SsrfValidatingResolver` — a custom [`reqwest::dns::Resolve`] installed on every `reqwest::Client` Rover builds. The active `SsrfLevel` is carried into the resolver via a `tokio::task_local!` (`SSRF_LEVEL`) populated by the fetch entry point, so the same policy is re-applied to:

- the initial connection,
- every redirect hop reqwest follows internally,
- any new connection reqwest opens after the pre-flight has already returned.

A malicious authoritative DNS server that returns a public address to the pre-flight and a private or loopback address to the dial-time resolver is rejected with a `DialBlocked` error (wrapping the same `SsrfError::Address` variant) before any bytes leave the host. The retry classifier promotes `DialBlocked` to a fatal failure, so retries don't burn against a forbidden destination.

The image-fetch helpers in `extractor::images` (`download_image_bytes`, `partial_fetch_dimensions`, `fetch_content_length`, `download_one`) thread the active `SsrfLevel` down from `images::apply` and police every request like the primary fetch path. A pre-flight `validate_url_for_level` resolves the host and checks each address before connecting — this rejects literal-IP targets such as the cloud-metadata `169.254.169.254`, since reqwest skips the custom resolver for literal IPs — and the send itself is wrapped in the `SSRF_LEVEL` scope so hostname targets are re-validated at dial time. Image downloads and caption-filter probes (dimension and size HEAD plus range requests) are subject to the same policy as the page fetch that referenced them.

## `file://` symlink handling

A symlink is a redirect the filesystem honors, so `file://` access is resolved before it's trusted. When `[ssrf] level` is `project`, `lan`, or `none`, `file://` URLs are allowed. The path is canonicalized via `std::fs::canonicalize` — which resolves every symlink in the path — then checked against the canonicalized `project_root`. A symlink whose target lives outside `project_root` is rejected after resolution with `SsrfError::FileOutsideProjectRoot`. URLs at `strict` and `loopback` are rejected with `SsrfError::FileSchemeNotAllowed` without touching the filesystem.

## Secret redaction

The custom tracing formatter (`RedactingFormatEvent`) scrubs two classes of secrets from every field value before it reaches any log destination:

1. **URL query-string values** whose key name contains any of the following substrings (case-insensitive): `api_key`, `token`, `secret`, `password`.
2. **HTTP `Authorization`-style credentials**:
   - A field literally named `authorization` (case-insensitive) has its **entire value** replaced with `<redacted>`.
   - Any value that embeds a `Bearer <token>` or `Basic <token>` shape — regardless of field name, which catches debug-printed `HeaderMap`s and similar — has the **credential portion** replaced with `<redacted>`.

**Deliberately not redacted:**

- Request and response bodies in HAR files (`[debug] har_path`). HAR is opt-in debug instrumentation for inspecting raw traffic; redacting the bytes you enabled HAR to inspect would defeat the purpose. Protect the HAR file with filesystem permissions and treat it as sensitive material.
- Environment variables. The `api_key_env` config field is a pointer; the resolved value is held in memory and never logged.

## Prompt-injection guard

Fetched web content is untrusted data, not instructions, and Rover enforces that boundary before any page reaches your agent. Every document a content-returning tool hands back is framed by a trusted preamble and sealed inside a per-response nonce delimiter that tells the model the enclosed text is third-party content to read, never obey. The wrapper never depends on catching anything.

For the full layering — the literal and regex pattern detectors, the optional model classifier, the response levels, and the allowlist and override surface — see [Trust & prompt injection](/docs/trust). For the shape of the wrapped `content` string and the per-tool telemetry, see [MCP tools](/docs/mcp-tools).

The limitation is in the detection, not the wrapper. Pattern matching and the optional model classifier enumerate known techniques and score text; both are best-effort, and a novel attack can slip past them. Because the nonce is generated fresh per response and never shown to the page, a malicious document can't predict the tag or forge its own closing fence, and any literal copy of the tag in the body is stripped before the real wrapper goes on. Detection is a bonus. The fence holds by construction.

## Cache poisoning

The cache doesn't validate authenticity — a fresh entry is served until its TTL expires, whatever the upstream has done since. The cache key is `(url, params)`, so the same URL with different upstream content produces different `content_hash` values, and an attacker who controls one upstream can't serve poisoned data to a different URL's consumer. But if an upstream is compromised and serves malicious content while Rover's cache entry is still fresh, that content is served from the cache on subsequent requests until the entry expires.

Operators handling adversarial upstreams should:

1. Lower `[cache] default_ttl` (and possibly `max_ttl`) to bound the staleness window.
2. Use `force_refresh` on the MCP tool calls or `--force-refresh` on the CLI for traffic that must hit origin.
3. Avoid `[cache] override_no_store` for any host that legitimately sends `no-store`.

See [Caching & freshness](/docs/caching) for how TTLs are derived and revalidated.

## Per-process rate-limit scope

The rate limiter and concurrency semaphores live in process memory, not SQLite. Two concurrent `rover mcp` processes each maintain their own buckets; a tight shell loop of `rover fetch` invocations is not paced across process boundaries. This is acceptable for Rover's single-user-local design.

## Robots.txt fail-closed cache window

When a robots.txt fetch returns 5xx or times out, Rover fails closed: it caches a `disallow_all` sentinel for `[robots] failure_ttl` (default `5m`). During that window, all fetches to that host are refused with `robots_fetch_failed` / `robots_disallowed`. The short TTL means recovered servers are picked up quickly. For hosts whose robots endpoint is chronically broken, raise `failure_ttl` or list the host in `[robots] ignore_domains`.

## Headless asset interception and SSRF

A rendered page issues sub-requests Rover doesn't write, and every one of them runs through the same SSRF validator the top-level fetch uses. When the `headless` feature is enabled and a fetch runs in `headless: { mode: "on" }` or triggers via `mode: "auto"`, the browser fetches CSS, fonts, images, and whatever else the page asks for. Each intercepted sub-request URL is checked against the configured `[ssrf] level` before it's allowed out.

Sub-requests that would violate the policy are intercepted via the CDP Fetch domain and fulfilled with an empty `200` response — never aborted. Aborting causes many SPAs to error out on missing CSS, font, or image references; an empty `200` keeps the page rendering.

The HAR recorder records only the top-level navigation. Sub-resources (CSS, JS, images, fonts, beacons) are not in the HAR file. This keeps HAR files navigable and stops sub-resources from masking what Rover actually returned.

A malicious page can't use Rover's headless renderer to scan internal networks. Embedded `<iframe>`, `<img>`, or `fetch()` calls can't reach destinations the SSRF policy forbids. The always-floor address set (link-local, multicast, `0.0.0.0`, broadcast) plus the `block_third_party = true` default cover the common attack paths. Operators who set `[ssrf] level = "none"` opt out of these checks; the WARN line at startup documents that choice. See [JavaScript & dynamic pages](/docs/dynamic-pages) for when the renderer runs at all.

## Local model files

A model file on disk is part of Rover's trust boundary: a tampered weight or tokenizer is loaded with whatever privileges the agent has. The `local-inference` feature downloads model weights from HuggingFace on first use, or ahead of time via `rover model download`.

- Weights are stored under `$HF_HOME/hub/` (default `~/.cache/huggingface/hub/`).
- Rover does not modify or upload model weights.
- The default model (`Qwen/Qwen3.5-0.8B`) is public; no authentication required.
- Pulling gated or private repos requires `HF_TOKEN` in the environment.

### Integrity verification

A per-file integrity manifest guards the model trust boundary. A tampered file aborts the load before the weights ever reach the inference engine.

- **Recording.** After a download (`rover model download`, or a fresh first-use download triggered by inference), Rover hashes every file in the resolved snapshot and writes a sidecar manifest, `<snapshot>/.rover-integrity.toml`, recording the SHA256 of each file and the resolved revision — the snapshot commit sha, which also pins reproducibility after first download even without an explicit revision.
- **Verification.** Before a cached model is loaded, every recorded file is re-hashed and compared. A mismatch aborts the load with a typed `ModelIntegrityFailure { file, expected, actual }` error, surfaced as a clear "model file X has been modified" message. The weights are never handed to the inference engine.
- **Trust-on-first-bootstrap.** A cache populated before this feature existed — or by `mistralrs`' own internal downloader, which Rover does not intercept — has no manifest. On first encounter Rover hashes the files in place, writes the manifest, and emits a `warn`. Rover can't know whether those bytes were already tampered with, only that they won't change afterward.
- **On demand.** `rover model verify [<repo_id>]` re-runs verification for one repo or every cached model; `rover doctor` includes a `local_model_integrity` check when a local-model feature is compiled in.
- **Escape hatch.** `--unsafe-disable-model-integrity-check` (or `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK=1`) skips verification entirely and logs a `warn` at startup. The name is deliberately long. It is a security-sensitive bypass.

Disk usage: see `rover model list`. Models can be removed with `rover model remove <repo_id>`. Weights are not garbage-collected automatically. See [Optional features](/docs/features) for how to compile local inference in.

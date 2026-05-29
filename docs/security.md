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

## DNS rebinding

Rover validates DNS resolution twice per request. A cheap pre-flight in `fetcher::fetch` rejects obviously-bad addresses before TLS is set up. The dial-time enforcement that actually closes the TOCTOU window lives in `fetcher::dns::SsrfValidatingResolver` — a custom [`reqwest::dns::Resolve`] installed on every `reqwest::Client` Rover builds. The active `SsrfLevel` is carried into the resolver via a `tokio::task_local!` (`SSRF_LEVEL`) populated by the fetch entry point, so the same policy is re-applied to:

- the initial connection,
- every redirect hop reqwest follows internally,
- any new connection reqwest opens after the pre-flight has already returned.

A malicious authoritative DNS server that returns a public address to the pre-flight and a private/loopback address to the dial-time resolver is rejected with a `DialBlocked` error (wrapping the same `SsrfError::Address` variant) before any bytes leave the host. The retry classifier promotes `DialBlocked` to a fatal failure so retries do not burn against a forbidden destination.

The image-fetch helpers in `extractor::images` (`download_image_bytes`, `partial_fetch_dimensions`, `fetch_content_length`, `download_one`) thread the active `SsrfLevel` down from `images::apply` and police every request exactly like the primary fetch path: a pre-flight `validate_url_for_level` resolves the host and checks each address before connecting (this is what rejects literal-IP targets such as the cloud-metadata `169.254.169.254`, since reqwest skips the custom resolver for literal IPs), and the send itself is wrapped in the `SSRF_LEVEL` scope so hostname targets are re-validated at dial time. So image downloads and caption-filter probes (dimension/size HEAD+range requests) are subject to the same policy as the page fetch that referenced them.

## `file://` symlink handling

When `[ssrf] level` is `project`, `lan`, or `none`, `file://` URLs are allowed. The path is canonicalized via `std::fs::canonicalize` (which resolves every symlink in the path) before being checked against the canonicalized `project_root`. A symlink whose target lives outside `project_root` is rejected after resolution with `SsrfError::FileOutsideProjectRoot`. URLs at `strict` / `loopback` are rejected with `SsrfError::FileSchemeNotAllowed` without ever touching the filesystem.

## Secret redaction

The custom tracing formatter (`RedactingFormatEvent`) scrubs two classes of secrets from every field value before it hits any log destination:

1. **URL query-string values** whose key name contains any of the following substrings (case-insensitive): `api_key`, `token`, `secret`, `password`.
2. **HTTP `Authorization`-style credentials**:
   - A field literally named `authorization` (case-insensitive) has its **entire value** replaced with `<redacted>`.
   - Any value that embeds a `Bearer <token>` or `Basic <token>` shape (regardless of field name — catches debug-printed `HeaderMap`s and similar) has the **credential portion** replaced with `<redacted>`.

**Deliberately not redacted:**
- Request and response bodies in HAR files (`[debug] har_path`). HAR is opt-in debug instrumentation for inspecting raw traffic; redacting the bytes the user enabled HAR to inspect would defeat the purpose. Protect the HAR file with filesystem permissions and treat it as sensitive material.
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

## Headless asset interception and SSRF (M9)

When the `headless` feature is enabled and a fetch runs in `headless: { mode: "on" }`
or triggers via `mode: "auto"`, the browser issues sub-requests that Rover doesn't
directly control. M9 wires every intercepted sub-request URL through the same
`SsrfLevel` validator the top-level fetch uses.

Sub-requests that would violate the configured `[ssrf] level` are intercepted via
the CDP Fetch domain and fulfilled with an empty 200 response — they are **never
aborted**. Aborting causes many SPAs to error out on missing CSS/font/image
references; an empty 200 keeps the page rendering.

The HAR recorder only records the top-level navigation. Sub-resources (CSS, JS,
images, fonts, beacons) are not in the HAR file. This keeps HAR files navigable
and stops sub-resources from masking what Rover actually returned.

**Threat model:** a malicious page cannot use Rover's headless renderer to scan
internal networks via embedded `<iframe>`, `<img>`, or `fetch()`. The
always-blocked address set (link-local, multicast, `0.0.0.0`, broadcast) plus
the `block_third_party = true` default cover the common attack paths. Operators
who set `[ssrf] level = "none"` opt out of these checks; the WARN line at
startup documents that choice.

## Local model files (M9)

The `local-inference` and `local-vision` features download model weights from
HuggingFace on first use (or ahead-of-time via `rover model download`).

- Weights are stored under `$HF_HOME/hub/` (default `~/.cache/huggingface/hub/`).
- Rover does not modify or upload model weights.
- The default models (`Qwen/Qwen3.5-0.8B`, `Qwen/Qwen2-VL-2B-Instruct`)
  are public; no authentication required.
- Users pulling gated/private repos must set `HF_TOKEN` in the environment.

### Integrity verification

A model file on disk is part of Rover's trust boundary: a tampered weight or
tokenizer is executed/loaded with whatever privileges the agent has. Rover
defends this with a per-file integrity manifest.

- **Recording.** After a download (`rover model download`, or a fresh
  first-use download triggered by inference), Rover hashes every file in the
  resolved snapshot and writes a sidecar manifest,
  `<snapshot>/.rover-integrity.toml`, recording the SHA256 of each file and the
  resolved revision (the snapshot commit sha — this also pins reproducibility
  after first download even without an explicit revision).
- **Verification.** Before a cached model is loaded, every recorded file is
  re-hashed and compared. A mismatch aborts the load with a typed
  `ModelIntegrityFailure { file, expected, actual }` error surfaced as a clear
  "model file X has been modified" message — the weights are never handed to
  the inference engine.
- **Trust-on-first-bootstrap.** A cache populated before this feature existed
  (or by `mistralrs`' own internal downloader, which Rover does not intercept)
  has no manifest. On first encounter Rover hashes the files in place, writes
  the manifest, and emits a `warn`. Rover cannot know whether those bytes were
  already tampered with — only that they will not change afterwards.
- **On demand.** `rover model verify [<repo_id>]` re-runs verification for one
  repo or every cached model; `rover doctor` includes a `local_model_integrity`
  check (only when a local-model feature is compiled in).
- **Escape hatch.** `--unsafe-disable-model-integrity-check` (or
  `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK=1`) skips verification entirely
  and logs a `warn` at startup. The name is deliberately long — it is a
  security-sensitive bypass.

Disk usage: see `rover model list`. Models can be removed with `rover model remove
<repo_id>`. Weights are not garbage-collected automatically.

# Rover M5 — Rate Limiting & Robots Design

**Status:** draft, awaiting review.
**Date:** 2026-05-14.
**Milestone:** M5 — Rate Limiting & Robots (PRD §5.4, §5.6; manifest M5 section).
**Branch:** `m5-rate-limiting` (cut from `main` after M4 PR #5 merged).
**Companions:**
- PRD: [`../prd/2026-05-07-rover-prd.md`](../prd/2026-05-07-rover-prd.md) — §5.4 (rate limiting) + §5.6 (robots.txt).
- Design supplement: [`./2026-05-07-rover-design.md`](./2026-05-07-rover-design.md) — error model, logging, schema migration policy.
- Milestone manifest: [`../milestones/rover-milestones.md`](../milestones/rover-milestones.md) — M5 section (open questions resolved below).
- Previous milestone plan: [`../plans/2026-05-14-rover-m4-extraction.md`](../plans/2026-05-14-rover-m4-extraction.md) — shape reference.

---

## 1. Scope and Goals

Add polite, paced, robots-aware fetching to the M1–M4 pipeline without disturbing existing surfaces.

In scope for M5:

1. **Per-domain token-bucket rate limiting** via the `governor` crate, keyed by host.
2. **Layered concurrency caps**: a global semaphore plus a per-host semaphore.
3. **In-line retry policy** with exponential backoff + jitter, honoring `Retry-After` (seconds + HTTP-date). Max 3 retries per call (4 total attempts). Retries cover 429, 503, other 5xx, and transient network errors (timeouts, connect failures).
4. **Robots.txt fetching and respect** via the `robotxt` crate. Cached in the existing `robots_cache` SQLite table. 4xx → cache allow-all; 5xx/timeout → cache short-TTL disallow-all.
5. **`Crawl-Delay` enforcement** as a floor on the rate limiter, via a per-host `last_request_at` min-interval map.
6. **Three M4 follow-ups bundled** because we are already touching `FetcherError`, the fetcher call sites, and the CLI paths layer:
   - `FetcherError::Extract(ExtractorError)` variant + remap of 3 call sites (`cli/fetch.rs`, `mcp/tools/fetch.rs`, `mcp/tools/get_metadata.rs`) so `readabilityrs` failures surface as `extract_failed` rather than `fetch_failed`.
   - Shared `data_dir()` helper, replacing the four duplicates in `cli/{fetch,cache,mcp}.rs` and `extractor/output.rs::OutputPaths::resolve`.
   - One-line PRD §14 footnote formally deferring `MetadataPreset { Default, All, Minimal }` + `metadata.fields: Option<Vec<String>>` to M8/M9.

Explicitly deferred from M5:

- Deferred-task retry (the "longer retries become deferred tasks" branch of PRD §5.4) — lives in M6 once the task system exists.
- Cross-process rate-limit sharing (two concurrent `rover mcp` processes maintain independent buckets). Documented as a known v1 limitation in `docs/security.md` alongside the SSRF/DNS-rebinding note.
- Cancellation mid-retry-sleep — M6 wires the cancellation flag through; M5's retry sleeps complete naturally (worst case: one extra fetch attempt after a cancel request).
- Robots `Sitemap:` directive consumption (parsed but not acted on).

## 2. Decisions Inherited from Open-Question Round

| # | Question | Decision | Rationale |
|---|----------|----------|-----------|
| 1 | Robots crate | `robotxt` | Newer (last published 2024-03 vs `texting_robots` 2023-03), explicit feature flags, supports Crawl-Delay + universal-match + Sitemap. |
| 2 | Token bucket implementation | `governor` keyed by host (`DashMapStateStore<String>`) | Mature, non-blocking algorithm, async-aware via `until_ready_with_jitter`. Saves us from hand-rolling and testing a clock-driven refill. |
| 3 | Concurrency acquisition order | Per-host permit first, then global | Preserves per-host fairness — no host can starve another by hogging global slots while a host-bound task waits. |
| 4 | Retry error scope | 429 + 503 + other 5xx + transient network (timeout/connect) | Matches PRD §5.4 plus the conventional network-error case the PRD leaves implicit. |
| 5 | Retry placement | New `fetcher::retry` module wrapping `fetch_url_conditional` | Robots fetcher and any future single-shot caller can opt in. Per-host semaphore permit and rate-limit token are acquired once outside the retry loop. |
| 6 | Rate-limiter scope | Per-process | Each `rover` process builds its own state at startup. Multi-process sharing is a v2 concern. |
| 7 | Robots fetch failures | 4xx → allow-all (full TTL); 5xx/timeout → fail-closed (`failure_ttl`, default 5min) | RFC 9309 §2.3.1.4 approach. The 5xx fail-closed window prevents hammering a broken server. |
| 8 | Crawl-Delay enforcement | Separate per-host `last_request_at` min-interval map on top of governor | Clean separation: governor handles rate+burst, the min-interval map handles Crawl-Delay floor. governor doesn't natively support per-key dynamic quotas. |
| 9 | M4 follow-ups bundled | #1 Extract variant, #5 data_dir helper, #2 PRD MetadataPreset note | Natural fit — touching the same error enum, call sites, and CLI paths. The remaining M4 follow-ups (#3 raw_html_text_len, #4 ExtractorError::Metadata rename, #6 Windows signal cfg) stay deferred to a cleanup PR. |

## 3. Architecture

### 3.1 Module Layout

```
src/fetcher/
  rate_limit.rs        # governor wrapper + per-host last_request_at min-interval map
  retry.rs             # backoff loop, Retry-After parsing, retry classification
  robots.rs            # robotxt integration, robots_cache reads/writes, evaluator
  concurrency.rs       # global + per-host Semaphore registry + acquire helper
  mod.rs               # adds new exports + FetcherError variants

src/storage/
  robots.rs            # async API over robots_cache table (lookup, upsert, prune)
  migrations/003_robots_state.sql  # adds `state` column

src/paths.rs           # shared data_dir() helper (M4 follow-up #5)

tests/
  fetcher_rate_limit.rs
  fetcher_retry.rs
  fetcher_robots.rs
  fetcher_full_loop.rs
```

Edited files:

```
src/fetcher/cached.rs           # robots gate + Pacer wiring around fetch_url_conditional
src/fetcher/fetch.rs            # unchanged surface; called from retry::with_retries now
src/config.rs                   # new RateLimitConfig + RobotsConfig sections
src/cli/{fetch,cache,mcp}.rs    # CLI flag plumbing + use shared data_dir()
src/extractor/output.rs         # use shared data_dir()
src/mcp/server.rs               # build Pacer once at startup, share via Arc
src/mcp/tools/fetch.rs          # remap new FetcherError variants to MCP error codes
src/mcp/tools/get_metadata.rs   # same remap (uses fetch_with_cache)
```

### 3.2 The `Pacer` Type

A single ownership point for all pacing state. Built once at `rover mcp` startup (and at each `rover fetch` invocation), shared via `Arc<Pacer>`.

```rust
pub struct Pacer {
    rate_limit: governor::RateLimiter<
        String,
        governor::state::keyed::DashMapStateStore<String>,
        governor::clock::DefaultClock,
    >,
    concurrency_global: Arc<Semaphore>,
    concurrency_per_host: Mutex<HashMap<String, Arc<Semaphore>>>,
    min_interval: Mutex<HashMap<String, Instant>>,
    config: RateLimitConfig,
}

pub struct PacerGuard<'a> {
    pacer: &'a Pacer,
    host: String,
    _per_host_permit: Option<OwnedSemaphorePermit>,  // None for acquire_global_only
    _global_permit: OwnedSemaphorePermit,
    crawl_delay: Option<Duration>,
    updates_min_interval: bool,                       // false for acquire_global_only
}

impl Drop for PacerGuard<'_> {
    fn drop(&mut self) {
        // Record last_request_at = now() for Crawl-Delay floor on the next
        // real fetch. Robots fetches set updates_min_interval = false so they
        // don't artificially pace subsequent content requests.
        if self.updates_min_interval {
            if let Ok(mut map) = self.pacer.min_interval.try_lock() {
                map.insert(self.host.clone(), Instant::now());
            }
        }
        // Permits drop and release automatically.
    }
}
```

`Pacer::acquire(&self, host: &str, crawl_delay: Option<Duration>) -> PacerGuard` async fn handles the four steps in order:

1. Look up (or insert) the per-host `Arc<Semaphore>` from `concurrency_per_host`, clone it, acquire a permit.
2. Acquire a permit from `concurrency_global`.
3. Call `rate_limit.until_ready_with_jitter(host)` to wait for a token.
4. Read `min_interval[host]`; sleep until `last_request_at + crawl_delay` if needed.

`Pacer::acquire_global_only(&self, host: &str) -> PacerGuard` is the variant used for robots-fetches (described in 3.4). It skips steps (1) per-host and (4) min-interval — the per-host gates are chicken-and-egg for the robots fetch itself.

### 3.3 Call Flow on a Single Fetch

```
fetch_with_cache(url):
├─ canonicalize host
├─ robots gate:
│   ├─ if !config.robots.respect → skip
│   ├─ if host in config.robots.ignore_domains → skip
│   ├─ storage::robots::lookup(host) → Option<RobotsEntry>
│   ├─ if missing or expired:
│   │   ├─ robots_fetch(host) using Pacer::acquire_global_only
│   │   └─ upsert RobotsEntry (with state ∈ {parsed, allow_all, disallow_all})
│   ├─ evaluate the entry against (configured UA, url path)
│   └─ if disallowed → return FetcherError::RobotsDisallowed { url, ua }
├─ extract crawl_delay from the robots entry (or None)
├─ cache lookup (unchanged from M2)
├─ if fresh hit → return; no Pacer needed
├─ retry::with_retries(pacer, url, ssrf_level, cond, crawl_delay):
│   ├─ guard = pacer.acquire(host, crawl_delay)
│   ├─ for attempt in 0..=max_retries:
│   │   ├─ fetch_url_conditional(...)
│   │   ├─ classify result → Done | RetryAfter(d) | Backoff(attempt) | Fatal
│   │   ├─ on retry: sleep, continue
│   │   └─ on success / fatal / budget exhausted → break
│   └─ drop guard (records last_request_at, releases semaphores)
├─ extract → cache write (unchanged)
└─ return
```

### 3.4 Robots Fetch's Own Pacing

Per the open-question decision, `robots_fetch` uses `Pacer::acquire_global_only(host)`:

- **Global semaphore:** acquired. Prevents 100 parallel robots fetches across hosts during a cold-start burst.
- **Governor token:** consumed. A misbehaving "fetch 1000 hosts in a loop" caller is still paced.
- **Per-host semaphore:** *not* acquired. Otherwise the first real fetch to a host would queue behind robots fetch, and any retries on the robots fetch would block real fetches.
- **Min-interval (Crawl-Delay):** *not* checked. We don't know the crawl-delay until we've fetched and parsed robots.txt; circular.
- **Retry loop:** applies the same `retry::with_retries` policy. 429/503/5xx/network errors retried up to `max_retries`. Exhaustion → `RobotsFetchFailed`, surfaces upstream and triggers the fail-closed cache write per §3.7.

### 3.5 Retry Layer

`fetcher::retry::with_retries` is the single entry point for HTTP calls that should retry. Signature:

```rust
pub async fn with_retries(
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    crawl_delay: Option<Duration>,
    cfg: &RateLimitConfig,
) -> Result<FetchedPage, FetcherError>
```

Algorithm:

```
guard = pacer.acquire(host, crawl_delay)
attempts = 0
loop:
    result = fetch_url_conditional(client, url, level, cond)
    class = classify(result, cfg)
    match class:
        Done(page)            → return Ok(page)
        Fatal(err)            → return Err(err)
        RetryAfter(d)         → wait = min(d, cfg.retry_after_ceiling)
        Backoff               → wait = jittered(cfg.initial_backoff * 2^attempts, cfg.max_backoff)
    if attempts == cfg.max_retries:
        return Err(FetcherError::RetryExhausted { attempts: attempts + 1, last: Box::new(err) })
    sleep(wait).await
    attempts += 1
```

Classifier:

| Result | Class |
|---|---|
| 2xx | `Done` |
| 304 | `Done` (cached.rs handles freshness extension) |
| 429 with `Retry-After` | `RetryAfter(parsed)` |
| 429 without `Retry-After` | `Backoff` |
| 503 with `Retry-After` | `RetryAfter(parsed)` |
| 503 without `Retry-After` | `Backoff` |
| 500, 502, 504 | `Backoff` |
| Other 4xx (404, 401, 403, etc.) | `Fatal(FetcherError::Status)` |
| Other 5xx (505+) | `Backoff` (treated as transient by RFC 9110 spirit) |
| `reqwest::Error` where `is_timeout()` or `is_connect()` | `Backoff` |
| `reqwest::Error` other | `Fatal` |
| `FetcherError::Ssrf` / `FetcherError::Url` / `FetcherError::Storage` | `Fatal` |

`Retry-After` parsing handles both formats per RFC 9110:

- Integer seconds via `str::parse::<u64>()`.
- HTTP-date via `httpdate::parse_http_date` (already a transitive dep through `reqwest`); compute `parsed - now()` and floor at 0.

A `cfg.retry_after_ceiling: Duration` (default 5 minutes, configurable) caps server-requested wait times to prevent indefinite hangs. The clamp is logged at `warn` level when it bites.

### 3.6 Crawl-Delay Floor

Stored on the `RobotsEntry` returned from the robots gate. Passed through `with_retries` to `Pacer::acquire`. The per-host `min_interval` map is consulted as the final wait in `acquire`:

```
let last = pacer.min_interval.lock().get(host).copied();
if let (Some(last), Some(d)) = (last, crawl_delay) {
    let elapsed = last.elapsed();
    if elapsed < d {
        tokio::time::sleep(d - elapsed).await;
    }
}
```

`PacerGuard::drop` updates `last_request_at = Instant::now()` so the next acquire for the same host respects the delay. Holding the guard across retries means a 429-induced sleep counts toward the next request's min-interval (no double-wait).

### 3.7 Robots Gate

The robots cache uses the existing `robots_cache` table from `001_initial.sql`, with one new column added in `003_robots_state.sql`:

```sql
ALTER TABLE robots_cache ADD COLUMN state TEXT NOT NULL DEFAULT 'parsed';
-- state ∈ {'parsed', 'allow_all', 'disallow_all'}
```

`state` semantics:

- `'parsed'`: `body` contains real robots.txt text; parse on every evaluation (cheap).
- `'allow_all'`: 4xx response (no robots file or explicit allow); `body` is NULL; evaluation always returns Allowed.
- `'disallow_all'`: 5xx/timeout fail-closed; `body` is NULL; evaluation always returns Disallowed for any path.

TTL:

- `'parsed'`: honor `Cache-Control` `max-age` from the robots HTTP response if present, else `config.robots.default_ttl` (24h).
- `'allow_all'`: same as parsed (24h).
- `'disallow_all'`: `config.robots.failure_ttl` (5min default). Short on purpose — once the server recovers, we want to pick up the real robots.txt promptly.

Evaluation against an entry uses `robotxt`'s API, passing the configured `[fetch] user_agent` so UA-specific rules apply.

`config.robots.respect = false` → bypass the gate entirely (no robots fetch, no cache write).
`config.robots.ignore_domains = ["host1.com", ...]` → bypass for matching hosts only.

### 3.8 FetcherError Additions

```rust
#[derive(Debug, Error)]
pub enum FetcherError {
    // existing variants unchanged...

    #[error("retries exhausted after {attempts} attempts; last error: {last}")]
    RetryExhausted {
        attempts: u8,
        last: Box<FetcherError>,
    },

    #[error("rate limited: server requested wait of {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("robots.txt disallows {url} for user-agent {ua}")]
    RobotsDisallowed { url: String, ua: String },

    #[error("robots.txt fetch failed for {host}")]
    RobotsFetchFailed {
        host: String,
        #[source]
        source: Box<FetcherError>,
    },

    #[error("extractor error: {0}")]
    Extract(#[from] crate::extractor::ExtractorError),  // M4 follow-up #1
}
```

`RetryExhausted::last` and `RobotsFetchFailed::source` are boxed to keep the enum size small. `RateLimited` is only emitted if a caller bypasses the retry layer; normal callers see `RetryExhausted` with `last` describing the final 429/503.

### 3.9 Shared `data_dir()` Helper (M4 Follow-up #5)

New `src/paths.rs`:

```rust
pub fn data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("ROVER_DATA_DIR") {
        return PathBuf::from(env);
    }
    dirs::data_local_dir()
        .map(|p| p.join("rover"))
        .unwrap_or_else(|| PathBuf::from("./.rover"))
}
```

The four duplicates in `cli/fetch.rs`, `cli/cache.rs`, `cli/mcp.rs`, and `extractor/output.rs::OutputPaths::resolve` all switch to this helper. There is no `[server]` section in `Config` yet — until M8 adds one, `data_dir()` only consults env + platform default. The helper signature is intentionally `fn data_dir()` (no `&Config` arg) for now; M8 can add a `Config`-aware variant.

## 4. Configuration

### 4.1 New Config Sections

Added to `src/config.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    #[serde(default = "default_rpm_per_domain")]
    pub requests_per_minute_per_domain: u32,   // 60

    #[serde(default = "default_per_domain_concurrency")]
    pub per_domain_concurrency: u32,           // 2

    #[serde(default = "default_global_concurrency")]
    pub global_concurrency: u32,               // 8

    #[serde(default = "default_max_retries")]
    pub max_retries: u8,                       // 3

    #[serde(default = "default_initial_backoff", with = "humantime_serde")]
    pub initial_backoff: Duration,             // 500ms

    #[serde(default = "default_max_backoff", with = "humantime_serde")]
    pub max_backoff: Duration,                 // 30s

    #[serde(default = "default_retry_after_ceiling", with = "humantime_serde")]
    pub retry_after_ceiling: Duration,         // 5min — caps Retry-After honor

    #[serde(default)]
    pub jitter_seed: Option<u64>,              // test-only; None → entropy
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotsConfig {
    #[serde(default = "default_respect")]
    pub respect: bool,                         // true

    #[serde(default)]
    pub ignore_domains: Vec<String>,           // lowercased in validate()

    #[serde(default = "default_robots_ttl", with = "humantime_serde")]
    pub default_ttl: Duration,                 // 24h

    #[serde(default = "default_robots_failure_ttl", with = "humantime_serde")]
    pub failure_ttl: Duration,                 // 5min
}
```

Both hung off `Config` as `#[serde(default)]` sections.

### 4.2 Validation

In `validate(&mut cfg)`:

- All counts > 0 (`requests_per_minute_per_domain`, `per_domain_concurrency`, `global_concurrency`, `max_retries`).
- `max_retries ≤ 10` (sanity ceiling — preserves single-call latency).
- `requests_per_minute_per_domain ≤ 6000` (100 req/s sanity cap — catches typos).
- `initial_backoff ≤ max_backoff`.
- `failure_ttl ≤ default_ttl`.
- `retry_after_ceiling > 0`.
- `ignore_domains` lowercased in-place (mirrors `override_no_store_domains`).

### 4.3 CLI Flags

Added to `rover fetch` and `rover mcp`:

```
--ignore-robots               boolean; overrides [robots] respect=true for this invocation
--rate-limit-rpm <N>          override requests_per_minute_per_domain
--per-host-concurrency <N>    override per_domain_concurrency
--global-concurrency <N>      override global_concurrency
--max-retries <N>             override max_retries
```

Plus the existing `--config <path>` continues to work. Per the design supplement §4.3 (configuration layering), CLI flags win over config file values.

### 4.4 MCP Error Code Mapping

Per design supplement §4.4 — codes are stable snake_case strings.

| FetcherError variant | MCP `error.code` | `details` payload keys |
|---|---|---|
| `Ssrf(_)` | `ssrf_blocked` | `url`, `resolved_ip` (where applicable), `level` |
| `Http(_)` | `fetch_failed` | `url`, `reason` |
| `Url(_)` | `invalid_url` | `url`, `reason` |
| `Dns { .. }` | `dns_failed` | `host`, `reason` |
| `Decode` | `decode_failed` | `url` |
| `Status { .. }` | `http_status` | `url`, `status` |
| `Storage(_)` | `internal_error` | (no public payload; details logged) |
| `RetryExhausted { .. }` | `retry_exhausted` | `attempts`, `last_status`, `last_error` |
| `RateLimited { .. }` | `rate_limited` | `retry_after_secs` |
| `RobotsDisallowed { .. }` | `robots_disallowed` | `url`, `user_agent` |
| `RobotsFetchFailed { .. }` | `robots_fetch_failed` | `host`, `reason` |
| `Extract(_)` | `extract_failed` | `url`, `reason` |

`http_status`, `decode_failed`, and `internal_error` are pre-existing (M3); listed for completeness. `rate_limited` is reserved — normal callers see `retry_exhausted` because the retry layer wraps the underlying 429/503.

## 5. Schema

One new migration:

```sql
-- 003_robots_state.sql
ALTER TABLE robots_cache ADD COLUMN state TEXT NOT NULL DEFAULT 'parsed';
```

Migration applies inline at startup per the design supplement §4.2 protocol; `system.schema_version` advances from 2 → 3.

Default value `'parsed'` matches existing rows' semantics (any pre-M5 robots_cache rows were parsed-from-body entries — though M5 is the first milestone to actually write to this table, so the upgrade path is effectively a no-op for users on first M5 install).

## 6. Test Strategy

### 6.1 Unit Tests

Colocated `#[cfg(test)] mod tests` blocks per module:

- **`fetcher/rate_limit.rs`**: governor token acquisition timing via `tokio::test(start_paused = true)` + `tokio::time::advance`; per-host bucket isolation; Crawl-Delay min-interval enforcement; `Pacer::acquire` step ordering.
- **`fetcher/retry.rs`**: `Retry-After` parsing for seconds-as-int and HTTP-date; exponential backoff schedule (with seeded jitter); classifier branches for every row in the §3.5 table; `RetryExhausted` carries the last error correctly.
- **`fetcher/robots.rs`**: `robotxt` UA-rule evaluation against fixture set (allow, disallow, longest-match, wildcard); Crawl-Delay extraction; `state` column round-trip; cache-TTL honoring (Cache-Control on robots response).
- **`storage/robots.rs`**: lookup-by-host, upsert, TTL expiry filter, prune-expired.
- **`config.rs`**: new validation paths (zero counts, max_retries > 10, failure_ttl > default_ttl, ignore_domains lowercase normalisation, requests_per_minute_per_domain clamp).

### 6.2 Integration Tests

All against `wiremock` (no live network). The test-loopback SSRF level from M3 (behind `--features test-loopback`) is required.

**`tests/fetcher_rate_limit.rs`:**

- 10 concurrent requests to one host at `rpm=60` ⇒ pacing matches 1-per-second within tolerance.
- Two hosts in parallel each get their own bucket.
- Global semaphore caps total in-flight: 50 requests with `global_concurrency=4` ⇒ never more than 4 concurrent server-side handlers.
- Per-host semaphore caps per-host in-flight: 10 requests to one host with `per_host_concurrency=2` ⇒ at most 2 concurrent server-side.
- Crawl-Delay floor: robots advertises `Crawl-Delay: 5`, `rpm=60` (would allow 1s/req) ⇒ effective pacing 5s/req.

**`tests/fetcher_retry.rs`:**

- 429 with `Retry-After: 1` ⇒ retried after ~1s, success on attempt 2.
- 429 with HTTP-date `Retry-After` ⇒ parsed correctly, retried.
- 503 with no `Retry-After` ⇒ exponential backoff, retried up to budget.
- 500 → exponential backoff, retried.
- 502 + 502 + 200 ⇒ succeeds on attempt 3 (2 retries used).
- 500 × 4 ⇒ `RetryExhausted { attempts: 4, last: .. }`.
- Connection-reset (wiremock close-without-response) ⇒ retried, `RetryExhausted` carries `is_connect()` source.
- 404 ⇒ no retry; `FetcherError::Status` surfaced directly.
- `Retry-After: 99999` ⇒ clamped to `retry_after_ceiling`, logged at `warn`.

**`tests/fetcher_robots.rs`:**

- Robots allows `/articles/*`, disallows `/admin/*` for our UA ⇒ allowed paths fetch; disallowed paths return `RobotsDisallowed`.
- Robots 404 ⇒ `state = 'allow_all'`, cached for `default_ttl`.
- Robots 500 ⇒ `state = 'disallow_all'`, cached for `failure_ttl` (5min); next host fetch within window refuses without re-fetching robots.
- `robots.respect = false` ⇒ no robots fetch occurs; no `robots_cache` row.
- `ignore_domains = ["example.com"]` ⇒ skipped for that host only.
- Robots cache hit (fresh row) ⇒ no new HTTP for robots; cache miss triggers one.
- `Crawl-Delay: 2` parsed ⇒ second fetch to host waits ≥ 2s after first.

**`tests/fetcher_full_loop.rs`:**

- Cold cache, 5 URLs across 2 hosts, both with robots allowing all + `Crawl-Delay: 1`, server 500s on first attempt for one URL ⇒ all 5 succeed; total time consistent with crawl-delay + retry timing.
- `FetcherError::Extract` (M4 follow-up #1): force an extraction failure (malformed body served with `200 OK`) ⇒ MCP `fetch` tool returns `code: extract_failed`, not `fetch_failed`.

### 6.3 Test Infrastructure

- `tokio::test(start_paused = true)` with `tokio::time::advance(...)` for timing-sensitive cases.
- `RateLimitConfig::jitter_seed: Option<u64>` lets tests deterministically reproduce backoff schedules.
- Robots fixtures under `tests/fixtures/robots/*.txt`.
- New fixture for the `extract_failed` test: an HTML body crafted to defeat `readabilityrs` (or a binary blob served with `Content-Type: text/html`).

### 6.4 What We Are NOT Testing in M5

- Cross-process rate-limit sharing (per the decision: per-process only).
- Long-running deferred-task retry (M6).
- Robots `Sitemap:` directive consumption (out of scope).
- Cancellation mid-retry-sleep (M6 plumbs cancellation; M5 finishes naturally).

## 7. Crate Dependencies Added

```toml
governor = "0.8"   # confirm latest at planning time
robotxt = "0.6"    # confirm latest at planning time
httpdate = "1"     # likely already transitive via reqwest; confirm and pin direct if needed
dashmap = "6"      # required by governor's DashMapStateStore
```

The exact versions are confirmed in the planning step (`cargo add --dry-run` to see resolution). Crate licenses (MIT / Apache-2.0) are verified against the supply-chain policy.

## 8. Open Items Deferred to Writing-Plans

These are implementation-planning concerns rather than design concerns:

- Whether to expose `cfg.rate_limit.jitter_seed` in the documented config surface or hide it (lean: documented but explicitly labelled "test/debug only").
- Exact governor-version-pinned API call for `until_ready_with_jitter` (the crate has churned API across 0.6 → 0.7 → 0.8; planning will validate).
- Whether the per-host `concurrency_per_host` `HashMap` ever needs eviction. For v1 we expect ≤ a few hundred hosts in a long-running mcp session; the memory cost is trivial. If profiling later shows growth issues, an LRU eviction policy is a one-task follow-up.
- Whether `Pacer` should expose a metrics surface (per-host current bucket level, in-flight counts). Useful for `rover doctor` (M8); not required for M5 functional acceptance.
- Whether to add a one-time `tracing` warning when `rover mcp` startup detects another live `rover mcp` process in `servers` table — to remind the user that rate limiters won't be shared. Cheap; nice-to-have.

## 9. Acceptance Criteria

Per PRD §14 M5: bulk requests against a single domain are paced; robots-disallowed paths are refused.

Concretely for this milestone:

1. `cargo test` green with full unit + integration coverage above.
2. Manual smoke: `rover fetch https://example.org` and `rover fetch https://www.iana.org` show paced behavior in logs at `info` level (one `info` span per fetch with `pacing.wait_ms` field).
3. Manual smoke: `rover fetch https://example.org/robots-disallowed-path` (against a `wiremock`-style local server) refuses with `error.code = robots_disallowed`.
4. `rover doctor` reports `schema_version = 3` after migration.
5. PR description includes the "known v1 limitation: per-process rate limit scope" disclaimer.

## 10. Decision Log

| # | Decision | Date |
|---|---|---|
| 1 | `robotxt` over `texting_robots` | 2026-05-14 |
| 2 | `governor` for token bucket | 2026-05-14 |
| 3 | Per-host permit before global permit | 2026-05-14 |
| 4 | Retry covers 429, 503, other 5xx, transient network; max 3 | 2026-05-14 |
| 5 | Retry lives in new `fetcher::retry` module wrapping `fetch_url_conditional` | 2026-05-14 |
| 6 | Per-process rate-limit scope | 2026-05-14 |
| 7 | Robots 4xx → allow-all (full TTL); 5xx/timeout → disallow-all (5min TTL) | 2026-05-14 |
| 8 | Crawl-Delay enforced via separate `last_request_at` min-interval map | 2026-05-14 |
| 9 | M4 follow-ups #1, #5, #2 bundled into M5; #3, #4, #6 stay deferred | 2026-05-14 |

# Rover Milestone Manifest (M1–M9)

> **Purpose.** This document is the bridge between the PRD (what we're building), the design supplement (how we're building it), and the per-milestone implementation plans. It records, for each milestone: scope, prerequisites, files touched, deferred items, and pre-plan brainstorming hooks. Use it as input when writing the next milestone's plan in a fresh context.

## How to use this doc

When starting a new milestone in a fresh session:

1. **Required reading:** load these files into context, in this order:
   - `docs/superpowers/prd/2026-05-07-rover-prd.md` — product spec, canonical for scope.
   - `docs/superpowers/specs/2026-05-07-rover-design.md` — architectural decisions and PRD corrections.
   - This file — milestone manifest.
   - The most recent completed plan in `docs/superpowers/plans/` — to see what already shipped.

2. **Decide if pre-plan brainstorming is needed.** Each milestone section below has an "Open questions before planning" list. If those aren't resolved, run `/superpowers:brainstorming` before writing the plan. If they are resolved (or the milestone has none), go straight to `/superpowers:writing-plans`.

3. **Run `/superpowers:writing-plans`** with the milestone's "Scope," "Prerequisites," "Files affected," and "Acceptance" sections plus pointers to the canonical docs.

4. **Plan structure to follow:** each milestone should produce a single TDD plan in `docs/superpowers/plans/YYYY-MM-DD-rover-<milestone>-<short-slug>.md` with task granularity matching M1's plan: ~10–15 tasks, each task TDD-shaped (test → fail → implement → pass → commit).

## Cross-cutting threads

Things that **span every milestone** and should be respected by every plan:

- **TDD discipline.** Every behavioral change starts with a failing test. Pure inline unit tests for module-internal logic; `tests/` integration tests for cross-module behavior.
- **Frequent commits.** One commit per task. Conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- **Single binary, single crate.** Per design supplement §5: stay in `crates/rover/` (currently flat at repo root with `src/`); no workspace until a real reusability case appears.
- **`tracing` everywhere.** Add `tracing` spans/events to new code; never `println!` for diagnostics. Stderr in `rover mcp`, stderr for diagnostics in CLI subcommands, stdout for primary output (Markdown, NDJSON, listings).
- **Error model.** Per-module `thiserror` enums. `anyhow` only at CLI binary boundary (`main.rs`, `cli/*` handlers). MCP tools return structured JSON errors with stable `code` strings.
- **Test-loopback feature.** Production SSRF surface in v1 is `Strict` only. Tests that need to hit `wiremock` (which binds to `127.0.0.1`) opt into `SsrfLevel::TestLoopback`, gated on `--features test-loopback`. Carry this convention forward — never expose loopback in the production CLI surface until M8 properly implements the full SSRF level matrix.
- **Schema evolution.** Every milestone that changes the SQLite schema adds a numbered migration file under `crates/rover/migrations/NNN_description.sql`. The storage actor applies them in order on startup. No retroactive edits to released migrations.
- **Frontmatter accretion.** The frontmatter envelope grows over milestones. M1 ships a minimal subset; later milestones add fields. The writer is one function — extending it is non-breaking for downstream consumers (agents read frontmatter, they don't write it).
- **`tokio-rusqlite` actor.** Once the storage actor lands in M2, all DB access goes through it. No direct `rusqlite::Connection` use outside the actor.
- **Multi-instance + PID-tagged heartbeats.** From M3 onward, every running `rover mcp` instance maintains its own row in `servers` and stamps tasks with `owner_pid`. CLI liveness checks "is any server alive."
- **`genai` for cloud LLMs.** Every cloud summarization/captioning call goes through the `genai` crate. Keys via env vars per `genai` conventions. Custom OpenAI-compatible endpoints (LM Studio, Ollama, vLLM) via `ServiceTargetResolver`.

---

## M1 — Single-URL Fetch Path

**Status:** plan written (`docs/superpowers/plans/2026-05-07-rover-m1-fetch-path.md`).

**Scope.** `rover fetch <url>` end-to-end: fetch the URL, detect charset, decode to UTF-8, extract main content via `readabilityrs`, wrap with a YAML frontmatter envelope (M1 subset), print Markdown to stdout. No caching, no MCP server, no batching.

**Prerequisites.** None — this is the foundation milestone.

**Files / modules introduced.**
```
Cargo.toml, src/main.rs, src/lib.rs
src/telemetry.rs                          # tracing init
src/error.rs                              # crate-wide Error/Result
src/config.rs                             # TOML loader (M1 subset: fetch.user_agent, fetch.timeout)
src/cli/mod.rs, src/cli/fetch.rs
src/fetcher/mod.rs, ssrf.rs, charset.rs, client.rs, canonical.rs, fetch.rs
src/extractor/mod.rs, pipeline.rs, frontmatter.rs
tests/cli_fetch.rs, tests/fetcher_integration.rs
```

**Acceptance (PRD §14, M1).** Can fetch `https://example.com` and a few real article URLs, produces clean Markdown with frontmatter.

**Deferred from M1 (do not pull forward).**
- Caching, conditional GETs, `force_refresh`. → M2.
- MCP server mode. → M3.
- Real tokenizers. M1 ships chars/4 heuristic behind `extractor::frontmatter::estimate_tokens`. → M3.
- Metadata extraction (JSON-LD, OG, Twitter Card, microdata). M1 only surfaces what `readabilityrs` already returns (title, byline, lang, excerpt, site_name, published_time, image). → M4.
- Tables/images transformation modes. → M4.
- Rate limiting / robots.txt. → M5.
- Long-running tasks. → M6.
- Summarization. → M7.
- SSRF levels beyond `Strict`. → M8.
- HAR mode, `rover doctor`, `rover config show/set`. → M8.
- Headless / local-inference / VLM features. → M9.

**Open questions before planning.** None — resolved during brainstorming.

---

## M2 — Caching & Storage

**Status:** plan executed; PR open against `main`.

**Scope.** SQLite cache for fetched pages; TTL logic that respects HTTP semantics; conditional GETs (`ETag`, `Last-Modified`); `force_refresh` flag on the fetcher; `rover cache list/get/purge/stats` subcommands.

**Prerequisites.** M1.

**Key design references.**
- PRD §8 (caching), §3.2 (storage), §3.3 (inter-process coordination — note: revised by design supplement §2.3).
- Design supplement §2.1 (tokio-rusqlite actor), §2.3 (multi-instance + servers table), §3.4 (heartbeat → servers table), §4.2 (schema migrations).

**Files / modules introduced.**
```
src/storage/mod.rs                        # tokio-rusqlite actor + public API surface
src/storage/migrations/
  001_initial.sql                         # pages, robots_cache, system tables
src/storage/pages.rs                      # async API for pages table
src/storage/system.rs                     # schema_version + scalar metadata
src/cli/cache.rs                          # rover cache list/get/purge/stats
tests/storage_integration.rs
tests/cache_lifecycle.rs                  # TTL, force_refresh, conditional GETs
```

Migration `001_initial.sql` should include schemas for `pages`, `robots_cache`, and `system` from PRD §8.1. **Skip `tasks`, `task_events`, and `summary_cache`** — those land with M6 and M7 in their own migrations. Include `servers` from design supplement §2.3 only if M3 lands first; if M2 ships standalone, defer `servers` to M3's migration.

**Cache-control parsing.** Robust `Cache-Control` header parsing (max-age, s-maxage, no-store, no-cache, must-revalidate). Probably `headers` crate or hand-rolled — small parser, hand-roll is fine.

**TTL precedence (PRD §8.2 + design §3.6 clarification).**
1. `Cache-Control: no-store` → don't cache, unless `override_no_store` (global or per-domain) is true; in that case, floor at `min_ttl`.
2. `max-age` / `s-maxage` from `Cache-Control`.
3. `Expires` header.
4. `cache.default_ttl`.
5. Cap at `cache.max_ttl`.

**Stale-while-revalidate.** When a request hits expired cache: return stale immediately, return `revalidation_task_id` envelope per design supplement §3.3. The actual revalidation task scheduling depends on the task system landing in M6 — for M2, return only the stale content with `cache_status: "stale"` and defer the task scheduling. **Decision needed in M2 plan brainstorming:** ship stale-served-without-revalidation in M2, or wait for M6 and add SWR then?

**Acceptance (PRD §14, M2).** Repeated fetches hit cache; purging works; expired entries re-fetch with conditional headers.

**Deferred from M2.**
- `tasks`, `task_events`, `servers` schemas (unless M3 ships first).
- `summary_cache` table → M7.
- `force_refresh` MCP arg → M3 (M2 only exposes the flag through `rover fetch --force-refresh`).
- Pretty-printed `cache list` formatting beyond a basic table.

**Open questions before planning.**
1. **`servers` table timing.** Land in M2 (forward-looking) or M3 (when there's a process to track)? Recommendation: M3 — there's no live writer in M2 since CLI subcommands are short-lived.
2. **Pages: opportunistic write from `rover fetch`?** Per design §2.3 instance model, multiple writers are allowed. So `rover fetch` should be allowed to write to the cache. WAL + busy_timeout on the actor handles contention. Confirm in plan.
3. **SWR scheduling in M2 vs M6.** See above.
4. **Cache-Control parsing crate vs hand-roll.** Plan should pick.
5. **`cache purge` glob semantics.** PRD §8.5 says "literal URL or glob (e.g., `https://docs.example.com/*`)." Decide: shell glob or SQL `LIKE`? Probably translate `*` → SQL `%` and `?` → `_`, after escaping the rest.

---

## M3 — MCP Server Mode

**Status:** plan executed; PR open against `main`.

**Plan:** [`docs/superpowers/plans/2026-05-13-rover-m3-mcp-server.md`](../plans/2026-05-13-rover-m3-mcp-server.md).
**Spec:** [`docs/superpowers/specs/2026-05-13-rover-m3-mcp-design.md`](../specs/2026-05-13-rover-m3-mcp-design.md).

**Scope.** `rover mcp` subcommand starts the MCP server over stdio. Implement `fetch` and `count_tokens` MCP tools. Real tokenizers replace the M1 chars/4 heuristic. Multi-instance support with `servers` table and PID-tagged ownership.

**Prerequisites.** M1, M2.

**Key design references.**
- PRD §4.1 (`fetch` tool), §4.5 (`count_tokens` tool), §10 (token counting).
- Design supplement §2.2 (stdio-only transport), §2.3 (multi-instance + servers + owner_pid), §3.4 (heartbeat → servers).

**Files / modules introduced.**
```
src/mcp/mod.rs, server.rs, envelope.rs    # rmcp wiring, transport, shared types
src/mcp/tools/mod.rs, fetch.rs, count_tokens.rs
src/storage/migrations/002_servers.sql    # servers table, owner_pid on pages? (no, just servers)
src/storage/servers.rs
src/tokenizer/mod.rs, openai.rs, hf.rs    # tiktoken-rs + tokenizers wrappers, in-memory cache
src/cli/mcp.rs                            # rover mcp subcommand body
tests/mcp_smoke.rs                        # spawn rover mcp, speak stdio JSON-RPC
tests/tokenizer_integration.rs
```

**Tokenizer registry.** A `Tokenizer` enum with variants per supported family. Lazy-load tokenizers on first use; cache loaded ones in a `OnceCell` map. PRD §10 lists `cl100k`, `o200k`, `claude`, `llama3`, `qwen3` — wire all five, document the actual tokenizer IDs each maps to.

The M1 `extractor::frontmatter::estimate_tokens(text: &str) -> usize` function gets a `tokenizer: Tokenizer` parameter (or a thread-local default). Pick a non-breaking refactor: leave `estimate_tokens` as a default-tokenizer wrapper around a new `count_tokens(text: &str, t: Tokenizer) -> usize`. Or take the breaking change and update all call sites — there's only one in M2 cache size accounting, so breaking is fine.

**Multi-instance model (design §2.3).**
- Server startup: insert/upsert own row in `servers` (PID, started_at, last_heartbeat, version).
- Heartbeat task: every ~5s, `UPDATE servers SET last_heartbeat = ? WHERE pid = ?`.
- Startup reap: delete `servers` rows where `last_heartbeat < now() - 60s`. For each reaped PID, mark its `running` tasks (orphaned) — but tasks land in M6, so the reap-then-resume logic is mostly forward-prep here.
- Clean shutdown: SIGTERM/SIGINT handler deletes own `servers` row.

**rmcp transport.** stdio. Logs to stderr (PRD/design conflict resolution: stdio claims stdout, so all `tracing` output must go to stderr from `rover mcp`). The `telemetry::init` from M1 already enforces stderr — confirm.

**`fetch` tool.** Wires the M1 fetch + extract + frontmatter pipeline behind the MCP tool surface. Honors `force_refresh`, `count_only`, `tokenizer` args. `headless`, `tables`, `images`, `metadata`, `summarize`, `max_tokens` — see "Deferred" below.

**Acceptance (PRD §14, M3).** Claude Code can connect to Rover and successfully fetch URLs.

**Deferred from M3.**
- `headless` arg behavior (still always parses to `"auto"` but auto-detect doesn't kick in until M9 ships the headless feature).
- `tables`, `images`, `metadata` arg structures land in M4. M3 should accept the args (so the MCP schema is stable) but they're no-ops until M4.
- `summarize` arg, `max_tokens` auto-summarize → M7.
- `batch_fetch`, `summarize`, `get_metadata` tools → M4 (`get_metadata`), M6 (`batch_fetch`), M7 (`summarize`).
- Stale-while-revalidate task scheduling → M6 if not already in M2.

**Open questions before planning.** *(All resolved during pre-plan brainstorming. Decisions are recorded in the M3 design spec.)*

1. **rmcp API shape.** Pin a concrete `rmcp` version. Confirm the tool registration and transport pattern (likely `rmcp::serve_stdio` or similar). Look at the latest rmcp examples at planning time.
2. **`fetch` tool arg shape.** Define the MCP-side argument struct. PRD §4.1 lists arguments — encode them as a serde struct. Args for unimplemented features (`tables`, `images`, `summarize`, etc.) should be accepted but ignored, with a one-line `tracing::debug` noting the no-op.
3. **`force_refresh` plumbing.** Pass through from MCP arg → fetcher.
4. **Token counter call sites.** Confirm: frontmatter writer, MCP `count_tokens` tool, `max_tokens` budget check (M3 already? or M7?). PRD §4.1 has `max_tokens` on `fetch`; without summarization it can only error or truncate. Decide: error-with-suggestion in M3 ("max_tokens exceeded; call summarize"), or defer the arg's behavior to M7?
5. **Process model for tests.** `tests/mcp_smoke.rs` spawns `rover mcp` and speaks JSON-RPC over stdin/stdout. Use `assert_cmd` or roll a simple subprocess + JSON-RPC client. Pick in plan.

---

## M4 — Metadata, Tables, Images, Links

**Scope.** Structured metadata extraction from JSON-LD, Open Graph, Twitter Cards, microdata. Metadata presets and field overrides. Tables transformation modes (Embed / Sample / Summarize / CsvFile / Drop). Images transformation modes (Keep / AltTextOnly / CaptionVlm / Download / Drop) — VLM mode is wired but the actual implementation is gated on the `vlm` feature (M9). Relative-to-absolute link rewriting. `get_metadata` MCP tool.

**Prerequisites.** M3.

**Key design references.**
- PRD §6 (extraction), §6.3–6.6 in particular.
- Design supplement §2.8 (output paths: `$output_dir/{tables,images}/<host>/<sha8>.{csv,ext}`, absolute paths in frontmatter).

**Files / modules introduced.**
```
src/extractor/links.rs                    # relative → absolute, srcset, base href
src/extractor/tables.rs                   # transform modes
src/extractor/images.rs                   # transform modes; VLM call site stub
src/extractor/metadata.rs                 # JSON-LD, OG, Twitter, microdata
src/mcp/tools/get_metadata.rs
tests/extractor_metadata.rs
tests/extractor_tables.rs
tests/extractor_images.rs
tests/extractor_links.rs
```

**Frontmatter additions in M4.** `language`, `extraction_quality`, `schema_types`, all metadata preset fields (`description`, `author`, `published`, `modified`, `image`, `og:type`, `canonical`), `tables_transformed`. Update the M1 frontmatter writer to accept the larger meta struct.

**Output paths (design §2.8).** Tables CsvFile → `$output_dir/tables/<host>/<sha8>.csv`. Images Download → `$output_dir/images/<host>/<sha8>.<ext>`. `<sha8>` is the first 8 hex of `sha256(absolute_url)`. Auto-create dirs on first write. Frontmatter records absolute paths.

**Tables Summarize mode.** Calls into the summarizer — but summarization lands in M7. **Decision needed:** ship Summarize mode in M4 with a stub that errors "summarization backend not yet wired" (and only enable it in M7 when summarization lands), or defer Summarize mode to M7 and ship Embed/Sample/CsvFile/Drop in M4? Recommendation: defer Summarize mode to M7 — keeps M4 self-contained.

**Images CaptionVlm mode.** Same dependency on the `vlm` feature (M9). Either ship the call site behind `#[cfg(feature = "vlm")]` and error at runtime if invoked without the feature, or defer. Recommendation: ship the call site stubbed; M9 fills it in.

**Acceptance (PRD §14, M4).** Complex pages produce well-structured frontmatter; large tables don't blow token budgets; all links in output are absolute.

**Deferred from M4.**
- Tables Summarize mode → M7 (or stub in M4 with clear error).
- Images CaptionVlm mode body → M9.

**Open questions before planning.**
1. **Microdata crate.** PRD §6.6 suggests `microdata`. Audit the crate's freshness; if stale, fall back to manual `scraper`-based extraction.
2. **`extraction_quality` heuristic.** Define a concrete formula. PRD §6.2 shows a value `0.87` but doesn't define it. Suggestion: function of `(extracted_text_length / raw_html_length)`, capped to [0, 1], with bonuses for presence of structured metadata. Decide in plan brainstorming.
3. **JSON-LD walker depth.** JSON-LD can be deeply nested with `@graph`. Decide on a walker strategy (recurse with depth limit, or flatten).
4. **`<base href>` handling.** Confirm: link rewriting takes `<base href>` if present, else uses the final URL after redirects. PRD §6.5 spells this out — implement it in `extractor/links.rs`.
5. **Image src rewriting timing.** Should links/images be rewritten before or after `readabilityrs`? `readabilityrs` itself handles some link normalization (it accepts a base URL). Confirm what `readabilityrs` does and where our post-processing fits.
6. **Sample strategy: HeadTail vs Stratified vs RandomSeed.** PRD §6.3 lists three; ship HeadTail in M4, mark the others as future work? Or ship all three? Recommendation: ship HeadTail only in M4, defer Stratified/RandomSeed (no clear consumer demand).

---

## M5 — Rate Limiting & Robots

**Scope.** Per-domain token bucket rate limiter. `429`/`Retry-After` honoring (seconds and HTTP-date formats). `503`/`Retry-After` honoring. Robots.txt fetching, parsing, caching, and respect (configurable to ignore globally or per-domain). `Crawl-Delay` integration with the rate limiter.

**Prerequisites.** M2 (for `robots_cache` table — already added in M2's `001_initial.sql`).

**Key design references.**
- PRD §5.4 (rate limiting), §5.6 (robots.txt).
- Design supplement: no specific decisions; PRD wording stands.

**Files / modules introduced.**
```
src/fetcher/rate_limit.rs                 # per-domain token buckets, semaphores
src/fetcher/retry.rs                      # backoff, Retry-After parsing
src/fetcher/robots.rs                     # fetching, caching, evaluation
tests/fetcher_rate_limit.rs
tests/fetcher_robots.rs
```

**Rate limiter.** A `HashMap<String, TokenBucket>` keyed by host, behind an async-aware mutex. Buckets refill at `requests_per_minute_per_domain / 60` per second. `Crawl-Delay` from robots.txt is a floor on the inter-request interval.

**Retry-After parsing.** Two formats per RFC 9110: integer seconds, or HTTP-date. Use `httpdate` crate or manual parse with `jiff`.

**Robots cache.** `robots_cache` table from M2's `001_initial.sql`. TTL: honor `Cache-Control` from the robots.txt response if present, else 24h (PRD §5.6).

**User-Agent.** Ensure the configured UA from `[fetch] user_agent` is the one used to evaluate User-Agent-specific rules in robots.txt.

**Acceptance (PRD §14, M5).** Bulk requests against a single domain are paced; robots-disallowed paths are refused.

**Deferred from M5.** Nothing structural — M5 is mostly self-contained.

**Open questions before planning.**
1. **`robotxt` vs `texting_robots`.** Audit both at planning time, pick the more recently maintained one. Per PRD §5.6, do not hand-roll.
2. **Per-domain concurrency vs global concurrency.** PRD §4.2 has both (`concurrency` and `per_domain_concurrency`). Implement two layered semaphores: global `tokio::sync::Semaphore` + per-host semaphores.
3. **5xx retry policy.** PRD §5.4 says "exponential backoff with jitter, max 3 retries within a single sync call (longer retries become deferred tasks)." Deferred-task scheduling depends on M6. Decide: implement up-to-3 in-line retries in M5, defer "longer retries become deferred tasks" to M6.
4. **Rate limiter scope: per-process or per-server-instance?** With multi-instance servers, two concurrent `rover mcp` processes each have their own bucket. Acceptable for v1 — document this in the plan.

---

## M6 — Long-Running Tasks & Batching

**Scope.** Task system: `tasks` and `task_events` tables, heartbeat-claim model, cancellation, NDJSON event streaming. `batch_fetch` MCP tool. `rover batch <uuid>` and `rover task <uuid>` CLI subcommands with both snapshot and `--monitor` (NDJSON streaming) modes. Cancellation flag plumbing. Stale-while-revalidate revalidation as a task (if not already in M2).

**Prerequisites.** M3 (for the `servers` table and PID-tagged ownership), M5 (rate limiting integrates into batch concurrency).

**Key design references.**
- PRD §3.3 (task resumption), §4.2 (batch_fetch tool), §9 (long-running task pattern).
- Design supplement §2.3 (multi-instance + owner_pid), §3.1 (task ID format: bare UUIDv7), §3.2 (kind naming: `summarize`), §3.4 (servers table).

**Files / modules introduced.**
```
src/storage/migrations/003_tasks.sql      # tasks, task_events tables, owner_pid column
src/storage/tasks.rs                      # async API for tasks + events
src/storage/events.rs
src/tasks/mod.rs                          # task scheduler / claim loop
src/tasks/batch_fetch.rs                  # batch worker
src/tasks/revalidate.rs                   # SWR worker
src/tasks/retry.rs                        # deferred retry worker
src/mcp/tools/batch_fetch.rs
src/cli/batch.rs                          # rover batch <id> [--monitor]
src/cli/task.rs                           # rover task <id> [--monitor] [--cancel]
tests/tasks_lifecycle.rs
tests/cli_batch_monitor.rs
```

**Task ID format (design §3.1).** UUIDv7 strings, no kind prefix.
**Task kind naming (design §3.2).** `batch_fetch | retry | revalidate | summarize`.
**Owner PID model (design §2.3).** New tasks insert with own PID. Live servers periodically scan for `owner_pid NOT IN (SELECT pid FROM servers)` orphans. Resumable kinds (batch_fetch, retry, revalidate): CAS-claim and resume from persisted progress. Non-resumable (summarize): mark `failed` with `error='owner_died'`.

**NDJSON streaming (PRD §9.2).** The `--monitor` loop polls `task_events WHERE task_id = ? AND id > last_seen_id`, sleeps 200ms, repeats until `tasks.status` is terminal. Trap SIGINT for clean exit. Write every event as one JSON line to stdout.

**Cancellation (PRD §9.3).** `rover task <id> --cancel` sets `tasks.cancellation_requested = 1`. Workers check this flag at safe points (between URLs in a batch, between retries, at chunk boundaries). Cooperative — no hard kills.

**Liveness warning (design §2.3 + PRD §9.4).** CLI checks `MAX(last_heartbeat) FROM servers`. If older than 30s and the inspected task is `running`, print the warning.

**Stale-while-revalidate.** If M2 punted the task scheduling, implement here. Stale fetch returns `revalidation_task_id` envelope (design §3.3).

**Acceptance (PRD §14, M6).** A batch of 20 URLs returns a task ID immediately, progress is observable via monitor or poll, completes correctly, can be cancelled mid-flight.

**Deferred from M6.**
- `summarize` task kind body — the task scheduler accepts `summarize` jobs in M6 (so the schema is final), but the worker just errors `summarization not yet implemented`. Real impl in M7.

**Open questions before planning.**
1. **Per-task owner PID claim mechanism.** CAS via `UPDATE WHERE owner_pid = $orphan AND status = 'running'`. Confirm transaction boundary and idempotency.
2. **Batch progress tracking.** Per-URL state stored in `task_events` (item_started, item_done, item_failed) plus a denormalized `progress_json` in `tasks.params_json` for snapshot reads? Or compute snapshot from events? Computing from events is simpler; denormalize only if profiling demands.
3. **NDJSON output schema.** PRD §9.2 sketches `batch_start`, `item_done`, `item_failed`, `final` event kinds. Pin the full schema (with `ts`, `kind`, payloads) in the M6 plan.
4. **Monitor poll interval.** PRD says 200ms. Confirm; consider exponential backoff during long quiet periods.
5. **Task timeout / max duration.** Should batches have an upper-bound runtime? PRD doesn't say. Default to no timeout in v1; document.
6. **CLI snapshot output format.** PRD §9.2 shows a human-friendly format and a `--format=ndjson` mode. Both ship.

---

## M7 — Summarization

**Scope.** Extractive (TextRank) summarizer (no model required, offline). `genai` integration for cloud backends. `SummarizerBackend` trait with config-driven instantiation. `summarize` MCP tool with full compaction steering parameters. `summary_cache` table. Summarize-on-cache-miss behavior (design §2.7: fetch with defaults, then summarize). Tables Summarize mode wired up if it was deferred from M4.

**Prerequisites.** M3 (MCP), M4 (extracted markdown), M6 (task system for summarize jobs if any are deferred).

**Key design references.**
- PRD §4.3 (`summarize` tool), §7 (summarization), §8.4 (summary_cache).
- Design supplement §2.6 (`[summarization] default_backend`), §2.7 (cache miss → fetch with defaults), §3.5 (params hash includes backend name).

**Files / modules introduced.**
```
src/storage/migrations/004_summary_cache.sql
src/storage/summaries.rs
src/summarizer/mod.rs                     # SummarizerBackend trait, registry
src/summarizer/extractive.rs              # TextRank
src/summarizer/cloud.rs                   # genai wrapper
src/summarizer/prompts.rs                 # abstractive prompt templates
src/mcp/tools/summarize.rs
tests/summarizer_extractive.rs
tests/summarizer_cloud.rs                 # against a mocked OpenAI-compatible endpoint
tests/summary_cache_lifecycle.rs
```

**Config schema additions (design §2.6).**
```toml
[summarization]
default_backend = "default"

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"

# ... etc
```

**Extractive TextRank (PRD §7.2).** ~250 lines. Sentence segmentation via `unicode-segmentation`. TF-IDF, cosine similarity, PageRank, top-k by original position. Deterministic, fast, offline.

**Backend trait (PRD §7.1).**
```rust
#[async_trait]
trait SummarizerBackend: Send + Sync {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError>;
    fn name(&self) -> &str;
}
```

**Cloud backend.** Wrap `genai::Client`. Use `ServiceTargetResolver` for OpenAI-compatible endpoints (LM Studio, Ollama, vLLM). API keys via env vars per `genai` conventions.

**Summary cache (design §3.5).** `params_hash = sha256(backend_name || model || mode || target_tokens || focus || preserve_sorted || style)`. Sort keys before hashing.

**Cache-miss summarize (design §2.7).** If the URL isn't in `pages`, internally call the fetch path with default options, then summarize. Document this in the tool docstring.

**Compaction steering (PRD §7.5).** `target_tokens`, `mode` (Extractive/Abstractive/Headlines), `focus`, `preserve` ([Code|Tables|Quotes|Lists]), `style` (Bullet/Prose/Executive), `backend` (named).

**Auto-summarize on `max_tokens`.** PRD §4.1 has a `max_tokens` arg on `fetch` that auto-summarizes if exceeded. If that was deferred from M3, implement here using the default backend.

**Acceptance (PRD §14, M7).** All summarization modes work against at least one cloud provider; extractive mode works offline; summary cache avoids redundant LLM calls.

**Deferred from M7.**
- Local inference backend (`LocalMistralRs`) → M9.

**Open questions before planning.**
1. **Sentence segmenter.** PRD recommends `unicode-segmentation` or `icu_segmenter`. Pick `unicode-segmentation` for binary size unless multilingual quality demands push to ICU.
2. **TF-IDF vocabulary scope.** Over the document being summarized only (within-doc IDF), or no IDF (use TF-only with cosine on raw counts)? Implementer's call; document the choice.
3. **PageRank tolerance / iterations.** PRD §7.2 suggests 20 iterations or convergence at 1e-4 with damping 0.85. Confirm.
4. **Abstractive prompt wording.** PRD §7.5 has a sample template. Refine and pin in `summarizer::prompts`.
5. **Streaming responses.** `genai` supports streaming. M7 could either collect into a `String` (simpler) or stream into the MCP response. MCP doesn't natively stream tool responses. Recommendation: collect, no streaming in v1.
6. **Headlines mode definition.** PRD lists `Headlines` as a mode but doesn't specify behavior. Decide: bullet-list of section titles? top-k sentence headlines? Pin in plan.
7. **Tables Summarize mode.** Was it deferred from M4? If so, wire up here using the default summarization backend.

---

## M8 — SSRF Levels, Diagnostics, Polish

**Scope.** Full SSRF level matrix (Strict, Loopback, Project, LAN, None). HAR debug mode. `rover doctor` health check. `rover config show` and `rover config set`. Logging/tracing polish across the codebase. Secret-redaction in URL logging.

**Prerequisites.** M5 (so SSRF integrates with the production fetcher), M2/M3/M6 (so `rover doctor` can verify all the moving parts).

**Key design references.**
- PRD §5.5 (SSRF levels), §11 (debug & diagnostics), §12 (config), §16 (security).
- Design supplement §2.4 (DNS rebinding deferred to v2 — document in `docs/security.md`).

**Files / modules introduced.**
```
src/fetcher/ssrf.rs                       # extended: Loopback, Project, Lan, None levels
src/fetcher/har.rs                        # HAR recorder
src/cli/doctor.rs, src/doctor/mod.rs
src/cli/config.rs                         # show/set
docs/configuration.md
docs/security.md
docs/cli.md
docs/mcp-tools.md
docs/backends.md
tests/ssrf_levels.rs
tests/har_output.rs
tests/cli_doctor.rs
tests/cli_config.rs
```

**SSRF level extensions.**
- `Loopback`: Strict + 127.0.0.0/8 + ::1.
- `Project`: Loopback + `file://` URLs descendant of `[ssrf] project_root` after symlink resolution.
- `LAN`: Project + RFC1918 + IPv6 ULAs.
- `None`: trust the user; log a warning at startup; document risks.

The M1 `TestLoopback` variant becomes redundant once `Loopback` ships. **Decide:** either (a) keep `TestLoopback` separate and only-test, or (b) retire it and have tests use `Loopback` directly. Recommendation: (b) — once `Loopback` is a real production level, having two variants for "allow loopback" is confusing.

**File:// at Project level.** Canonicalize the path, resolve symlinks, then verify it's still a descendant of `project_root`. Reject if not. PRD §5.5 spells out the requirement.

**HAR mode (PRD §11.1).** Configurable `[debug] har_path`. When set, record every fetch as a HAR entry using the `har` crate. Configurable body size cap. Output viewable in Chrome DevTools.

**`rover doctor` (PRD §11.2).** Run a battery of checks; exit 0 if all pass, 1 otherwise. NDJSON mode for scripting (`--format=ndjson`). Checks:
- SQLite DB exists, writable, schema version current.
- WAL mode enabled.
- Network reachability (try `https://example.com`).
- Configured backends authenticate (try a trivial completion against each cloud backend).
- Output dir writable.
- Feature-gated checks for `local-inference`, `vlm`, `headless` (M9).

**`rover config show/set` (PRD §12).** `show` prints the merged effective config with provenance comments. `set` mutates the file with `toml_edit` (preserves comments).

**Secret redaction.** Logging redacts URL query parameters matching `api_key`, `token`, `secret`, `password` (case-insensitive substring).

**Documentation deliverables (PRD §17).** All five docs (`configuration.md`, `mcp-tools.md`, `cli.md`, `security.md`, `backends.md`) authored alongside this milestone. `docs/security.md` documents the v1 DNS-rebinding limitation (design §2.4) and the cache-poisoning consideration (PRD §16).

**Acceptance (PRD §14, M8).** `rover doctor` passes on a clean install; HAR files import cleanly into Chrome DevTools.

**Deferred from M8.**
- DNS-rebinding-resistant fetching → v2 (design §2.4). Document the limitation; note `reqwest::ClientBuilder::resolve` as the v2 implementation path.
- Headless / local-inference / VLM doctor checks land in M9.

**Open questions before planning.**
1. **`har` crate version.** Confirm latest stable at planning time.
2. **Config layering UX.** `rover config show` — show defaults + file-overrides + env-overrides + CLI-flag-overrides separately, or merged? PRD §12 says merged with provenance comments. Pin format in plan.
3. **`config set` validation.** Should it validate the new value parses correctly before writing? Yes — round-trip through the typed schema.

---

## M9 — Feature-Flagged Extras

**Scope.** Three independent Cargo features: `local-inference` (mistral.rs + Qwen 3.5 0.8B), `headless` (chromiumoxide for SPA support), `vlm` (SmolVLM image captioning via mistral.rs). Each ships as opt-in; default `cargo install rover` produces a lean binary. `rover doctor` extended to verify feature-flagged dependencies when enabled.

**Prerequisites.** M7 (`SummarizerBackend` trait for `LocalMistralRs`), M4 (image transformation pipeline for VLM call site), M3 (`fetch` MCP tool for headless arg).

**Key design references.**
- PRD §5.7 (headless), §7.3 (local inference), §7.4 (image captioning), §13 (deps).

**Files / modules introduced.**
```
src/fetcher/headless.rs                   # behind `headless` feature
src/summarizer/local.rs                   # behind `local-inference` feature
src/vlm/mod.rs                            # behind `vlm` feature
docs/features.md
tests/headless_smoke.rs
tests/local_inference_smoke.rs
tests/vlm_smoke.rs
```

**Cargo features.**
```toml
[features]
default = []
local-inference = ["dep:mistralrs"]
vlm = ["dep:mistralrs"]
headless = ["dep:chromiumoxide"]
test-loopback = []                        # carried forward from M1
```

**`local-inference`.** `LocalMistralRs` backend implementing `SummarizerBackend`. Default model: Qwen 3.5 0.8B. Accept `--model <hf_repo_id>` to swap. Document the default but make swapping trivial.

**`headless`.** `chromiumoxide`-based renderer. SPA detection heuristics (PRD §5.7) trigger when `headless: "auto"` and initial extraction is poor. Asset filtering via CDP — fulfill blocked requests with empty 200 responses, not aborts. Defaults: block media/images/fonts/third-party/service-workers; allow CSS.

**`vlm`.** SmolVLM (256M / 500M / 2.2B) via `mistral.rs`. Wired into the `extractor::images.rs` `CaptionVlm` mode call site that was stubbed in M4.

**`rover doctor` extensions.** Per-feature checks:
- `local-inference`: model files cached, mistral.rs loads.
- `vlm`: VLM model files.
- `headless`: chromiumoxide can find/launch a browser.

**Acceptance (PRD §14, M9).** Each feature works in isolation; default `cargo install rover` produces a lean binary; users opting in get the extras.

**Deferred from M9.** Nothing that was planned for v1.

**Open questions before planning.**
1. **`mistral.rs` API shape.** Pin a version. Confirm the inference API for both text and VLM workflows.
2. **`chromiumoxide` browser binary discovery.** Docker? System Chrome? Bundled? PRD §11.2 mentions `rover doctor` checks "chromiumoxide can find/launch a browser" — pick a discovery strategy.
3. **Model download UX.** First-time `local-inference` or `vlm` invocation triggers a HuggingFace download (gigabytes). Surface progress in stderr. Document the download in `docs/features.md`.
4. **Binary size with all features.** PRD §15 says default build < 25 MB. With features enabled the size can balloon. Document expected sizes.
5. **Cross-platform headless.** `chromiumoxide` works on Linux, macOS, Windows in principle, but the browser-launch path differs. Test matrix decision in plan.
6. **VLM model size selection.** Default to 256M for the speed/cost win, allow override. Document.

---

## Milestone dependency graph

```
M1 ─┬─> M2 ─┬─> M3 ─┬─> M4 ─┬─> M7 ──> M9 (vlm, local-inference)
    │       │       │       │
    │       │       └─> M5 ──> M6 ───┘
    │       │
    └───────┴─> M8 (any time after M5 + M6)
                │
                └─> M9 (headless requires M3 fetch tool surface)
```

- M1 is foundational; all others depend on it directly or transitively.
- M2 (storage) is required by M3, M5 (robots cache), M6 (tasks), M7 (summary cache).
- M3 (MCP) gates all subsequent MCP-tool deliveries.
- M4 (extraction richness) gates M7 (tables Summarize) and M9 (vlm).
- M5 (rate limiting) gates M6's batch worker (must integrate).
- M6 (task system) gates M7's deferred tasks if SWR scheduling lives there.
- M8 (polish) can ship any time after M5 + M6 — it's mostly diagnostics and config tooling, but `rover doctor` benefits from having all subsystems wired so it can check them.
- M9 (feature flags) is independent of M8 in principle but typically lands last because of the binary-size and operational-complexity considerations.

## When this doc gets updated

- Whenever a milestone plan ships, append a "Status" line to that section linking to the plan file.
- Whenever a deferred decision actually gets made, move it from "Open questions before planning" to the appropriate section body and link the resolving plan/PR.
- Whenever a new cross-cutting decision lands in the design supplement, audit this manifest and update affected milestone sections.
- Do not let this doc, the PRD, and the design supplement drift apart silently. If they conflict, the design supplement wins for architectural items, the PRD wins for product surface, this doc wins for milestone scoping.

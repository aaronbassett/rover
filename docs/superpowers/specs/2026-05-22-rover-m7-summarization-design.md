# Rover M7 — Summarization — Design

> Status: design complete, awaiting implementation plan.
>
> Prerequisites: M1 (fetcher), M2 (cache + storage actor), M3 (MCP server, `tokenizer` infra), M4 (extracted Markdown, tables/images sidecars), M5 (rate-limited fetcher), M6 (task scheduler — kept as schema-only for `summarize` kind).
>
> Canonical references:
> - PRD §4.1 (`fetch.summarize` arg, `fetch.max_tokens`), §4.3 (`summarize` tool), §4.4 (`get_metadata` tool), §4.5 (`count_tokens` tool), §6.3 (Tables Summarize mode), §7 (summarization), §8.1 + §8.4 (`summary_cache`).
> - Design supplement §2.6 (`[summarization]` config section), §2.7 (cache-miss path), §3.5 (`params_hash` includes backend identity), §4.4 (error model: snake_case codes), §4.5 (test strategy).
> - Milestone manifest §M7 (file layout, open questions, deferrals).

---

## 1. Scope and Goals

M7 ships the summarization subsystem and finishes the MCP tool surface that earlier milestones left stubbed.

**Summarizer core**
1. A `SummarizerBackend` trait with two implementations: `Extractive` (TextRank, offline, no dependencies on network or models) and `Cloud` (wraps `genai::Client`, covers all `genai`-supported providers plus `openai_compat` for custom OpenAI-compatible endpoints).
2. A config-driven backend registry: `[summarization]` section (defaults) plus `[backends.<name>]` blocks instantiated at startup.
3. A `SummarizerService` that owns the registry and the `summary_cache` read/write hot path.
4. Three compaction modes: `Extractive`, `Abstractive`, `Headlines`. Four output styles: `Bullet`, `Prose`, `Executive` (and an inherited Headlines bullet form). Preservation flags for code/tables/quotes/lists. Free-form focus prompt. Per-call backend override.

**MCP tools that ship here**
5. `summarize` — the headline tool (PRD §4.3). Synchronous in v1. On cache miss, runs the full fetch path then summarizes.
6. `count_tokens` — full envelope per PRD §4.5, including `summary_short` and `summary_medium` estimates computed by running the extractive backend at fixed target token budgets.
7. `get_metadata` — pure cache read of the existing `pages.metadata_json` (PRD §4.4); no summarizer involvement, but it lives in M7 to clear the PRD §4 tool list.

**Existing stubs/error paths wired through**
8. `fetch.summarize: SummarizeOpts | null` — currently accept-no-op; M7 wires it to run summarization after extraction.
9. `fetch.max_tokens: int | null` — currently returns `MaxTokensExceeded`; M7 turns the over-budget branch into auto-summarization using the default backend.
10. `TablesMode::Summarize` — currently errors `"tables summarize mode is not available until M7"`; M7 implements per-table summarization with extractive fallback.

**Schema**
11. Migration `005_summary_cache.sql` adds the `summary_cache` table (PRD §8.1).

**Acceptance (PRD §14, M7).** All three summarization modes work; cloud and extractive backends both produce non-empty summaries against representative inputs; the cache deduplicates repeat calls; `fetch` honors `max_tokens` by summarizing rather than erroring; `count_tokens` returns four estimates; `get_metadata` returns metadata for a cached URL without re-fetching.

**M6 follow-ups inherited.** M7 does *not* take the cross-process new-task notify deferral (manifest item #1) — the synchronous decision in §2 removes the dependency. Other M6 follow-ups (M7-irrelevant) stay open for M8: `--from-event` over MCP, wallclock batch timeout, SIGINT integration test for `--monitor`, orphan-reclaim integration test, `tasks/mod.rs` re-export visibility cleanup, CLI `--help` doc-comment polish, `item_failed { retry_in_s }` field reconciliation.

---

## 2. Decisions Inherited from Open-Question Round

| Question | Decision |
| --- | --- |
| Sentence segmenter | `unicode-segmentation` (UAX #29). Smaller binary; English-first product. |
| TF-IDF scope | Within-document IDF; each sentence is a pseudo-document. |
| PageRank params | `damping = 0.85`, `max_iter = 50`, `tol = 1e-4`, `similarity_floor = 0.1` (edges below the floor are dropped). |
| Sentence ordering in output | Original document order. |
| `Headlines` mode behavior | For each heading at the deepest covered depth (H1 if present, otherwise H2, etc.), emit the heading and the highest-PageRank-scoring sentence in its section. Documents with no headings fall back to a flat top-k extractive list. |
| `target_tokens` semantics | Extractive: pick the largest prefix of top-ranked-then-reordered sentences whose cumulative tokenizer count stays at-or-under `target_tokens`. Abstractive: embed as text in the prompt. Headlines: cap the number of (heading, sentence) pairs by estimated token cost. |
| Cloud streaming | Collect into `String`. No streaming in v1. |
| Backend errors | Distinct snake_case codes: `summarizer_backend_unavailable`, `summarizer_rate_limited`, `summarizer_auth_failed`, `summarizer_model_error`, `summarizer_no_such_backend`. If `[summarization] fallback_to_extractive = true` (default), a non-extractive failure retries once against the configured extractive backend; the response carries `summarizer_fallback: { from, reason }` in its metadata. |
| `params_hash` inputs | Sorted SHA-256 over `backend_name`, `model_id`, `mode`, `target_tokens` (or literal `"null"`), `focus` (trimmed; or `""`), `preserve` (lexically sorted), `style`. Components joined with `U+001E` (record separator) to avoid ambiguity. |
| Sync vs task | All summarize work is synchronous in M7. No task rows are inserted. The `summarize` kind stays in the migration enum to keep the schema final. |
| Summarize worker stub | Worker stays in `src/tasks/summarize.rs` but its body changes to error with `summarize_no_longer_a_task_kind` if a stale row exists. Schema is final; no new migration. |
| Summarization defaults | New `[summarization]` keys: `default_backend`, `default_mode`, `default_style`, `fallback_to_extractive`. |
| Cloud provider scope | All `genai` built-in providers (OpenAI, Anthropic, Gemini, xAI, Groq, DeepSeek, Together, Fireworks) plus a `openai_compat` kind that uses `ServiceTargetResolver` for `base_url`. Free-form `provider` string in config; rover delegates parsing to `genai`. |
| Other tool scope | `count_tokens` and `get_metadata` both ship in M7 alongside `summarize`. |
| Cache-miss path | `summarize` and the new MCP tools dispatch through the existing fetcher (full pipeline, cache write enabled), then summarize. |
| Migration | `005_summary_cache.sql`. |
| Backend registry shape | Built once at startup; shared via `Arc<SummarizerRegistry>`; injected into the MCP `Server` state and CLI commands the same way `Fetcher` is. |
| Trait dispatch | `Arc<dyn SummarizerBackend>` with `async_trait`. |
| Cache placement | `SummarizerService` wraps the registry and owns the `summary_cache` read/write path; individual backends are cache-unaware. |
| `max_tokens` overflow flow | Run summarization once with `mode = default_mode`, `target_tokens = max_tokens`, `style = default_style`. If the result still exceeds `max_tokens`, return the existing `MaxTokensExceeded` error — no recursion. |
| Tables-Summarize failure | Per-table: backend → extractive → keep verbatim. Surfaced in `tables_transformed[i].fallback_reason` (`"backend_failed"` / `"extractive_failed"`). |

---

## 3. Architecture

### 3.1 Module layout

```
src/
  summarizer/
    mod.rs                  # Re-exports; SummarizerService
    backend.rs              # SummarizerBackend trait + CompactOpts/Mode/Style/Preserve
    extractive.rs           # TextRank implementation
    cloud.rs                # genai wrapper
    prompts.rs              # Abstractive system-prompt template + render fn
    registry.rs             # Backend registry construction from [backends.*]
    error.rs                # SummarizerError (thiserror)
    types.rs                # Plain data types (SummaryRequest, SummaryResult, FallbackInfo)
  storage/
    summaries.rs            # tokio-rusqlite handle for summary_cache
    migrations/005_summary_cache.sql
  mcp/
    tools/
      summarize.rs          # MCP `summarize`
      count_tokens.rs       # MCP `count_tokens`
      get_metadata.rs       # MCP `get_metadata`
    state.rs                # extended with Arc<SummarizerService>
tests/
  summarizer_extractive.rs
  summarizer_cloud.rs       # wiremock-backed openai_compat backend
  summary_cache_lifecycle.rs
  mcp_summarize.rs
  mcp_count_tokens.rs
  mcp_get_metadata.rs
  fetch_max_tokens_auto_summarize.rs
  fetch_summarize_arg.rs
  tables_summarize_mode.rs
```

Crate stays single-binary. The summarizer is not promoted to a workspace member; the existing convention is to promote only when modules cross 3k LOC of public surface or grow reusable consumers.

### 3.2 Component diagram

```
                              ┌──────────────────────────┐
                              │      MCP Server          │
                              │  (rover mcp / rmcp stdio)│
                              └────┬─────────┬───────┬───┘
                                   │         │       │
              ┌────────────────────┴──┐  ┌───┴────┐ ┌┴───────────────┐
              │  fetch tool            │  │ count_ │ │ summarize tool  │
              │   - summarize arg      │  │ tokens │ │  - cache lookup │
              │   - max_tokens overflow│  │ tool   │ │  - cache-miss   │
              │   - tables.Summarize   │  └───┬────┘ │     fetch path  │
              └──┬─────────────────────┘      │      └─────┬───────────┘
                 │                            │            │
                 │   ┌────────────────────────┴────────────┴────────┐
                 │   │           SummarizerService                  │
                 │   │   compact(content, opts, content_hash):      │
                 │   │     1. params_hash = hash(opts, backend)     │
                 │   │     2. summary_cache lookup                  │
                 │   │     3. on miss: backend.compact()            │
                 │   │     4. on failure + fallback_to_extractive:  │
                 │   │            retry once with extractive        │
                 │   │     5. write summary_cache row               │
                 │   └────────────────────────┬─────────────────────┘
                 │                            │
                 │                  ┌─────────┴──────────┐
                 │                  │ SummarizerRegistry │
                 │                  │ HashMap<String,    │
                 │                  │   Arc<dyn SB>>     │
                 │                  └─┬───────┬──────┬───┘
                 │                    │       │      │
                 │              ┌─────┴───┐ ┌─┴────┐ ┌┴─────────┐
                 │              │Extract- │ │Cloud │ │ ...named │
                 │              │ive      │ │(genai│ │ backends │
                 │              │backend  │ │)     │ │          │
                 │              └─────────┘ └──┬───┘ └──────────┘
                 │                             │
                 │                             └── genai::Client
                 │                                  (OpenAI, Anthropic,
                 │                                   Gemini, openai_compat,
                 │                                   ...)
                 │
                 └─── Fetcher (M5) ─── Storage actor (M2) ─── summary_cache
```

### 3.3 Lifecycle: `summarize` MCP tool

```
1. rmcp dispatches `summarize` -> handler in src/mcp/tools/summarize.rs
2. Validate args (url, target_tokens, mode, focus, preserve, style, backend).
3. Look up cached page:
     storage.pages.get(url_hash) -> Option<PageRow>
4a. Miss path:
     - Call fetcher.fetch(url, default_extractor_opts).
     - Write to pages (already done inside fetcher).
     - Continue with the just-extracted markdown.
4b. Hit path:
     - Continue with page.extracted_md.
5. Build CompactOpts from args + [summarization] defaults.
6. SummarizerService.compact(page.content_hash, page.extracted_md, opts) ->
     - hash params (§3.5 below).
     - SELECT summary_md FROM summary_cache WHERE content_hash=? AND params_hash=?
     - on hit: return cached summary_md immediately.
     - on miss: backend.compact(content, &opts).
         - extractive: no I/O.
         - cloud: genai chat completion via the resolved provider config.
     - on cloud error and fallback_to_extractive: retry once with extractive.
     - INSERT INTO summary_cache.
7. Build response envelope:
     {
       "summary_md": "...",
       "metadata": {
         "backend": "fast" | "default",
         "mode": "abstractive",
         "target_tokens": 500,
         "estimated_tokens": 487,
         "cache_status": "hit" | "miss",
         "summarizer_fallback": { "from": "fast", "reason": "rate_limited" } | null,
         "source_url": "...",
         "source_fetched_at": "..."
       }
     }
```

### 3.4 Lifecycle: `fetch` with `summarize` arg or `max_tokens`

```
1. Normal fetch path runs to completion (extraction yields markdown + metadata).
2. If args.summarize is present:
     - Parse SummarizeOpts (same shape as the summarize tool).
     - Call SummarizerService.compact(content_hash, body_md, opts).
     - Replace body_md with the summary; tag metadata.summarized = true.
3. Else if args.max_tokens is Some(max) and tokens(body_md) > max:
     - Build CompactOpts {
          mode: default_mode,
          target_tokens: Some(max),
          style: default_style,
          focus: None,
          preserve: vec![],
          backend: None,    // resolves to default_backend
       }
     - Call SummarizerService.compact(...).
     - tokens(summary) > max ?
         - Yes: return McpError::MaxTokensExceeded { actual, max }.
         - No: replace body_md; tag metadata.auto_summarized = true.
4. Continue down the existing response-build path (frontmatter envelope + tables/images sidecars).
```

The auto-summarize branch is **single-shot**. No recursion. If a long document compresses badly we fail loud rather than silently degrading.

### 3.5 `params_hash` computation

```
serialize_for_hash =
    backend_name + "\u{1E}" +
    model_id     + "\u{1E}" +    // resolved from backend config; "" for extractive
    mode_str     + "\u{1E}" +    // "extractive" | "abstractive" | "headlines"
    target_tokens_str + "\u{1E}" + // integer-as-str or "null"
    focus_str         + "\u{1E}" + // s.trim().to_string() or ""
    preserve_sorted_csv + "\u{1E}" + // sorted([code,tables,quotes,lists]).join(",")
    style_str    // "bullet" | "prose" | "executive"

params_hash = lowercase_hex(sha256(serialize_for_hash))
```

Inputs are stable strings; we do not depend on Serde field ordering. `model_id` for extractive is the empty string (no model). `model_id` for cloud is the `model` key from the backend config.

### 3.6 SummarizerBackend trait

```rust
use async_trait::async_trait;

#[async_trait]
pub trait SummarizerBackend: Send + Sync {
    async fn compact(
        &self,
        content: &str,
        opts: &CompactOpts,
    ) -> Result<String, BackendError>;

    fn name(&self) -> &str;

    /// Resolved model identifier used for params_hash. "" for extractive.
    fn model_id(&self) -> &str { "" }
}
```

`BackendError` is a leaf error type with variants for `Unavailable`, `RateLimited`, `AuthFailed`, `ModelError`, `Invalid` (programmer-visible misuse). The `SummarizerService` translates these into `SummarizerError` (which the MCP layer maps to stable `McpError` codes).

### 3.7 Extractive algorithm

Implementation outline for `src/summarizer/extractive.rs`:

1. **Sentence split.** `unicode_segmentation::UnicodeSentences` over the full content. Each sentence becomes `(span_start_byte, text)`. Empty sentences and sentences shorter than 3 characters dropped.
2. **Tokenize per sentence.** Lowercased word tokens via `unicode_segmentation::UnicodeWords` + Unicode general-category filter (letters, numbers). No stemming.
3. **TF map per sentence.** `HashMap<&str, usize>` raw counts; convert to L2-normalized `HashMap<&str, f32>` after IDF.
4. **IDF over sentence corpus.** `idf(term) = ln(N / df(term))` where `N` = number of sentences. Terms appearing in every sentence get `idf = 0` (consistent with TF-IDF; suppresses stop-word-ish content without a stop list).
5. **Similarity matrix.** Cosine similarity between every pair of TF-IDF vectors. Drop edges below `0.1`.
6. **PageRank.** Power iteration with `damping = 0.85`, `max_iter = 50`, `tol = 1e-4`. Initial vector uniform.
7. **Mode-dependent selection:**
   - `Extractive` mode: rank sentences by score; greedily add in descending-rank order while cumulative `tokenizers::Tokenizer::count(s)` ≤ `target_tokens` (or until rank exhausted if no target). Re-order chosen sentences by `span_start_byte`.
   - `Headlines` mode: walk the source markdown's heading tree (PRD §4.4 metadata already extracts heading structure via `scraper` / `readabilityrs`'s rendered ATX format). For each heading at the deepest covered depth, find the highest-scoring sentence inside its section. Emit `## {heading}\n{sentence}\n\n` blocks. If no headings exist, fall back to flat top-k.
8. **Output.** Concatenate selected sentences with single newlines (or as bullets if `style == Bullet`); join headlines blocks with blank-line separators; for `style == Executive`, drop the leading filler sentences if a known "TL;DR" / "Summary" / "Abstract" heading exists, else fall back to `Prose`.

Deterministic; no randomness; ~250 lines as PRD §7.2 forecast.

### 3.8 Cloud backend

`src/summarizer/cloud.rs` wraps a single `genai::Client` per backend instance. Backend configuration:

```toml
[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
# api_key resolved by genai from OPENAI_API_KEY

[backends.lm_studio]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234/v1"
model = "qwen3.5-0.8b"
api_key_env = "LM_STUDIO_KEY"   # optional; openai_compat servers often need any non-empty key
```

For `openai_compat`, we install a `ServiceTargetResolver` on the `genai::Client` that maps the `model` to the configured `base_url`. For other providers, we delegate fully to `genai`'s built-in resolver (which already covers OpenAI/Anthropic/Gemini/xAI/Groq/DeepSeek/Together/Fireworks).

Sampling: we hold all sampling params at `genai` defaults in M7 (no `temperature`, `max_tokens` is implicit per provider). Sampling becomes a per-backend config knob if/when users ask. This is documented as a known limitation.

Request shape: a single chat completion. The summarizer builds a system message (prompt template, see §3.9) and a single user message (the content to summarize). The response is collected into a `String`.

Error mapping (genai → BackendError):
- `genai::Error::ApiError { status: 401 | 403 }` → `AuthFailed`.
- `genai::Error::ApiError { status: 429 }` → `RateLimited`.
- `genai::Error::ApiError { status: 5xx }` → `Unavailable`.
- network/timeout errors → `Unavailable`.
- `genai::Error::ModelNotFound` / invalid model → `ModelError`.
- Anything else → `Unavailable` (with the genai error stringified for the log).

### 3.9 Abstractive prompt template

`src/summarizer/prompts.rs` exports `render_abstractive(opts, content_hint) -> String`. Template:

```
You are a precise summarizer. Reply with only the summary — no preamble, no
postamble, no meta-commentary. Output valid Markdown.

Summarize the content provided in the user message.

Target length: ~{target_tokens} tokens.
Output style: {style_description}.
{if focus}
Focus on: {focus}
{endif}
{if preserve}
Preserve the following elements verbatim wherever they appear: {preserve_list}.
{endif}

Rules:
- Do not add information not present in the source.
- Do not include section titles or headers that the source does not have, unless
  the chosen style explicitly produces them.
- If the source is already shorter than the target, return it unchanged.
```

`style_description` table:
- `Bullet` → "Markdown bullet list, one fact per bullet, no nested bullets."
- `Prose` → "One or more short paragraphs."
- `Executive` → "Two-section format: a one-sentence headline, then a 'Details' paragraph."

The renderer omits sections cleanly (no leftover blank lines). Tests in `summarizer_prompts.rs` assert exact output for fixed inputs.

### 3.10 Tables Summarize mode

When `TablesMode::Summarize` is selected, each `<table>` block in the post-extraction markdown is summarized inline. Per table:

1. Convert table markdown to a stable plaintext rendering (`||` separators, header underline rows preserved).
2. Build `CompactOpts { mode: default_mode, target_tokens: Some(150), style: Bullet, focus: Some("Describe what this table shows. Highlight any extreme values or notable rows."), preserve: vec![], backend: None }`.
3. Call `SummarizerService.compact(table_content_hash, table_text, opts)` where `table_content_hash = sha256(table_text)`. Tables get their own cache rows keyed on table content; the same `summary_cache` table holds them.
4. Replace the table with the produced markdown. Annotate in `tables_transformed[i]`: `applied_mode: summarize`, `summary_md: "..."`, `fallback_reason: null | "backend_failed" | "extractive_failed"`.
5. On full failure (extractive also failed): keep the original table verbatim and record `fallback_reason: "extractive_failed"`.

This integrates with the existing `extractor::tables::apply` dispatch (§3.10 in M6 design): the `Summarize` arm dispatches through `SummarizerService`. The fetch tool reuses its existing `Arc<SummarizerService>` from MCP state; for `rover fetch` CLI, the binary constructs one at startup.

### 3.11 `count_tokens` MCP tool

Returns four estimates (PRD §4.5):

```json
{
  "url": "...",
  "estimates": {
    "raw_html":       14823,
    "extracted_md":   2891,
    "summary_short":  312,
    "summary_medium": 891
  }
}
```

Behavior:
- `raw_html`: tokenize the raw HTML bytes (UTF-8-decoded) with the configured tokenizer. Today no code path populates `pages.raw_html_zstd` even when `[cache] store_raw_html = true` (M2 left it NULL on purpose). In M7 we wire the fetcher to actually store the compressed raw HTML when the config flag is true; if `store_raw_html` is false (default) or the cached row predates the change, `raw_html` is returned as `null`. Document this in the tool docstring.
- `extracted_md`: tokenize `pages.extracted_md`.
- `summary_short`: run `SummarizerService.compact` with `mode = Extractive, target_tokens = Some(250), style = Bullet, backend = "<default-extractive-backend>"`, then tokenize. Always uses the extractive backend (never cloud) regardless of `default_backend`.
- `summary_medium`: same as short but `target_tokens = Some(750)`.

The two summary estimates are cached in `summary_cache` like any other summary, so the `count_tokens` cost is paid once per `(content_hash, params_hash)`.

### 3.12 `get_metadata` MCP tool

Pure cache read:

1. Look up `pages` by `url_hash`. On miss: fall through to the same fetch-with-defaults path that `summarize` uses, then read.
2. Parse `metadata_json`.
3. Return `{ url, metadata }`.

Lives in `src/mcp/tools/get_metadata.rs`. ~70 lines.

### 3.13 Configuration

New `[summarization]` section:

```toml
[summarization]
default_backend = "default"
default_mode    = "abstractive"   # extractive | abstractive | headlines
default_style   = "prose"         # bullet | prose | executive
fallback_to_extractive = true

[backends.default]
kind = "extractive"

[backends.fast]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"

[backends.smart]
kind = "cloud"
provider = "anthropic"
model = "claude-sonnet-4-7"

[backends.lm_studio]
kind = "cloud"
provider = "openai_compat"
base_url = "http://localhost:1234/v1"
model = "qwen3.5-0.8b"
```

Loading order:
1. Parse `[summarization]` → `SummarizationDefaults`.
2. Parse each `[backends.*]` as `BackendConfig { kind, provider?, model?, base_url?, api_key_env? }`.
3. Validate: `default_backend` must refer to an actual `[backends.<name>]` entry. If not, fail at startup with `summarizer_no_such_backend`.
4. Validate: at least one extractive backend exists. If `fallback_to_extractive = true` and there isn't one, fail at startup with `summarizer_no_extractive_backend_for_fallback`.

If no `[backends.*]` blocks are configured at all, rover installs an implicit `[backends.default] kind = "extractive"` and uses that. This makes a fresh install summarizable offline without any config.

### 3.14 Error model

Per-module error enum (`SummarizerError`):

```rust
#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    #[error("no such backend: {name}")]
    NoSuchBackend { name: String },

    #[error("backend {name} unavailable: {reason}")]
    BackendUnavailable { name: String, reason: String },

    #[error("backend {name} rate limited")]
    RateLimited { name: String },

    #[error("backend {name} auth failed")]
    AuthFailed { name: String },

    #[error("backend {name} model error: {reason}")]
    ModelError { name: String, reason: String },

    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("token counting error: {0}")]
    Tokenizer(#[from] crate::tokenizer::TokenizerError),
}
```

MCP code mapping (added to `src/mcp/envelope.rs`):

| Variant                 | MCP code                          |
| ----------------------- | --------------------------------- |
| `NoSuchBackend`         | `summarizer_no_such_backend`      |
| `BackendUnavailable`    | `summarizer_backend_unavailable`  |
| `RateLimited`           | `summarizer_rate_limited`         |
| `AuthFailed`            | `summarizer_auth_failed`          |
| `ModelError`            | `summarizer_model_error`          |
| `Storage`               | passthrough (existing storage codes) |
| `Tokenizer`             | passthrough |

The fallback path **does not** surface the original backend's error to the agent — it surfaces success with a `summarizer_fallback` metadata block. Only when both the requested backend and the extractive backend fail does the call return an error (and the code is the extractive backend's error, not the original's).

### 3.15 Crate dependencies added

```toml
async-trait = "0.1"
unicode-segmentation = "1"
genai = "0.4"           # latest stable as of plan date; pinned in plan
```

`tokenizers` is already a dep (M3). `sha2` is already a dep (M1). No new transitive surprises expected.

---

## 4. Schema (migration 005_summary_cache.sql)

```sql
CREATE TABLE summary_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash  TEXT NOT NULL,
    params_hash   TEXT NOT NULL,
    summary_md    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE(content_hash, params_hash)
);

CREATE INDEX summary_cache_by_content ON summary_cache(content_hash);
```

Notes:
- `content_hash` is the existing `pages.content_hash` (sha256 of `extracted_md`). For Tables Summarize, it's `sha256(table_text)` instead — the same column holds both shapes.
- `params_hash` is the §3.5 hash.
- No FK to `pages` because table-text summaries don't have a page row.
- `created_at` retained for future LRU eviction (PRD §8.5 cache stats); no eviction logic ships in M7.

Migration runner uses the same `crate::storage::migrations::apply_all` machinery already in place. No backfill required.

---

## 5. MCP Tool Schemas

### 5.1 `summarize`

Input (matches PRD §4.3):

```json
{
  "url": "https://example.com/article",
  "target_tokens": 500,
  "mode": "abstractive",
  "focus": "key technical claims",
  "preserve": ["code"],
  "style": "prose",
  "backend": "fast"
}
```

All fields except `url` are optional. Defaults pulled from `[summarization]`.

Output:

```json
{
  "summary_md": "...",
  "metadata": {
    "backend":     "fast",
    "mode":        "abstractive",
    "style":       "prose",
    "target_tokens": 500,
    "estimated_tokens": 487,
    "cache_status": "hit",
    "summarizer_fallback": null,
    "source_url": "https://example.com/article",
    "source_fetched_at": "2026-05-22T10:00:00Z",
    "preserve": [],
    "focus": null
  }
}
```

Docstring (visible to MCP clients) spells out the cache-miss-fetch behavior per design §2.7.

### 5.2 `count_tokens`

Input:

```json
{
  "url": "https://example.com/article",
  "tokenizer": "cl100k"
}
```

Output: as PRD §4.5 (above).

### 5.3 `get_metadata`

Input:

```json
{
  "url": "https://example.com/article",
  "metadata": { ... MetadataOpts ... }
}
```

Output: parsed metadata object (same as the `metadata` field on `fetch`'s output).

---

## 6. Workers

M7 introduces no new workers and removes none. The `summarize` task kind stays in the schema's CHECK constraint and the `TaskKind` enum (frozen in M6). The worker stub in `src/tasks/summarize.rs` changes its error message to `summarize_no_longer_a_task_kind` and gains a comment explaining that future v2 work may repurpose the kind for batched async summarization.

No new tasks are inserted in M7. The summarize MCP tool and the fetch-time hooks are all synchronous.

---

## 7. CLI

No new top-level CLI subcommands. The CLI surfaces stay minimal in M7 — the summarizer's home is the MCP tool surface. The MCP server receives an `Arc<SummarizerService>` constructed in `src/main.rs` from the loaded `Config`, parallel to how `Fetcher` is built. `rover mcp` and `rover fetch` (and any other binary subcommand that grows summarizer use later) share the same instance via the binary's startup wiring.

`rover fetch`'s CLI flag surface (a thin wrapper over the same args the MCP fetch tool accepts) gains two flags in M7:
- `--max-tokens <N>` — same semantics as the MCP arg.
- `--summarize <JSON>` — JSON literal matching the `SummarizeOpts` shape, e.g. `--summarize '{"mode":"abstractive","target_tokens":500}'`. Mirrors the existing `--tables` / `--images` JSON-arg pattern.

No other summarization-specific CLI flags (no `--summarize-mode`, `--summarize-target-tokens`, etc.). Adding ergonomic shortcuts is M8 polish.

---

## 8. Test Strategy

| Layer | Test |
| ----- | ---- |
| Unit | `extractive::tests::sentence_split_handles_unicode_punctuation` |
| Unit | `extractive::tests::tfidf_with_repeated_terms` |
| Unit | `extractive::tests::pagerank_converges_on_uniform_chain` |
| Unit | `extractive::tests::target_tokens_caps_output` |
| Unit | `prompts::tests::abstractive_template_renders_all_styles` |
| Unit | `service::tests::params_hash_deterministic_under_input_reordering` |
| Unit | `service::tests::cache_hit_short_circuits_backend` |
| Unit | `service::tests::backend_failure_falls_back_when_enabled` |
| Unit | `service::tests::backend_failure_propagates_when_fallback_disabled` |
| Unit | `registry::tests::missing_default_backend_fails_startup` |
| Integration | `summarizer_extractive` — feed a known document, assert sentence selection and ordering |
| Integration | `summarizer_cloud` — wiremock-backed `openai_compat` endpoint; assert request shape (model, system/user messages) and response handling |
| Integration | `summary_cache_lifecycle` — insert, hit, miss-after-params-change |
| Integration | `mcp_summarize` — end-to-end via `tests/common/mod.rs::spawn_client`; covers cache hit, cache miss with internal fetch, fallback metadata |
| Integration | `mcp_count_tokens` — verify four estimates + cache reuse |
| Integration | `mcp_get_metadata` — covers cache hit, cache miss with internal fetch |
| Integration | `fetch_max_tokens_auto_summarize` — extracted > max_tokens triggers auto-summarize; if still over, returns `MaxTokensExceeded` |
| Integration | `fetch_summarize_arg` — `summarize: { ... }` arg produces summarized body |
| Integration | `tables_summarize_mode` — per-table summarization + extractive fallback path |

Cloud tests use `--features test-loopback` to point genai at a wiremock server bound to `127.0.0.1`. SSRF must be set to `Loopback` (via `TestLoopback` for now) in those tests; rover.toml seeded with `robots.respect = false` per existing convention.

No live network calls in CI. `genai`'s real provider integrations are exercised manually via `rover doctor` (M8).

---

## 9. Error Codes Added

Stable MCP error codes added to `src/mcp/envelope.rs`:

- `summarizer_no_such_backend`
- `summarizer_no_extractive_backend_for_fallback`
- `summarizer_backend_unavailable`
- `summarizer_rate_limited`
- `summarizer_auth_failed`
- `summarizer_model_error`
- `summarize_no_longer_a_task_kind` *(stub-worker error; will appear only if a stale row from a pre-M7 DB is reclaimed)*

Existing codes referenced or rewritten:
- `max_tokens_exceeded` — message updated to note that auto-summarization was attempted and still overshot.

All codes documented in `docs/mcp-tools.md` during M8 polish.

---

## 10. Acceptance Criteria

1. `rover mcp` boots with no `[backends.*]` in config and answers `summarize` calls using the implicit extractive backend.
2. With a `[backends.fast]` `openai_compat` block pointed at a local LM Studio (or `cargo test --features test-loopback`'s wiremock surrogate), `summarize { backend: "fast" }` returns a non-empty `summary_md`.
3. A second `summarize` call with identical params returns `cache_status: "hit"` and skips the backend.
4. `fetch { url, max_tokens: 100 }` against a document that extracts to >100 tokens returns a summarized body and `metadata.auto_summarized = true` (rather than `MaxTokensExceeded`).
5. `fetch { url, tables: { mode: "summarize" } }` against a document containing tables returns markdown with each table replaced by a summary.
6. `count_tokens { url }` returns four numeric estimates; `summary_short` and `summary_medium` are present even when the URL was previously fetched without summaries.
7. `get_metadata { url }` returns the metadata object without re-fetching when the URL is cached; fetches with defaults when it isn't.
8. Removing a backend's API-key env var causes the next `summarize { backend: "<that one>" }` to fall back to extractive (with `summarizer_fallback: { reason: "auth_failed" }`) and return a non-empty summary.
9. Setting `fallback_to_extractive = false` in the same scenario returns `summarizer_auth_failed`.
10. All 322 existing tests still pass; new test suite adds at least 18 new test cases (10 unit + 8 integration).

---

## 11. Open Items Deferred to Writing-Plans

None of these block the design; they're plan-level details that can be settled when each task is scaffolded.

1. **`genai` version pin.** Check `npm view`-equivalent for crates.io (`cargo search genai`) at plan-writing time; pin the patch version. Confirm `ServiceTargetResolver` and `openai_compat` shape are still stable in the picked version. If breaking changes since this design, write a one-paragraph adaptation note in the plan.
2. **`async-trait` version pin.** Standard 0.1.x; pick the latest patch.
3. **Token-budget greedy selection corner case.** If even the single highest-ranked sentence exceeds `target_tokens`, emit it anyway and log a warning. Pin this behavior in the extractive task.
4. **Heading-tree extraction in Headlines mode.** Re-use existing metadata extraction if it surfaces a heading list; otherwise add a tiny ATX/setext walker in `extractive.rs`. Pin the choice once the existing extractor's outputs are inspected.
5. **`genai` chat-message shape.** Two messages (system + user) vs. one merged message — depends on provider behavior. Default: two messages.
6. **Table content hashing.** Confirm post-extraction tables have a stable text rendering — if `readabilityrs` produces nondeterministic table markdown (column padding etc.), normalize whitespace before hashing.
7. **`get_metadata` argument shape.** `metadata: MetadataOpts` mirrors `fetch`'s; double-check the structurally compatible Rust type lives in one place.

---

## 12. Decision Log

| # | Decision | Why |
| - | -------- | --- |
| 1 | Synchronous summarize tool in v1 | Removes cross-process notify dependency, removes summarize task spawning, keeps M7 small. Cloud calls are seconds, not minutes. |
| 2 | Implicit `[backends.default] kind = "extractive"` if no backends configured | A fresh install should be useful offline without any user setup. |
| 3 | Service wraps registry; backends are cache-unaware | One hot path, one tested cache write, easy backend swap. |
| 4 | Fallback-to-extractive default `true` | Robust agent flows. Strict mode is one config line away. |
| 5 | `params_hash` includes backend identity (not just model) | Two backends with the same model may differ in prompts/sampling. Mirrors design §3.5. |
| 6 | `count_tokens` runs real extractive summarization for short/medium estimates | Honest numbers worth the few-ms cost; results cached. Heuristic ratios were considered and rejected. |
| 7 | `get_metadata` ships in M7 alongside `count_tokens` | Closes the PRD §4 tool list. Trivial code change once the cache-miss-fetch utility exists. |
| 8 | `Headlines` mode = heading + top-1 sentence per section | Tightest useful definition given that the source markdown carries headings already. |
| 9 | Single-shot `max_tokens` auto-summarize, no recursion | Bounded cost, clear failure mode. Recursion has degenerate cases. |
| 10 | Cloud streaming deferred | MCP isn't streaming-aware; defer until a streaming consumer exists. |
| 11 | Per-table cache rows in the same `summary_cache` table | Avoids a second cache table; `content_hash` is naturally generic. |
| 12 | Summarize task kind stays in schema but worker is stub-only | Schema was declared final in M6. Worker change is a code-only edit. |
| 13 | No new `--summarize-*` CLI flags in M7 | Stays minimal. Config + JSON arg cover the v1 surface. Flag bloat is an M8 question. |
| 14 | All `genai`-supported providers in scope | Delegating to `genai`'s built-in resolver is zero-cost. Per-provider hardening is an M8/M9 problem. |

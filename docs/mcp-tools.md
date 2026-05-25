# Rover MCP Tools

Rover speaks the Model Context Protocol over stdio (`rover mcp`). It exposes five tools: `fetch`, `batch_fetch`, `summarize`, `get_metadata`, and `count_tokens`. All arguments are validated by JSON Schema (`deny_unknown_fields`) — unknown keys are rejected with `invalid_args`.

Errors are returned as a stable envelope:

```jsonc
{ "code": "<stable_string>", "message": "<human_readable>" }
```

Code strings are stable from M3 onward:

`max_tokens_exceeded`, `invalid_args`, `invalid_url`, `ssrf_denied`, `fetch_failed`, `extract_failed`, `storage_error`, `tokenizer_unavailable`, `robots_disallowed`, `robots_fetch_failed`, `retry_exhausted`, `rate_limited`, `deferred`, `too_many_urls`, `empty_url_list`, `summarizer_no_such_backend`, `summarizer_no_extractive_backend_for_fallback`, `summarizer_backend_unavailable`, `summarizer_rate_limited`, `summarizer_auth_failed`, `summarizer_model_error`, `summarizer_invalid_request`.

## `fetch`

Synchronously fetches a URL, runs the M1+M4 extraction pipeline, and returns Markdown + frontmatter + metadata. Optional inline summarization.

**Args:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | string | required | URL to fetch. |
| `force_refresh` | bool | `false` | Bypass cache for this request. |
| `count_only` | bool | `false` | Skip extraction; return only the token count of the (cached or fresh) extracted body. |
| `tokenizer` | string | from `[tokenizer] default` | `o200k` / `cl100k` / `claude`. |
| `max_tokens` | integer | unset | Auto-summarize when the extracted body exceeds this. Must be `> 0`. Single-shot: if the summary is still over budget, returns `max_tokens_exceeded`. |
| `tables` | object | `{mode:"embed"}` | Per-table mode. See below. |
| `images` | object | `{mode:"alt_text_only"}` | Per-image mode. See `## images modes` below. |
| `metadata` | string | `"include"` | `"include"` or `"skip"`. When `skip`, the response's metadata fields are blanked (the cache row still carries them). |
| `summarize` | object | unset | Inline summarize after extraction. See below. |
| `headless` | object | unset | Browser rendering control (M9). See `## headless` below. |

**`tables` modes:**

```jsonc
{"mode":"embed"}
{"mode":"drop"}
{"mode":"csv_file"}
{"mode":"summarize"}
{"mode":"sample","strategy":"head_tail","head":5,"tail":5}
{"mode":"sample","strategy":"random_seed","rows":10,"seed":42}
```

`head_tail` defaults: `head=5`, `tail=5`. `random_seed` defaults: `rows=10`, `seed=42`. `head`/`tail`/`rows` must be `> 0`.

### `images` modes (M9)

```jsonc
{"mode":"keep"}
{"mode":"alt_text_only"}
{"mode":"download"}
{"mode":"drop"}
{"mode":"caption"}
```

- `keep` — preserve all image tags; images appear as `![alt](src)` in the Markdown.
- `alt_text_only` — replace each image with its alt text only (no image tag).
- `download` — fetch each image and write to `[output] dir`; Markdown contains local file references.
- `drop` — remove all image tags.
- `caption` — replace each image with a generated caption.
  Requires at least one configured captioner (`[captioners.<name>]`). The
  default captioner comes from `[image_captions] default`; override per-call
  via `images.captioner: "<name>"`.

### `headless` (M9)

When the binary is built with `--features headless`, pass:

```json
{
  "headless": {
    "mode": "off" | "on" | "auto",
    "wait": "domcontentloaded" | "networkidle2",
    "timeout_secs": 15
  }
}
```

- `mode` (default: derived from `[headless] auto_detect_spa`)
  - `off` — disable headless for this call (use the reqwest path only)
  - `on` — render this URL via headless unconditionally
  - `auto` — try reqwest first; re-render via headless if SPA heuristics fire
- `wait` (default: `[headless] default_wait`)
- `timeout_secs` (default: `[headless] timeout`)

When the binary is built **without** the `headless` feature:
- `mode: "off"` and the absent case work as today (no-op)
- `mode: "on"` returns the error `headless_feature_not_compiled`
- `mode: "auto"` keeps the reqwest result silently (no error)

**`summarize` sub-arg** (mirrors the standalone `summarize` tool minus `url`):

```jsonc
{
  "target_tokens": 500,
  "mode": "extractive|abstractive|headlines",
  "focus": "...",
  "preserve": ["code","tables","quotes","lists"],
  "style": "bullet|prose|executive",
  "backend": "<backend name>"
}
```

When `summarize` is provided, the returned `markdown` is the summary (not the extracted body) and `summarized: true` is set.

**Response (full):**

```jsonc
{
  "markdown": "...",
  "frontmatter": "---\n...\n---",
  "cache_status": "hit|miss|stale",
  "revalidation": {                    // present iff cache_status="stale" and a revalidate task was queued
    "task_id": "...",
    "monitor_command": "rover task <id> --monitor",
    "poll_command": "rover task <id>",
    "hint": "Optional. Revalidation runs in the background regardless."
  },
  "summarized": true,                  // present when `summarize` arg was used
  "auto_summarized": true,             // present when `max_tokens` triggered auto-summarize
  "summarizer_fallback": {             // present when whichever summarize path ran fell back to extractive
    "from": "fast",
    "reason": "auth_failed"
  },
  "images_processed": [                // present when images.mode includes caption filtering (M9)
    {
      "src": "https://example.com/image.jpg",
      "decision": "captioned",
      "captioner": "openai",
      "caption": "A black labrador retriever sitting on a wooden dock."
    },
    {
      "src": "https://example.com/icon.svg",
      "decision": "skipped",
      "reason": "below_min_dimensions",
      "dimensions": { "width": 24, "height": 24 }
    },
    {
      "src": "https://example.com/large.jpg",
      "decision": "skipped",
      "reason": "above_max_bytes",
      "bytes": 18234567
    },
    {
      "src": "https://example.com/photo.jpg",
      "decision": "skipped",
      "reason": "per_page_budget"
    },
    {
      "src": "https://example.com/error.jpg",
      "decision": "skipped",
      "reason": "captioner_error",
      "error": "openai: rate limited"
    }
  ]
}
```

**Response (`count_only=true`):** the `CountSingleResponse` shape — see `count_tokens` below.

**Errors:** `invalid_url`, `invalid_args`, `ssrf_denied`, `fetch_failed`, `robots_disallowed`, `robots_fetch_failed`, `retry_exhausted`, `rate_limited`, `extract_failed`, `tokenizer_unavailable`, `max_tokens_exceeded`, `summarizer_*`.

## `batch_fetch`

Schedules a background batch fetch. Returns a `TaskCreatedResponse` immediately; the task runs asynchronously and is observed via `rover batch <id>` (or the `Monitor` MCP tool).

**Args:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `urls` | array<string> | required | 1–100 URLs. SSRF-validated up front; any reject pre-empts the task insert. |
| `force_refresh` | bool | `false` | Apply to every URL. |
| `concurrency` | integer | `8` | Total in-flight requests for this batch. Clamped to `1..=32`. |
| `per_domain_concurrency` | integer | `2` | Per-host in-flight requests for this batch. Clamped to `1..=8`. |

**Response:**

```jsonc
{
  "task_id": "...",
  "status": "running",
  "kind": "batch_fetch",
  "monitor_command": "rover batch <id> --monitor",
  "poll_command": "rover batch <id>",
  "cancel_command": "rover task <id> --cancel",
  "hint": "Use the Monitor tool with monitor_command for live updates, or call poll_command to check status."
}
```

**Errors:** `empty_url_list`, `too_many_urls`, `invalid_url`, `ssrf_denied`, `invalid_args`, `storage_error`.

## `summarize`

Cache-or-fetch a URL, dispatch through the summarizer service, return the summary. Synchronous; no task spawning.

**Args:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | string | required | URL to summarize. |
| `target_tokens` | integer | unset | Target token count for the summary. Hint, not a hard cap. |
| `mode` | string | from `[summarization] default_mode` (`abstractive`) | `extractive`, `abstractive`, or `headlines`. |
| `focus` | string | unset | Free-text focus prompt threaded into the summarizer prompt. |
| `preserve` | array<string> | `[]` | Sections to keep verbatim. Subset of `code`, `tables`, `quotes`, `lists`. |
| `style` | string | from `[summarization] default_style` (`prose`) | `bullet`, `prose`, or `executive`. |
| `backend` | string | from `[summarization] default_backend` | Named `[backends.<name>]` to use. |
| `tokenizer` | string | from `[tokenizer] default` | Family used to count the resulting summary. |

**Response:**

```jsonc
{
  "summary_md": "...",
  "metadata": {
    "backend": "fast",
    "mode": "abstractive",
    "style": "prose",
    "target_tokens": 500,                // omitted when unset
    "estimated_tokens": 487,
    "cache_status": "hit|miss",
    "summarizer_fallback": {             // omitted when no fallback
      "from": "fast",
      "reason": "rate_limited"
    },
    "source_url": "https://...",
    "source_fetched_at": "2026-05-22T12:34:56Z",
    "focus": "...",                      // omitted when unset
    "preserve": ["code","tables"]
  }
}
```

**Errors:** `invalid_url`, `invalid_args`, `ssrf_denied`, `fetch_failed`, `extract_failed`, `tokenizer_unavailable`, `summarizer_no_such_backend`, `summarizer_no_extractive_backend_for_fallback`, `summarizer_backend_unavailable`, `summarizer_rate_limited`, `summarizer_auth_failed`, `summarizer_model_error`, `summarizer_invalid_request`.

## `get_metadata`

Cache-or-fetch a URL and return only the structured metadata (no Markdown body).

**Args:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | string | required | URL to fetch. |
| `force_refresh` | bool | `false` | Bypass cache. |
| `tokenizer` | string | from `[tokenizer] default` | Tokenizer family (passed through to ensure the registry is loaded; not surfaced in the response). |

**Response:**

```jsonc
{
  "title": "...",                  // each field omitted when null
  "description": "...",
  "author": "...",
  "published": "ISO-8601 string",
  "modified": "ISO-8601 string",
  "image": "https://...",
  "og_type": "article",
  "canonical": "https://...",
  "language": "en",
  "schema_types": ["Article"],
  "extraction_quality": 0.87,
  "url": "https://...",
  "content_hash": "sha256:...",
  "fetched_at": "2026-05-22T12:34:56Z",
  "cache_status": "hit|miss|stale"
}
```

**Errors:** `invalid_url`, `invalid_args`, `ssrf_denied`, `fetch_failed`, `extract_failed`, `tokenizer_unavailable`.

## `count_tokens`

Two shapes selected by `mode`.

**Args:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `text` | string | unset | In-process tokenization. Mutually exclusive with `url`. |
| `url` | string | unset | Tokenize the extracted body of `url`. Mutually exclusive with `text`. |
| `tokenizer` | string | from `[tokenizer] default` | Tokenizer family. |
| `mode` | string | `"single"` | `single` (one count) or `estimates` (four counts, URL-only). |

### `mode = "single"` (default)

Requires exactly one of `text` or `url`.

```jsonc
{
  "tokens": 1234,
  "tokenizer": "o200k",
  "source": "text|url",
  // url-mode only:
  "url": "https://...",
  "content_hash": "sha256:...",
  "fetched_at": "2026-05-22T12:34:56Z",
  "cache_status": "hit|miss|stale"
}
```

### `mode = "estimates"` (URL-only)

Rejects `text`. Returns four counts in one round-trip: the cached raw HTML (when `[cache] store_raw_html = true` and the row carries a valid zstd blob — `null` otherwise), the extracted Markdown, and two extractive-summary estimates at `~250` and `~750` target tokens. Estimates always run on the extractive backend (never cloud); requires at least one extractive backend or returns `summarizer_no_extractive_backend_for_fallback`.

```jsonc
{
  "url": "https://...",
  "tokenizer": "o200k",
  "estimates": {
    "raw_html": 8421,                    // omitted when null
    "extracted_md": 1234,
    "summary_short": 248,
    "summary_medium": 742
  }
}
```

**Errors:** `invalid_args`, `invalid_url`, `ssrf_denied`, `fetch_failed`, `extract_failed`, `tokenizer_unavailable`, `summarizer_no_extractive_backend_for_fallback`.

---
id: token-budgets
title: Managing token budgets
---

# Managing token budgets

Every page you fetch costs tokens, and most of that cost is decided before the body reaches your model. Rover gives you the count up front and the controls to cap it.

## Know the cost before you pay it

Every fetched document states its own cost in its frontmatter. Each Rover document carries `estimated_tokens` and `tokenizer` at the top — see [Anatomy of a Rover document](/docs/output) for the full shape.

For a count without fetching the body, use the [`count_tokens`](/docs/mcp-tools) tool. It runs in two modes.

`single` (the default) returns one count. Pass exactly one of `text` (an in-process string) or `url` (the extracted body of a page) — supplying both, or neither, is an error.

`estimates` (URL only) returns four counts in one round-trip. It rejects `text`. You get `extracted_md`, `summary_short` (~250 target tokens), `summary_medium` (~750 target tokens), and `raw_html` — `raw_html` appears only when `[cache] store_raw_html = true` and a valid stored blob exists, and is omitted otherwise. Estimates always run on the offline extractive backend, so they cost no API calls; they need at least one extractive backend configured.

[`fetch`](/docs/mcp-tools) with `count_only = true` returns the token count of the extracted body and nothing else.

## Pick the tokenizer that matches your model

A token count is only useful if it counts the way your model does. Rover ships five tokenizer families. A count from the wrong tokenizer is a confident estimate of the wrong thing.

| Family | Matches |
| --- | --- |
| `cl100k` | GPT-4 |
| `o200k` | GPT-4o — the default |
| `claude` | Claude |
| `llama3` | Llama 3 |
| `qwen3` | Qwen3 |

The default comes from `[tokenizer] default` in your config, set to `o200k` — see [Configuration](/docs/configuration). `fetch`, `summarize`, `count_tokens`, and `get_metadata` each take a per-call `tokenizer` argument to override it. Tokenizers lazy-download on first use, so the first count with a new family pays a one-time fetch; every count after that is local.

## Fit a page to a budget

A budget is a number you hand Rover so a page can't blow past it. Set it with `max_tokens` (it must be greater than 0). When the extracted body exceeds the budget, Rover auto-summarises once toward it and sets `auto_summarized: true` on the response. The MCP tool and the CLI diverge on what happens when that one summary still doesn't fit.

MCP `fetch` `max_tokens` is a hard ceiling. The auto-summarise is single-shot. If the one summary still lands over budget, the call returns the `max_tokens_exceeded` error rather than handing back something too big. Nothing over the limit reaches your context window. If you've already supplied an explicit `summarize` argument, Rover won't override your choice — it surfaces the error directly.

CLI `rover fetch --max-tokens N` is a best-effort target. Same single auto-summarise, but the budget is a target, not a wall. The offline summary can land a few tokens over, and the CLI emits it anyway — no error. Use the MCP ceiling when going over is unacceptable; use the CLI target when close enough is fine and you'd rather have the content than a failure.

```bash
# Best-effort: emits the summary even if it lands a little over 4000 tokens
rover fetch --max-tokens 4000 https://example.com/long-article

# Summarise explicitly first, then apply the budget
rover fetch --summarize '{"mode":"abstractive","target_tokens":1500}' \
  --max-tokens 4000 https://example.com/long-article
```

The `--summarize` blob takes the same shape as the [`summarize`](/docs/summarizing) tool's arguments, minus `url`. It runs first; `--max-tokens` applies to the result. To shape a page deliberately rather than capping it, reach for the `summarize` tool or the inline `summarize` argument on `fetch` — see [Summarising pages](/docs/summarizing).

## A budgeting workflow

Estimate, decide, then fetch — in that order. The estimate is cheap and offline; the fetch is the part that costs you.

1. **Estimate.** Run `count_tokens` in `estimates` mode against the URL. One call gives you the full extracted size and two summary sizes, with no API spend.
2. **Decide.** If the page fits, fetch it as-is. If it's close, fetch with a `max_tokens` budget. If it's far over, summarise deliberately to the size you want.
3. **Fetch.** Pull the page with the choice you made — at full size, capped to a ceiling, or pre-summarised.

The numbers come from the same offline extractive backend whether you estimate, cap, or summarise, so the size you see at step 1 is the size you act on at step 3. A second fetch of a page you already estimated reuses the stored copy rather than paying the round-trip again — see [Caching & freshness](/docs/caching).

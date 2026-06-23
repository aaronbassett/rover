---
id: token-budgets
title: Managing token budgets
---

# Managing token budgets

Most of a page's token cost is fixed before its body reaches your model. Rover reports the count first and gives you the controls to cap it.

## Count before fetching

Every Rover document carries its own cost. The frontmatter includes `estimated_tokens` and `tokenizer` at the top; see [Anatomy of a Rover document](/docs/output) for the full shape.

To get a count without pulling the body, use the [`count_tokens`](/docs/mcp-tools) tool. It has two modes.

`single` is the default and returns one count. Pass exactly one of `text` (an in-process string) or `url` (the extracted body of a page). Supplying both or neither is an error.

`estimates` is URL-only and returns four counts in a single round-trip. It rejects `text`. You get `extracted_md`, `summary_short` (~250 target tokens), `summary_medium` (~750 target tokens), and `raw_html`. The `raw_html` count appears only when `[cache] store_raw_html = true` and a valid stored blob exists, and is omitted otherwise. Estimates always run on the offline extractive backend, so they cost no API calls. They need at least one extractive backend configured.

[`fetch`](/docs/mcp-tools) with `count_only = true` returns the token count of the extracted body and nothing else.

## Match the tokenizer to your model

A count is only useful if it counts the way your model does. Rover ships five tokenizer families.

| Family | Matches |
| --- | --- |
| `cl100k` | GPT-4 |
| `o200k` | GPT-4o (the default) |
| `claude` | Claude |
| `llama3` | Llama 3 |
| `qwen3` | Qwen3 |

The default is `[tokenizer] default` in your config, set to `o200k`; see [Configuration](/docs/configuration). `fetch`, `summarize`, `count_tokens`, and `get_metadata` each accept a per-call `tokenizer` argument to override it. Tokenizers lazy-download on first use, so the first count with a new family pays a one-time fetch. Every count after that is local.

## Fit a page to a budget

A budget caps how large a page can be. Set it with `max_tokens`, which must be greater than 0. When the extracted body exceeds the budget, Rover auto-summarises once toward it and sets `auto_summarized: true` on the response. The summary runs once. What happens if that single pass still doesn't fit depends on whether you're calling the MCP tool or the CLI.

Under MCP, `fetch` `max_tokens` is a hard ceiling. If the one summary still lands over budget, the call returns the `max_tokens_exceeded` error instead of oversized content, so nothing over the limit reaches your context window. If you've already supplied an explicit `summarize` argument, Rover keeps your choice and surfaces the error directly rather than overriding it.

On the CLI, `rover fetch --max-tokens N` is a best-effort target. Same single auto-summarise, but the budget is a target rather than a wall. An offline summary can land a few tokens over, and the CLI emits it anyway with no error. Use the MCP ceiling when going over is unacceptable. Use the CLI target when close enough is fine and you'd rather have the content than a failure.

```bash
# Best-effort: emits the summary even if it lands a little over 4000 tokens
rover fetch --max-tokens 4000 https://example.com/long-article

# Summarise explicitly first, then apply the budget
rover fetch --summarize '{"mode":"abstractive","target_tokens":1500}' \
  --max-tokens 4000 https://example.com/long-article
```

The `--summarize` blob takes the same shape as the [`summarize`](/docs/summarizing) tool's arguments, minus `url`. It runs first, then `--max-tokens` applies to the result. To shape a page deliberately instead of just capping it, use the `summarize` tool or the inline `summarize` argument on `fetch`; see [Summarising pages](/docs/summarizing).

## A budgeting workflow

Estimate, decide, then fetch, in that order. The estimate is cheap and offline. The fetch is the part that costs you.

1. Estimate. Run `count_tokens` in `estimates` mode against the URL. One call gives you the full extracted size and two summary sizes, with no API spend.
2. Decide. If the page fits, fetch it as-is. If it's close, fetch with a `max_tokens` budget. If it's far over, summarise deliberately to the size you want.
3. Fetch. Pull the page with the choice you made: full size, capped to a ceiling, or pre-summarised.

The numbers come from the same offline extractive backend whether you estimate, cap, or summarise, so the size you see at step 1 is the size you act on at step 3. A second fetch of a page you already estimated reuses the stored copy instead of paying the round-trip again; see [Caching & freshness](/docs/caching).

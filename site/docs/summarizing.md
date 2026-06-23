---
id: summarizing
title: Summarising pages
---

# Summarising pages

**A summary is a shorter page that still answers the question.** Rover fetches the full document, compresses it, and hands the model a focused version instead of the whole thing — so your [token budget](/docs/token-budgets) goes toward reasoning, not toward scrolling past a navigation column. The defaults produce something sensible with no configuration. The arguments are there for when "sensible" isn't specific enough.

## Two ways to summarise

Reach for the standalone `summarize` tool when you already know you want a short version. It cache-or-fetches the URL, then summarises the result — the same fetch path as a normal request, with compression on the end. Everything except `url` is optional, and the defaults come from your `[summarization]` config.

```json
{
  "url": "https://example.com/long-report",
  "mode": "abstractive",
  "target_tokens": 500,
  "focus": "the security implications"
}
```

Use the inline `summarize` arg on `fetch` when you want extract-and-compress in one call. The returned document *is* the summary: the response sets `summarized: true`, and the `markdown` body is the compressed version, not the original. Same arguments, same backends — one round trip instead of two. See [MCP tools](/docs/mcp-tools) for the full `fetch` and `summarize` schemas.

The third path isn't a tool you call. When a body comes back over budget, `max_tokens` auto-summarises it down to fit — Rover replaces the body with a budget-sized summary and marks the frontmatter `summarized: true`. That's documented under [Managing token budgets](/docs/token-budgets); the steering arguments below apply there too.

## Modes

`mode` decides how the summary gets made, and the three modes are genuinely different operations — not three flavours of the same one.

| `mode` | What it does | Reach for it when |
| --- | --- | --- |
| `extractive` | Selects the highest-ranked sentences straight from the source via a TextRank-flavoured ranker. Offline — no API key, no network. | You want speed, determinism, or zero external calls, and verbatim source wording is fine. |
| `abstractive` | Has a model rewrite the content into new prose. | You want something that reads like a written summary, not a sentence collage. |
| `headlines` | Produces an ultra-short digest — the gist, nothing more. | You want a one-glance answer to "what is this page about." |

Default mode comes from `[summarization] default_mode` (`abstractive`). Extractive is the cheap, always-available option; it can't follow a `focus` instruction the way a model can, because it isn't rewriting anything — it's ranking and selecting what's already there.

## Steering the summary

Four arguments steer the result, and they work the same way on every backend. Set the ones that matter and leave the rest to the defaults.

| Argument | Effect |
| --- | --- |
| `target_tokens` | A length hint, not a hard cap. The summariser aims for roughly this size; it won't truncate mid-thought to hit an exact number. |
| `focus` | Free-text steer threaded into the summariser prompt — `"focus on the breaking changes"`, `"focus on pricing"`. The model weights toward what you name. |
| `preserve` | An array of sections kept verbatim instead of being compressed away. Any of `code`, `tables`, `quotes`, `lists`. |
| `style` | `bullet`, `prose`, or `executive`. Default from `[summarization] default_style` (`prose`). |

`focus` is the lever most people underuse. A page about a library release covers the changelog, the migration notes, the install steps, and the contributors — `focus` is how you tell Rover which of those you actually came for. Pair it with `preserve: ["code"]` on a tutorial and you get the prose compressed while every snippet survives intact. Compress the explanation, keep the thing you'd have copied anyway.

One caveat on `focus`: it steers backends that rewrite. Extractive ranks and selects, so `focus` and `style` land softly there — if you're steering hard, you're steering a model.

## Choosing a backend

`backend` names a `[backends.<name>]` block and picks who does the work for this one call. Two kinds are always in play: **extractive** runs offline in-process and is always available, and **cloud** calls a hosted LLM. Leave `backend` off and Rover uses `[summarization] default_backend`.

The split is the usual trade. Extractive is free, deterministic, and needs no network — and it can't paraphrase. Cloud reads better and follows `focus` and `style`, at the cost of an API call and a key. The full provider list, config keys, and per-provider env vars live on the [Backends](/docs/backends) page; this page is about steering whichever one you point at.

The `tokenizer` argument is orthogonal to all of this. It only sets which tokeniser family counts the resulting summary — `target_tokens` and the reported count are measured against it. It doesn't change what the summary says.

## When a backend fails

A failing cloud call doesn't have to fail your request. With `[summarization] fallback_to_extractive = true` — the default — an auth error, a rate limit, a model error, or an invalid request retries transparently on an extractive backend, and the response is tagged so you know it happened:

```json
{
  "summarizer_fallback": {
    "from": "anthropic",
    "reason": "rate_limited"
  }
}
```

You still get a summary; it's just the offline one, and `summarizer_fallback` tells you the cloud result you asked for isn't what came back. Set `fallback_to_extractive = false` if you'd rather the call fail loudly than quietly downgrade — strict-error mode is the right default for a pipeline that treats a missing cloud summary as a real failure. Configure it in [Configuration](/docs/configuration).

## From the CLI

The CLI runs the same path. `--summarize` takes a JSON blob with the same shape as the tool arguments:

```bash
rover fetch --summarize '{"mode":"abstractive","target_tokens":500}' https://example.com/long-report
```

Add a `focus` and a `preserve` the same way you would over MCP:

```bash
rover fetch --summarize '{"focus":"the security implications","preserve":["code"]}' https://example.com/advisory
```

Same modes, same backends, same fallback behaviour. The transport changes; the summary doesn't.

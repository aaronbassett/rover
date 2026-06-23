---
id: summarizing
title: Summarising pages
---

# Summarising pages

`summarize` compresses a page to a shorter version on a backend you choose. The defaults need no configuration; the arguments below steer length, focus, format, and backend. Summaries count against your [token budget](/docs/token-budgets).

## Two ways to summarise

The standalone `summarize` tool cache-or-fetches the URL, then summarises the result — the normal fetch path with compression on the end. Everything except `url` is optional; the defaults come from your `[summarization]` config.

```json
{
  "url": "https://example.com/long-report",
  "mode": "abstractive",
  "target_tokens": 500,
  "focus": "the security implications"
}
```

The inline `summarize` arg on `fetch` does extract-and-compress in one call. The returned document *is* the summary: the response sets `summarized: true`, and the `markdown` body is the compressed version. Same arguments, same backends, one round trip instead of two. See [MCP tools](/docs/mcp-tools) for the full `fetch` and `summarize` schemas.

When a body comes back over budget, `max_tokens` auto-summarises it down to fit and marks the frontmatter `summarized: true`. That's documented under [Managing token budgets](/docs/token-budgets); the steering arguments below apply there too.

## Modes

`mode` selects how the summary is produced.

| `mode` | What it does | Reach for it when |
| --- | --- | --- |
| `extractive` | Selects the highest-ranked sentences straight from the source via a TextRank-flavoured ranker. Offline — no API key, no network. | You want speed, determinism, or zero external calls, and verbatim source wording is fine. |
| `abstractive` | Has a model rewrite the content into new prose. | You want something that reads like a written summary, not a sentence collage. |
| `headlines` | Produces an ultra-short digest — the gist, nothing more. | You want a one-glance answer to "what is this page about." |

Default mode comes from `[summarization] default_mode` (`abstractive`). Extractive can't follow a `focus` instruction the way a model can — it ranks and selects what's already there, it isn't rewriting anything.

## Steering the summary

Four arguments steer the result, and they work the same way on every backend. Set the ones that matter and leave the rest to the defaults.

| Argument | Effect |
| --- | --- |
| `target_tokens` | A length hint, not a hard cap. The summariser aims for roughly this size; it won't truncate mid-thought to hit an exact number. |
| `focus` | Free-text steer threaded into the summariser prompt — `"focus on the breaking changes"`, `"focus on pricing"`. The model weights toward what you name. |
| `preserve` | An array of sections kept verbatim instead of being compressed away. Any of `code`, `tables`, `quotes`, `lists`. |
| `style` | `bullet`, `prose`, or `executive`. Default from `[summarization] default_style` (`prose`). |

A library release covers the changelog, migration notes, install steps, and contributors; `focus` tells Rover which you came for. Pair it with `preserve: ["code"]` on a tutorial to compress the prose while keeping every snippet intact.

`focus` and `style` only affect backends that rewrite. Extractive ranks and selects existing sentences, so neither changes much there.

## Choosing a backend

`backend` names a `[backends.<name>]` block and picks who does the work for this one call. **Extractive** runs offline in-process and is always available; **cloud** calls a hosted LLM. Leave `backend` off and Rover uses `[summarization] default_backend`.

Extractive is free, deterministic, and offline, and can't paraphrase. Cloud reads better and follows `focus` and `style`, at the cost of an API call and a key. The provider list, config keys, and per-provider env vars are on the [Backends](/docs/backends) page.

The `tokenizer` argument is orthogonal. It only sets which tokeniser family counts the resulting summary — `target_tokens` and the reported count are measured against it. It doesn't change what the summary says.

## When a backend fails

A failing cloud call doesn't have to fail your request. With `[summarization] fallback_to_extractive = true` — the default — an auth error, a rate limit, a model error, or an invalid request retries transparently on an extractive backend, and the response is tagged:

```json
{
  "summarizer_fallback": {
    "from": "anthropic",
    "reason": "rate_limited"
  }
}
```

You still get a summary — the offline one — and `summarizer_fallback` records the cloud backend that failed. Set `fallback_to_extractive = false` to make the call fail loudly instead of downgrading. Configure it in [Configuration](/docs/configuration).

## From the CLI

The CLI runs the same path. `--summarize` takes a JSON blob with the same shape as the tool arguments:

```bash
rover fetch --summarize '{"mode":"abstractive","target_tokens":500}' https://example.com/long-report
```

Add a `focus` and a `preserve` the same way you would over MCP:

```bash
rover fetch --summarize '{"focus":"the security implications","preserve":["code"]}' https://example.com/advisory
```

Same modes, same backends, same fallback behaviour.

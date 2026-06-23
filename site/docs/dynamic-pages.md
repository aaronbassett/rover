---
id: dynamic-pages
title: JavaScript & dynamic pages
---

# JavaScript & dynamic pages

**A JavaScript-rendered page hands a plain HTTP client an empty shell.** The server returns a near-empty root `<div>`, the real content renders later in JavaScript, and a plain fetch never runs that JavaScript. Readability finds nothing worth keeping and the `extraction_quality` in the [response envelope](/docs/output) comes back low. The fix is headless rendering — driving a real Chrome/Chromium over the DevTools Protocol so the page's scripts run before Rover extracts. The model reads the page a human would have seen, not the loading state.

## When you need it

Reach for headless when a fetch comes back thin and `extraction_quality` is low on a page you know has content. Single-page apps are the usual culprit: a React, Vue, or Svelte front end that ships an empty document and paints everything client-side. If the same URL returns a full article to a browser but a stub to Rover, the JavaScript never ran. That is the gap headless closes.

Headless is slower and heavier than the HTTP path — it launches a browser. Use it where the HTTP path genuinely fails, not as a default for every fetch.

## Turning it on

Headless rendering ships behind the **`headless` Cargo feature**, and it is not in the default build. The prebuilt binary and the Homebrew formula already include it, so a standard install needs nothing extra. From source, ask for it explicitly:

```sh
cargo install rover-fetch --features headless
```

The feature needs a Chrome or Chromium browser on the host. Rover auto-detects it on standard install paths; override the path with `chrome_executable` in the `[headless]` config block when the browser lives somewhere unusual. To confirm the launch path resolves before you depend on it, run:

```sh
rover doctor
```

When the feature is compiled, `doctor` verifies that Rover can actually launch the browser — not just that the binary exists on disk. See [Installation](/docs/install) for the install paths and [Optional features](/docs/features) for the full feature list.

## Per-call control

The MCP `fetch` tool takes a `headless` argument that decides, per call, whether to render. Three modes:

```json
{
  "url": "https://example.com/app",
  "headless": { "mode": "auto", "wait": "networkidle0", "timeout_secs": 20 }
}
```

| `mode` | Behaviour |
| ------ | --------- |
| `off` | HTTP path only. No browser launches. |
| `on` | Always render via headless, regardless of what the HTTP path returns. |
| `auto` | Try the HTTP path first; re-render via headless only if the SPA heuristics fire. |

The default mode comes from the `[headless] auto_detect_spa` config key — `auto` when it is `true` (the default), `off` when it is `false`. Set `wait` to choose a render-complete condition (below) and `timeout_secs` to bound the render. See [MCP tools](/docs/mcp-tools) for the full `fetch` argument shape.

**Behaviour without the feature is deliberate, not accidental.** When the `headless` feature is not compiled in, `mode: "off"` and an absent argument are no-ops — the HTTP path was all you were going to get anyway. `mode: "on"` returns the `headless_feature_not_compiled` error, because you asked for rendering the binary can't do and a silent fallback would hide that. `mode: "auto"` keeps the HTTP result with no error, since auto only ever promised to render *if it could*.

## Choosing a wait condition

The `wait` condition decides when Rover calls a render done, and the right answer depends on how the page loads its content. Two values:

- `domcontentloaded` (the default) returns as soon as the initial HTML is parsed. Fast, and correct for pages that render their content inline during the first paint.
- `networkidle0` waits for `domcontentloaded` and then until the network settles — zero requests in flight for a continuous 500 ms, bounded by the render timeout. Slower, but the right choice for SPAs that fetch their content over XHR after load.

The split is about timing. If a page paints a skeleton, then issues an XHR, then fills in the real content, `domcontentloaded` returns the skeleton and `networkidle0` waits for the data. A single pending request still blocks completion, so `networkidle0` costs you the slow tail — pay it only when the content arrives after load.

## From the CLI

`rover fetch` has no `--headless` flag. The one-shot CLI opts into rendering through config alone: set `auto_detect_spa = true` in the `[headless]` block, and Auto mode applies to every CLI fetch. Chromium launches lazily — only when the SPA heuristics fire on a given page — so a CLI run over static pages never pays the browser cost. The toggle lives in config because the CLI is meant to stay flag-light; the [Configuration](/docs/configuration) page documents the full `[headless]` block.

## What gets blocked, and why

While rendering, Rover blocks most subresources to keep renders fast and the request surface tight. By default it blocks images, fonts, media, third-party requests, and service workers — but not CSS, which some SPAs need to render at all. A page that depends on a blocked subresource for its layout still produces text, because Rover is after the content, not a pixel-perfect screenshot. The block flags, the per-render timeout, and `max_concurrent` (default 4) are all in the `[headless]` block on the [Configuration](/docs/configuration) page.

## Security

Every subresource the browser would issue is re-validated against the active SSRF policy before it leaves the renderer. A subrequest that would violate the policy never reaches the network. It is fulfilled with an empty `200` rather than aborted — aborting a request breaks rendering on many SPAs, and an empty success keeps the page running while denying the fetch. So a malicious page cannot use the renderer as a proxy into internal networks the way a naive headless setup would let it. The renderer is held to the same SSRF boundary as the HTTP path; full detail is on the [Security & threat model](/docs/security) page.

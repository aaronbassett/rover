---
id: dynamic-pages
title: JavaScript & dynamic pages
---

# JavaScript & dynamic pages

A JavaScript-rendered page hands a plain HTTP client an empty shell: the server returns a near-empty root `<div>`, the content paints later in JavaScript, and a plain fetch never runs it. Readability keeps nothing, and `extraction_quality` in the [response envelope](/docs/output) comes back low. Headless rendering fixes this. It drives a real Chrome/Chromium over the DevTools Protocol, runs the page's scripts, then hands the rendered DOM to extraction.

## When you need it

Reach for headless when a fetch comes back thin and `extraction_quality` is low on a page you know has content. Single-page apps are the usual culprit. A React, Vue, or Svelte front end ships an empty document and paints everything client-side, so the same URL that returns a full article to a browser returns a stub to Rover. That gap means the JavaScript never ran.

Headless launches a browser, so it's slower and heavier than the HTTP path. Use it where the HTTP path fails, not as a default for every fetch.

## Turning it on

Headless rendering ships behind the `headless` Cargo feature and is not in the default build. The prebuilt binary and the Homebrew formula include it, so a standard install needs nothing extra. From source, ask for it explicitly:

```sh
cargo install rover-fetch --features headless
```

The feature needs a Chrome or Chromium browser on the host. Rover auto-detects it on standard install paths. If the browser lives elsewhere, point `chrome_executable` in the `[headless]` config block at it. To confirm the launch path resolves:

```sh
rover doctor
```

When the feature is compiled, `doctor` launches the browser to verify it works, rather than only checking the binary exists on disk. See [Installation](/docs/install) for install paths and [Optional features](/docs/features) for the full feature list.

## Per-call control

The MCP `fetch` tool takes a `headless` argument that decides per call whether to render. There are three modes:

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

The default mode comes from the `headless.auto_detect_spa` config key: `auto` when `true` (the default), `off` when `false`. Use `wait` to pick a render-complete condition (covered below) and `timeout_secs` to bound the render. See [MCP tools](/docs/mcp-tools) for the full `fetch` argument shape.

Without the feature compiled in, each mode behaves as follows. `mode: "off"` and an absent argument are no-ops, since the HTTP path is all you get anyway. `mode: "on"` returns the `headless_feature_not_compiled` error: you asked for rendering the binary can't do, and a silent fallback would hide that. `mode: "auto"` keeps the HTTP result with no error, because auto only ever promised to render if it could.

## Choosing a wait condition

The `wait` condition decides when Rover calls a render done. The right value depends on how the page loads its content. There are two:

- `domcontentloaded` (the default) returns as soon as the initial HTML is parsed. It's fast, and correct for pages that render their content inline during the first paint.
- `networkidle0` waits for `domcontentloaded`, then until the network settles: zero requests in flight for a continuous 500 ms, bounded by the render timeout. It's slower, but right for SPAs that fetch their content over XHR after load.

The difference is timing. A page that paints a skeleton, issues an XHR, then fills in the real content returns the skeleton under `domcontentloaded` and the full data under `networkidle0`. A single pending request blocks completion, so `networkidle0` costs you the slow tail. Pay it only when the content arrives after load.

## From the CLI

`rover fetch` has no `--headless` flag. The one-shot CLI opts into rendering through config alone: set `auto_detect_spa = true` in the `[headless]` block, and Auto mode applies to every CLI fetch. Chromium launches lazily, only when the SPA heuristics fire (or a bot-protection challenge is detected), so a CLI run over static pages never pays the browser cost. A configurable `launch_delay_secs` (default `2`) pauses between that detection and the browser launch; set it to `0` to escalate immediately. The [Configuration](/docs/configuration) page documents the full `[headless]` block.

## What gets blocked, and why

While rendering, Rover blocks most subresources to keep renders fast and the request surface tight. The defaults block images, fonts, media, third-party requests, and service workers. CSS is not blocked, because some SPAs won't render without it. A page that needs a blocked subresource for its layout still produces text; Rover is after the content, not a pixel-perfect screenshot. The block flags, the per-render timeout, and `max_concurrent` (default 4) all live in the `[headless]` block on the [Configuration](/docs/configuration) page.

## Security

Every subresource the browser would issue is re-validated against the active SSRF policy before it leaves the renderer. A subrequest that would violate the policy never reaches the network. It's fulfilled with an empty `200` rather than aborted, because aborting breaks rendering on many SPAs while an empty success keeps the page running and still denies the fetch. A malicious page can't use the renderer as a proxy into internal networks. The renderer is held to the same SSRF boundary as the HTTP path. Full detail is on the [Security & threat model](/docs/security) page.

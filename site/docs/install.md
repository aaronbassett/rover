---
id: install
title: Installation
---

# Installation

Four channels: Homebrew, a prebuilt binary, Cargo, or a container. The first three install a binary named `rover`; the container builds an image instead.

## Homebrew (macOS)

```sh
brew install aaronbassett/tap/rover
```

Ships with both `headless` and `web-search` compiled in. It pulls in no browser — headless rendering is opt-in and Rover auto-detects a Chrome/Chromium install at runtime (`rover doctor` verifies it). For headless mode, install a browser yourself, e.g. `brew install --cask chromium`. Web search needs no extra software, just an API key: see [Web search](/docs/web-search).

## Prebuilt binary (Linux & macOS)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/aaronbassett/rover/releases/latest/download/rover-fetch-installer.sh | sh
```

Or grab a `.tar.xz` from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify its checksum, and move `rover` onto your `PATH`. Targets are `x86_64` and `aarch64` for Linux (gnu) and macOS, and all of them include both the `headless` and `web-search` features. Windows is unsupported.

## With cargo

```sh
cargo install rover-fetch
```

The crate is `rover-fetch` (the name `rover` was taken); the installed binary is `rover`. This builds the default binary: no web search, no Chrome, no model downloads. Turn on Cargo features for more (see [Optional features](/docs/features)):

```sh
cargo install rover-fetch --features web-search              # add web search
cargo install rover-fetch --features headless,web-search     # what the prebuilt binaries ship
```

`cargo install` is the one channel where the features are yours to choose — every packaged channel above already includes both. Users of those never need to know the flag exists.

For the latest unreleased code, install from the repo:

```sh
cargo install --git https://github.com/aaronbassett/rover --locked
```

Requires Rust 1.96+.

## Container

The repository's `Dockerfile` builds an image instead of a binary on `PATH`. Both targets include `web-search` — it needs nothing at runtime, so there is no reason for a container deployment to lack it. The default target ships without Chromium; a separate `runtime-headless` target adds it:

```sh
docker build --target runtime-headless -t rover:headless .
```

See [Deployment](/docs/deployment#spa-rendering) for the run flags Chrome's sandbox needs and for running Rover as a shared container on the network.

## Verify

```sh
rover doctor
```

This checks that the cache database opens, the network is reachable, the extractive backend works, any configured cloud backends authenticate, and whether web search is available. With the relevant features built, it also checks the headless browser and local models. For the full subcommand list, run `rover --help`.

The `web_search` line reports one of three states, and none of the unavailable ones is a failure — Rover fetches fine without search:

```text
- web_search feature not compiled (build with `--features web-search`, or use a prebuilt binary)
- web_search compiled but not configured (no API key in $BRAVE_SEARCH_API_KEY); fetching is unaffected
✓ web_search configured via $BRAVE_SEARCH_API_KEY (not probed — a live check is a billable request; run `rover search "rover mcp" -n 1` to verify)
```

Next: [Quickstart](/docs/quickstart).

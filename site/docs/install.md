---
id: install
title: Installation
---

# Installation

**Install with Homebrew, a prebuilt binary, or from source.** Every channel installs a binary named `rover`. Current release: `v0.1.0`.

## Homebrew (macOS)

```sh
brew install aaronbassett/tap/rover
```

Ships the `headless` build and depends on Chromium.

## Prebuilt binary (Linux & macOS)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/aaronbassett/rover/releases/latest/download/rover-fetch-installer.sh | sh
```

Or download a `.tar.xz` from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify its checksum, and move `rover` onto your `PATH`. Targets: `x86_64` and `aarch64`, Linux (gnu) and macOS. Includes the `headless` feature. Windows is unsupported.

## With cargo

```sh
cargo install rover-fetch
```

The crate is `rover-fetch` (the name `rover` was taken); the installed binary is `rover`. This builds the default binary — no Chrome, no model downloads. Add Cargo features for more (see [Optional features](/docs/features)):

```sh
cargo install rover-fetch --features headless
```

To build the latest unreleased code, install from the repo instead:

```sh
cargo install --git https://github.com/aaronbassett/rover --locked
```

Requires Rust 1.96+.

## Verify

```sh
rover doctor
```

Confirms the cache database opens, the network is reachable, the extractive backend works, and configured cloud backends authenticate — plus the headless browser and any local models when those features are built. `rover --help` lists every subcommand.

Next: [Quickstart](/docs/quickstart).

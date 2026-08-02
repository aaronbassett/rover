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

Ships the `headless` build but pulls in no browser — headless rendering is opt-in and Rover auto-detects a Chrome/Chromium install at runtime (`rover doctor` verifies it). For headless mode, install a browser yourself, e.g. `brew install --cask chromium`.

## Prebuilt binary (Linux & macOS)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/aaronbassett/rover/releases/latest/download/rover-fetch-installer.sh | sh
```

Or grab a `.tar.xz` from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify its checksum, and move `rover` onto your `PATH`. Targets are `x86_64` and `aarch64` for Linux (gnu) and macOS, and both include the `headless` feature. Windows is unsupported.

## With cargo

```sh
cargo install rover-fetch
```

The crate is `rover-fetch` (the name `rover` was taken); the installed binary is `rover`. This builds the default binary: no Chrome, no model downloads. Turn on Cargo features for more (see [Optional features](/docs/features)):

```sh
cargo install rover-fetch --features headless
```

For the latest unreleased code, install from the repo:

```sh
cargo install --git https://github.com/aaronbassett/rover --locked
```

Requires Rust 1.96+.

## Container

The repository's `Dockerfile` builds an image instead of a binary on `PATH`. The default target ships without Chromium, matching the default Cargo build; a separate `runtime-headless` target adds it:

```sh
docker build --target runtime-headless -t rover:headless .
```

See [Deployment](/docs/deployment#spa-rendering) for the run flags Chrome's sandbox needs and for running Rover as a shared container on the network.

## Verify

```sh
rover doctor
```

This checks that the cache database opens, the network is reachable, the extractive backend works, and any configured cloud backends authenticate. With the relevant features built, it also checks the headless browser and local models. For the full subcommand list, run `rover --help`.

Next: [Quickstart](/docs/quickstart).

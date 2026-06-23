---
id: install
title: Installation
---

# Install Rover

**Build from source today; switch to a packaged channel once the first release lands.** Rover is pre-1.0 (`0.1.0`), and only the build-from-source path is live right now. The packaged channels — Homebrew tap, prebuilt tarballs, crates.io — come online with the first tagged release. Every channel installs the same binary, named `rover`.

:::note
Building from source is the path that works today. The commands under Homebrew, prebuilt binary, and crates.io are accurate for the first release; until then, use **Build from source** below.
:::

## Build from source

`cargo install` from the Git repo is the fastest way to a working `rover` on any supported platform:

```sh
cargo install --git https://github.com/aaronbassett/rover --locked
```

Prefer to clone and build it yourself — to hack on the source, or to pin a checkout:

```sh
git clone https://github.com/aaronbassett/rover && cd rover
cargo build --release          # binary at target/release/rover
```

The default build is lean — around 20 MiB, with CI enforcing a hard ceiling under 75 MiB on the default-features binary. It needs no model downloads, no Chrome, and no extra runtime dependencies. That gets you the core fetch-and-extract surface. JavaScript rendering, local inference, and the ONNX injection classifier are opt-in Cargo features that add capability and size; see [Optional features](/docs/features) before you compile them in.

## Homebrew (macOS) — on release

```sh
brew install aaronbassett/tap/rover
```

The `rover` formula ships the JavaScript-rendering (`headless`) build and `depends_on "chromium"`, so a fetch against a single-page app works out of the box. For other optional features, install from source with `cargo install` or from crates.io.

## Prebuilt binary (Linux & macOS) — on release

The one-line installer downloads the right artifact for your platform, verifies it, and drops `rover` onto your `PATH`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/aaronbassett/rover/releases/latest/download/rover-fetch-installer.sh | sh
```

To install by hand, download a `.tar.xz` from the [latest release](https://github.com/aaronbassett/rover/releases/latest), verify its checksum, then extract and move the `rover` binary onto your `PATH`:

```sh
tar xf rover-fetch-<target>.tar.xz   # then move the extracted `rover` onto your PATH
```

Targets cover `x86_64` and `aarch64` Linux (gnu) plus Intel and Apple-Silicon macOS. The prebuilt binary includes the `headless` feature. Windows is out of scope.

## crates.io — on release

```sh
cargo install rover-fetch --features headless
```

The crate's default feature set is empty, so a plain `cargo install rover-fetch` builds the *basic* binary. Add `--features headless` to match what the prebuilt and Homebrew channels ship.

:::caution
The crate publishes as `rover-fetch`, not `rover` — the `rover` name on crates.io is held by an unrelated project. The installed binary is still `rover`. Install `rover-fetch`; run `rover`.
:::

## Requirements

Rover needs **Rust 1.96+** (edition 2024) to build from source. The prebuilt and Homebrew channels carry no toolchain requirement — they ship a compiled binary. The full stability and MSRV policy lives on the [Versioning & stability](/docs/versioning) page.

## Verify it worked

Run `rover doctor` once the binary is on your `PATH`:

```sh
rover doctor
```

`doctor` runs the health checks that matter before your agent depends on them: the cache database opens, the network is reachable, the extractive backend works, and any configured cloud backends authenticate. When the matching features are compiled in, it also confirms the headless browser launches and that local and injection models are present. A clean run means the install is sound.

For the full subcommand surface — every command and its flags — run:

```sh
rover --help
```

## Next steps

Wire the binary into your agent with the [Quickstart](/docs/quickstart): one command adds Rover as an MCP server, and the same binary doubles as a one-shot CLI. To add JavaScript rendering, local summarisation, or the ONNX injection classifier, read [Optional features](/docs/features) — each is a Cargo feature with its own size and runtime cost.

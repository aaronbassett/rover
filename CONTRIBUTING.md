# Contributing to Rover

Thanks for your interest. This document covers the dev environment setup and
the automated quality checks every change runs through.

## Prerequisites

- Rust stable (MSRV 1.96, edition 2024). Install via [rustup](https://rustup.rs).
- [Lefthook](https://lefthook.dev) for git hooks. The simplest install is:
  - macOS: `brew install lefthook`
  - Linux/macOS via curl: `curl -1sLf 'https://lefthook.dev/install.sh' | sudo sh`
  - Other platforms: see https://lefthook.dev
- [`commit-check`](https://crates.io/crates/conventional-commits-check) for
  Conventional Commits validation: `cargo install conventional-commits-check`
  (binary name is `commit-check`).

## One-time setup in your clone

```sh
lefthook install
```

This activates the pre-commit, pre-push, and commit-msg hooks.

## What the hooks do

### pre-commit (fast, only staged files)

- `rustfmt --check` on staged `.rs` files.
- `cargo clippy --all-targets --features test-loopback -- -D warnings` when
  any `.rs`/`.toml` is staged.

### pre-push (full project)

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --features test-loopback -- -D warnings`
- `cargo clippy --all-targets --no-default-features --features headless,web-search,test-loopback -- -D warnings`
  — the exact feature set the release binaries are built with, so a
  combination that compiles alone but not together is caught before the push
  rather than in the release job.
- `cargo test --lib --features test-loopback`
- `cargo test --lib --features test-loopback,web-search` — several unit tests
  are cfg-aware and assert the *feature-disabled* behaviour, so both shapes
  have to run.
- `cargo build --release`

All warnings are treated as errors. The Cargo.toml `[lints]` table sets
`warnings = "deny"` for both rustc and clippy crate-wide.

### commit-msg

- `commit-check` validates the message against the [Conventional Commits](https://www.conventionalcommits.org)
  spec with a max description length of 500 characters.

## CI parity

`.github/workflows/ci.yml` runs the same checks as `pre-push` on every PR and
on pushes to `main`, across `ubuntu-latest` and `macos-latest`.

## Running the checks manually

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --features test-loopback -- -D warnings
cargo test --features test-loopback
cargo build --release
```

### Feature combinations

Optional features change what compiles, so a change that touches a
feature-gated path needs checking in more than one shape. The combinations CI
covers:

```sh
cargo test  --features test-loopback                              # default
cargo test  --features test-loopback,web-search                   # + search
cargo check --all-targets --no-default-features \
            --features headless,web-search                        # the release set
cargo check --all-targets --features test-loopback,headless,local-inference,injection-model
```

`web-search` in particular must be exercised **both ways**: the suites assert
the working behaviour with it and the feature-disabled behaviour without it
(`tests/cli_search.rs`, `tests/meta_hook.rs`, `tests/meta_use.rs`, and the
`search::` unit tests all branch on `cfg!(feature = "web-search")`).

### Search tests never call the real API

Every search test drives a `wiremock` server via `[search] base_url`, with a
throwaway credential in a test-specific environment variable. The live Brave
API is metered and billed; nothing in the suite may touch it. If you add a
search test, point it at a mock and give it its own env-var name so parallel
tests do not fight over one.

Or run the lefthook stages directly:

```sh
lefthook run pre-commit
lefthook run pre-push
```

## Tooling philosophy

Project tooling stays Rust-native where possible (rustfmt, clippy, cargo
test/build, `commit-check`). Lefthook itself is the only external dependency.

## Skipping hooks (don't, except in emergencies)

If you genuinely need to bypass hooks (e.g., a fixup commit that intentionally
contains a known issue you'll resolve in a follow-up commit), prefix with
`LEFTHOOK=0`:

```sh
LEFTHOOK=0 git commit -m "wip: …"
```

CI on the PR will still enforce the same checks, so this only buys you a local
shortcut.

# Contributing to Rover

Thanks for your interest. This document covers the dev environment setup and
the automated quality checks every change runs through.

## Prerequisites

- Rust stable (MSRV 1.85, edition 2024). Install via [rustup](https://rustup.rs).
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
- `cargo test --features test-loopback`
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

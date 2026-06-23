---
id: versioning
title: Versioning & stability
---

# Versioning & stability

**Rover is pre-1.0 and unstable at the minor level — pin to an exact version in any agent harness.** A minor bump can break the MCP tool schema or the CLI flag set, so anything that depends on Rover behaving the same way tomorrow needs a pinned version and a read of the [CHANGELOG](https://github.com/aaronbassett/rover/blob/main/CHANGELOG.md) before you move it. Rover follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html), with the pre-1.0 caveats below.

## What you get on each release type (pre-1.0)

While Rover is `0.y.z`, the version number tells you how much can change:

- **Minor** versions (`0.MINOR.z`) may break the MCP tool schema or the CLI flag set. Pin to an exact version and read the [CHANGELOG](https://github.com/aaronbassett/rover/blob/main/CHANGELOG.md) before bumping the minor.
- **Patch** versions (`0.y.PATCH`) are bug-fix-only. They will not change the MCP tool schema or remove or rename CLI flags.
- **Pre-release** versions (`-rc.N`, `-beta.N`) are published to crates.io and GitHub Releases for feedback but do **not** update the Homebrew formula. Treat them as more volatile than a plain patch.

If you install through Homebrew, you stay on stable releases by default — see [Installation](/docs/install). Pre-releases are an opt-in you pull from crates.io or GitHub Releases.

## 1.0.0 and beyond

The first `1.0.0` locks the MCP tool schema. After that, normal SemVer applies:

- **Major** — breaking changes to the MCP tool schema or CLI surface.
- **Minor** — backwards-compatible additions: new tools, new flags, new config.
- **Patch** — backwards-compatible bug fixes.

## What "the schema" means

The stability surface that major and minor govern is three things:

- the set of [MCP tools](/docs/mcp-tools), their input arguments, and the shape of their results;
- the `rover` [CLI](/docs/cli) subcommands and their flags;
- the [configuration](/docs/configuration) file keys and their accepted values.

Internal Rust APIs (the `rover` library crate) carry **no** stability guarantee at any version. Rover is distributed as a binary, not as a library dependency — so the library surface is free to change underneath you without a version bump.

## Minimum supported Rust version

The MSRV is declared by `rust-version` in `Cargo.toml` — Rust 1.96 or newer, on edition 2024 — and verified in CI. **An MSRV bump is a minor-version change.** It will never happen in a patch release.

## Related

- [Installation](/docs/install) — stable releases through Homebrew; pre-releases from crates.io and GitHub Releases.
- [Releasing](/docs/releasing) — how versions are cut and published.
- [MCP tools](/docs/mcp-tools), [CLI](/docs/cli), and [Configuration](/docs/configuration) — the three surfaces this policy governs.

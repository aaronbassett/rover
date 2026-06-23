---
id: versioning
title: Versioning & stability
---

# Versioning & stability

Rover is pre-1.0 and unstable at the minor level. Pin to an exact version in any agent harness: a minor bump can break the MCP tool schema or the CLI flag set. Read the [CHANGELOG](https://github.com/aaronbassett/rover/blob/main/CHANGELOG.md) before you bump. Rover follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html), with the pre-1.0 rules below.

## Release types (pre-1.0)

While Rover is `0.y.z`, the version number tells you how much can change.

A minor bump (`0.MINOR.z`) may break the MCP tool schema or the CLI flag set. Pin to an exact version and read the [CHANGELOG](https://github.com/aaronbassett/rover/blob/main/CHANGELOG.md) before bumping.

A patch (`0.y.PATCH`) is bug-fix-only. It will not change the MCP tool schema or remove or rename CLI flags.

A pre-release (`-rc.N`, `-beta.N`) ships to crates.io and GitHub Releases, but does **not** update the Homebrew formula. Treat it as more volatile than a patch.

Homebrew installs stay on stable releases by default. See [Installation](/docs/install). Pre-releases are opt-in from crates.io or GitHub Releases.

## 1.0.0 and beyond

The first `1.0.0` locks the MCP tool schema. After that, normal SemVer applies:

- a major bump means breaking changes to the MCP tool schema or CLI surface;
- a minor bump means backwards-compatible additions: new tools, new flags, new config;
- a patch means backwards-compatible bug fixes.

## What "the schema" covers

Major and minor govern three surfaces:

- the [MCP tools](/docs/mcp-tools), their input arguments, and the shape of their results;
- the `rover` [CLI](/docs/cli) subcommands and their flags;
- the [configuration](/docs/configuration) file keys and their accepted values.

The internal Rust APIs (the `rover` library crate) carry **no** stability guarantee at any version. The library surface can change without a version bump.

## Minimum supported Rust version

The MSRV is Rust 1.96 or newer on edition 2024, declared by `rust-version` in `Cargo.toml` and verified in CI. An MSRV bump is a minor-version change. It never happens in a patch release.

## Related

- [Installation](/docs/install): stable releases through Homebrew; pre-releases from crates.io and GitHub Releases.
- [Releasing](/docs/releasing): how versions are cut and published.
- [MCP tools](/docs/mcp-tools), [CLI](/docs/cli), and [Configuration](/docs/configuration): the three surfaces this policy governs.

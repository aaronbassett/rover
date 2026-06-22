---
id: versioning
title: Versioning
---

# Versioning policy

Rover follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html), with
the pre-1.0 caveats below.

## Pre-1.0 (current)

While Rover is `0.y.z`, it is **unstable**:

- **Minor** versions (`0.MINOR.z`) may break the MCP tool schema or the CLI flag
  set. If you pin Rover in an agent harness, pin to an exact version and read the
  [CHANGELOG](https://github.com/aaronbassett/rover/blob/main/CHANGELOG.md) before bumping the minor.
- **Patch** versions (`0.y.PATCH`) are bug-fix-only. They will not change the MCP
  tool schema or remove/rename CLI flags.
- **Pre-release** versions (`-rc.N`, `-beta.N`) are published to crates.io and the
  GitHub Releases for feedback but do **not** update the Homebrew formula; treat
  them as more volatile than a plain patch.

## 1.0.0 and beyond

The first `1.0.0` locks the MCP tool schema. After that, normal SemVer applies:

- **Major** — breaking changes to the MCP tool schema or CLI surface.
- **Minor** — backwards-compatible additions (new tools, new flags, new config).
- **Patch** — backwards-compatible bug fixes.

## MSRV

The minimum supported Rust version is declared by `rust-version` in
`Cargo.toml` and verified in CI. **An MSRV bump is a minor-version change** — it
will not happen in a patch release.

## What "the schema" means

The stability surface that major/minor governs is:

- the set of MCP tools, their input arguments, and the shape of their results;
- the `rover` CLI subcommands and their flags;
- the configuration file keys and their accepted values.

Internal Rust APIs (the `rover` library crate) carry **no** stability guarantee
at any version — Rover is distributed as a binary, not as a library dependency.

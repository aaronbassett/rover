---
id: releasing
title: Releasing
---

# Releasing

**This page is for maintainers cutting a Rover release, not for people installing it.** It documents how a version goes from a merge on `main` to a published crate, a GitHub Release, and a Homebrew formula. If you only want to run Rover, start with [Installation](/docs/install). For how version numbers and stability promises work, see [Versioning & stability](/docs/versioning).

Two tools do the work, and they split it cleanly:

- **[release-plz](https://release-plz.dev)** owns versions, the changelog, the crates.io publish, and the git tag.
- **[dist](https://opensource.axo.dev/cargo-dist/)** (cargo-dist) owns the cross-platform binaries, the GitHub Release, the `curl | sh` installer, and the Homebrew formula.

A release ships through three channels, all automated:

1. **crates.io** — published as `rover-fetch`; the installed binary is still `rover`.
2. **GitHub Releases** — prebuilt tarballs for 4 targets, SHA-256 checksums, and a shell installer.
3. **Homebrew** — a single `rover` formula in `aaronbassett/homebrew-tap`, built `--features headless`, so it `depends_on "chromium"`.

## How it fits together

The handoff runs in one direction: release-plz publishes and tags, the tag triggers dist, dist builds and uploads.

```text
push to main ──► release-plz opens a "Release PR" (version bump + changelog)
     │
     ▼ merge the Release PR
release-plz: cargo publish rover-fetch ──► crates.io
            push tag vX.Y.Z ─────────────► triggers dist
     │
     ▼
dist: build 4 targets (--features headless) ──► GitHub Release + shell installer
      publish the `rover` formula ───────────► aaronbassett/homebrew-tap
```

release-plz creates the **tag**; dist creates the **GitHub Release**. They never both create the Release because `git_release_enable = false` in `release-plz.toml` turns off release-plz's side. Drop that line and the two tools fight over the same Release.

## The distributed binary vs. `cargo install`

**The prebuilt binary and the one `cargo install` gives you are not the same build.** The tarballs and the Homebrew formula ship `rover` built with `--features headless --no-default-features`. The crate's default feature set is empty (`default = []`), so `cargo install rover-fetch` builds the *basic* binary. To match the distributed binary from source, ask for the feature explicitly:

```sh
cargo install rover-fetch --features headless
```

Other optional features (`local-inference`, and the rest) work the same way. The full list is in [Optional features](/docs/features).

## Targets

dist builds four targets, each on a native runner where it can:

| Target | Runner |
| ------ | ------ |
| `x86_64-unknown-linux-gnu` | `ubuntu` (native) |
| `aarch64-unknown-linux-gnu` | `ubuntu` arm64 (native) |
| `x86_64-apple-darwin` | `macos` |
| `aarch64-apple-darwin` | `macos` (native) |

The one exception is `x86_64-apple-darwin`, which is cross-compiled on the arm64 macOS runner. Windows is out of scope. Targets live in `[workspace.metadata.dist]` in `Cargo.toml`.

## One-time setup

Three repository secrets make the pipeline run (Settings → Secrets and variables → Actions):

| Secret | Used by | Purpose |
| ------ | ------- | ------- |
| `CARGO_REGISTRY_TOKEN` | release-plz | `cargo publish` to crates.io |
| `RELEASE_PLZ_TOKEN` | release-plz | push the tag and open the Release PR |
| `HOMEBREW_TAP_TOKEN` | dist | push the formula to `aaronbassett/homebrew-tap` |

`RELEASE_PLZ_TOKEN` has to be a PAT or App token, not the default `GITHUB_TOKEN` — a tag pushed with `GITHUB_TOKEN` won't trigger the dist workflow, so the release stalls after the publish. It needs `contents: write` plus `pull-requests: write` on this repo. `HOMEBREW_TAP_TOKEN` needs write access to the tap repo, which must already exist with a `Formula/` directory (it can start empty). dist creates the GitHub Release itself with the auto-provided `GITHUB_TOKEN`.

## Cutting a release

Normal releases are hands-off — three steps, and two of them are just review:

1. Land changes on `main` using Conventional Commits (`feat:`, `fix:`, and the rest).
2. release-plz opens or updates a **Release PR** that bumps the version and updates `CHANGELOG.md`. Review it.
3. **Merge the Release PR.** release-plz publishes to crates.io and pushes `vX.Y.Z`; the tag triggers dist, which builds the binaries, creates the GitHub Release, and updates the Homebrew tap.

A pre-release version (`X.Y.Z-rc.1`) publishes to crates.io and GitHub but does *not* update the Homebrew formula. dist skips prereleases on purpose, so the tap always points at the latest stable build.

## The first release (v0.1.0)

`rover-fetch` is unpublished, and `Cargo.toml` is already at `0.1.0` with a curated `## [0.1.0]` changelog section. Once that lands on `main`:

- release-plz detects `0.1.0` is not on crates.io and publishes it. If it opens a Release PR instead, merge that PR to perform the release.
- Confirm the `v0.1.0` tag appears, the GitHub Release is created with the four tarballs, checksums, and installer, and the `rover` formula lands in the tap.

## Post-release validation

Install the release four ways and run `rover --help` each time. If one path is broken, you want to know before a user does:

- `brew install aaronbassett/tap/rover` (macOS arm64 and x86_64).
- Run the shell installer from the Release page.
- `cargo install rover-fetch --features headless`.
- Download a tarball, verify its checksum, and run `./rover --help`.

## Updating dist

dist is pinned via `cargo-dist-version` in `Cargo.toml`. To upgrade it, let dist rewrite its own config and workflow:

```sh
cargo install cargo-dist --locked   # install the new version
dist init --yes                     # re-read config, bump the pin
dist generate                       # regenerate .github/workflows/release.yml
git add Cargo.toml .github/workflows/release.yml && git commit
```

Never hand-edit `.github/workflows/release.yml`. It is generated, and the next `dist generate` will overwrite whatever you put there.

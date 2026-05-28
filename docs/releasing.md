# Releasing

Rover ships through three channels, all driven by one workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml)):

1. **GitHub Releases** — prebuilt tarballs for 4 targets × 5 feature variants,
   plus a `SHA256SUMS` manifest.
2. **Homebrew tap** — `aaronbassett/homebrew-tap`, five formulas, bumped
   automatically on a stable release.
3. **crates.io** — published as `rover-mcp` (the binary is still `rover`).

## Build matrix

| Target | Runner | Notes |
| ------ | ------ | ----- |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | via `cross` |
| `x86_64-apple-darwin` | `macos-latest` | cross-compiled from arm64 |
| `aarch64-apple-darwin` | `macos-latest` | native |

| Variant | Feature flags |
| ------- | ------------- |
| `basic` | `--no-default-features` |
| `local-inference` | `--no-default-features --features local-inference` |
| `local-vision` | `--no-default-features --features local-vision` |
| `headless` | `--no-default-features --features headless` |
| `complete` | `--no-default-features --features local-inference,local-vision,headless` |

Tarballs are named `rover-<version>-<target>-<variant>.tar.gz` and contain the
`rover` binary, both licenses, and a short README.

## One-time setup

Repository secrets (Settings → Secrets and variables → Actions):

- `CRATES_IO_TOKEN` — a crates.io API token with publish scope for `rover-mcp`.
- `HOMEBREW_TAP_GITHUB_TOKEN` — a fine-grained PAT scoped to
  `aaronbassett/homebrew-tap` with `contents: write`.
- `GPG_PRIVATE_KEY` *(optional)* — armored private key; if present, the workflow
  signs `SHA256SUMS` into `SHA256SUMS.asc`. If absent, signing is skipped (a
  documented v2 follow-up).

The tap repo (`aaronbassett/homebrew-tap`) must exist with a `Formula/`
directory. The formulas are generated, so it can start empty.

## Cutting a release

1. Bump `version` in `Cargo.toml`, update `CHANGELOG.md` (move `[Unreleased]`
   into a dated version section), and merge to `main`.
2. Dry-run the build matrix without publishing:
   Actions → **Release** → **Run workflow** (leave `dry_run` checked). This
   builds and uploads all 20 tarballs as run artefacts but touches nothing
   external.
3. Tag and push:
   ```sh
   git tag v0.1.0-alpha.1
   git push origin v0.1.0-alpha.1
   ```
   The tag push runs the full pipeline. A tag containing a hyphen
   (e.g. `-alpha.1`) is published as a **pre-release** and **does not** bump the
   Homebrew tap; a plain tag (e.g. `v0.1.0`) does both.

## Homebrew variants

Homebrew conflict detection means only one rover formula can be installed at a
time; each installs a `rover` binary.

- `rover` — the `basic` variant (default `brew install aaronbassett/tap/rover`).
- `rover-complete` — all features (`depends_on "chromium"`).
- `rover-local-inference`, `rover-local-vision` — single-feature; models
  download on first use, no extra system deps.
- `rover-headless` — `depends_on "chromium"`.

Formulas are rendered by
[`scripts/render-homebrew-formulas.sh`](../scripts/render-homebrew-formulas.sh)
from the release `SHA256SUMS`; the `bump-homebrew` job commits them to the tap.

## Post-release validation

- `brew install aaronbassett/tap/rover` on macOS (arm64 and x86_64).
- Download a tarball from the GitHub Release, verify against `SHA256SUMS`, run
  `./rover --help`.
- `cargo install rover-mcp` once the crates.io publish lands.

## v2 follow-ups

- Sign `SHA256SUMS` with a project GPG key (wire `GPG_PRIVATE_KEY`).
- Consider Linux package formats (deb/rpm) and a Windows target if demand
  appears (currently out of scope).

# Releasing

Rover's releases are automated by two tools:

- **[release-plz](https://release-plz.dev)** — versions, changelog, the crates.io
  publish, and the git tag.
- **[dist](https://opensource.axo.dev/cargo-dist/)** (cargo-dist) — cross-platform
  binaries, the GitHub Release, the `curl | sh` installer, and the Homebrew formula.

Three channels, all automated:

1. **crates.io** — `rover-fetch` (the binary is still `rover`).
2. **GitHub Releases** — prebuilt tarballs for 4 targets, SHA-256 checksums, and a
   shell installer.
3. **Homebrew** — a single `rover` formula in `aaronbassett/homebrew-tap`, built
   `--features headless` (so it `depends_on "chromium"`).

## How it fits together

```
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

release-plz creates the **tag**; dist creates the **GitHub Release**.
`git_release_enable = false` in `release-plz.toml` is what keeps them from
colliding.

## The distributed binary vs. `cargo install`

The prebuilt tarballs and the Homebrew formula ship `rover` built with
`--features headless --no-default-features`. The crate's own default feature set
is empty (`default = []`), so **`cargo install rover-fetch` builds the *basic*
binary**. To match the prebuilt binary from source:

```sh
cargo install rover-fetch --features headless
```

Other optional features (e.g. `local-inference`) work the same way.

## Targets

| Target | Runner |
| ------ | ------ |
| `x86_64-unknown-linux-gnu` | `ubuntu` (native) |
| `aarch64-unknown-linux-gnu` | `ubuntu` arm64 (native) |
| `x86_64-apple-darwin` | `macos` |
| `aarch64-apple-darwin` | `macos` (native) |

cargo-dist builds each target on a native runner, except `x86_64-apple-darwin`, which is cross-compiled on the arm64 macOS runner.

Windows is out of scope. Targets live in `[workspace.metadata.dist]` in
`Cargo.toml`.

## One-time setup

Repository secrets (Settings → Secrets and variables → Actions):

| Secret | Used by | Purpose |
| ------ | ------- | ------- |
| `CARGO_REGISTRY_TOKEN` | release-plz | `cargo publish` to crates.io |
| `RELEASE_PLZ_TOKEN` | release-plz | push the tag (a **PAT/App token**, not the default `GITHUB_TOKEN`, or the tag won't trigger dist) and open the Release PR |
| `HOMEBREW_TAP_TOKEN` | dist | push the formula to `aaronbassett/homebrew-tap` |

dist creates the GitHub Release with the auto-provided `GITHUB_TOKEN`.
`RELEASE_PLZ_TOKEN` needs `contents: write` + `pull-requests: write` on this
repo; `HOMEBREW_TAP_TOKEN` needs write access to the tap repo. The tap repo must
exist with a `Formula/` directory (it may start empty).

## Cutting a release

Normal releases are hands-off:

1. Land changes on `main` using Conventional Commits (`feat:`, `fix:`, …).
2. release-plz opens/updates a **Release PR** that bumps the version and updates
   `CHANGELOG.md`. Review it.
3. **Merge the Release PR.** release-plz publishes to crates.io and pushes
   `vX.Y.Z`; the tag triggers dist, which builds the binaries, creates the GitHub
   Release, and updates the Homebrew tap.

A pre-release version (`X.Y.Z-rc.1`) is published to crates.io and GitHub but
**does not** update the Homebrew formula (dist skips prereleases).

## The first release (v0.1.0)

`rover-fetch` is unpublished, and `Cargo.toml` is already at `0.1.0` with a
curated `## [0.1.0]` changelog section. Once this lands on `main`:

- release-plz detects `0.1.0` is not on crates.io and publishes it. If it instead
  opens a Release PR, merge that PR to perform the release.
- Confirm the `v0.1.0` tag appears, the GitHub Release is created with the four
  tarballs + checksums + installer, and the `rover` formula lands in the tap.

## Post-release validation

- `brew install aaronbassett/tap/rover` (macOS arm64 + x86_64); `rover --help`.
- Run the shell installer from the Release page; `rover --help`.
- `cargo install rover-fetch --features headless`; `rover --help`.
- Download a tarball, verify its checksum, run `./rover --help`.

## Updating dist

dist is pinned via `cargo-dist-version` in `Cargo.toml`. To upgrade:

```sh
cargo install cargo-dist --locked   # install the new version
dist init --yes                     # re-read config, bump the pin
dist generate                       # regenerate .github/workflows/release.yml
git add Cargo.toml .github/workflows/release.yml && git commit
```

Never hand-edit `.github/workflows/release.yml`; it is generated.

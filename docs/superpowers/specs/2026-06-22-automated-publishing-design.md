# Automated publishing: release-plz + cargo-dist

**Status:** approved (design) — pending implementation plan
**Date:** 2026-06-22
**Branch:** `feat/automated-publishing` (off `main`)

## Goal

Replace rover's hand-rolled release pipeline with a standard, low-maintenance
setup built on **release-plz** (version management, changelog, crates.io publish,
tagging) and **cargo-dist / `dist`** (cross-platform binary builds, GitHub
Releases, Homebrew, shell installer). Along the way, rename the crate so it can
actually be published.

Three distribution channels, all automated:

1. **crates.io** — published as `rover-fetch` (the `rover` name is taken by an
   unrelated Cosmos project; `rover-mcp` was the prior placeholder and is not
   yet published).
2. **GitHub Releases** — prebuilt tarballs for 4 targets + SHA256 checksums +
   a `curl | sh` installer.
3. **Homebrew** — a single `rover` formula published to the existing custom tap
   `aaronbassett/homebrew-tap`.

The installed binary remains **`rover`**; the library crate is still imported as
**`rover`**. Only the crates.io package name changes.

## Context: this is a migration, not a greenfield setup

The repo already has a complete hand-rolled pipeline:

- `.github/workflows/release.yml` — builds 4 targets × 4 feature variants
  (`basic`, `local-inference`, `headless`, `complete`) = 16 tarballs, publishes
  a GitHub Release with `SHA256SUMS`, publishes the crate, and bumps Homebrew.
- `scripts/render-homebrew-formulas.sh` — renders **5** Homebrew formulas
  (`rover`, `rover-complete`, `rover-local-inference`, `rover-local-vision`,
  `rover-headless`) with mutual conflict detection.
- `docs/releasing.md`, `docs/versioning.md` — document the manual tag-driven flow.

Nothing has shipped yet: **0 git tags**, and `rover-mcp` is **not** on crates.io.
So no names or versions are locked in — the rename and the pipeline swap are both
free to make now.

`ci.yml` and `smoketest.yml` are **out of scope** and stay exactly as they are.

## Key decisions (settled during brainstorming)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Tooling | release-plz + cargo-dist (`dist`) | Exactly the tools requested; standard, minimal custom YAML. |
| Variant strategy | **Single default binary** | cargo-dist builds one binary per platform; the multi-variant matrix and 5-formula Homebrew scheme are dropped. |
| Released binary features | **`--features headless`** | The one prebuilt/Homebrew binary bakes in headless. Power users get other variants via `cargo install` / source. |
| Crate `default` features | **stays `default = []`** | Decoupled from the distributed binary. `cargo build`/dev/CI defaults and crates.io source semantics are unchanged. The prebuilt/Homebrew binary differs from `cargo install rover-fetch` — documented. |
| Targets | **4 (no Windows)** | `x86_64`/`aarch64` Linux-gnu + `x86_64`/`aarch64` macOS. Same as today. Windows code paths (signals, paths, SSRF) are untested; out of scope. |
| Installers | **Homebrew + shell** | `dist` generates the tap formula and a `curl \| sh` one-liner (no-Homebrew, no-Rust path). |
| Crate name | **`rover-mcp` → `rover-fetch`** | `rover-fetch` is available on crates.io; `rover` is taken. |

## Architecture

Two-stage pipeline. release-plz owns **versioning + crates.io + the git tag**;
`dist` owns **binaries + GitHub Release + Homebrew + shell installer**.

```
push to main (conventional commits)
      │
      ▼
release-plz "release-pr" job ──► opens/updates a Release PR
      │                          (bumps version in Cargo.toml + Cargo.lock,
      │                           rewrites CHANGELOG.md from commit history)
      ▼  (maintainer merges the Release PR)
release-plz "release" job (runs under RELEASE_PLZ_TOKEN, a PAT/App token):
   • cargo publish  rover-fetch ──► crates.io        [CARGO_REGISTRY_TOKEN]
   • push git tag   v<version>   ──► triggers ▼
   • does NOT create the GitHub Release (git_release_enable = false)
      │
      ▼
dist's generated release.yml (triggered by the v<version> tag):
   • builds  rover --features headless --no-default-features  for the 4 targets
   • creates the GitHub Release + uploads tarballs + SHA256 checksums
   • generates the curl | sh installer
   • publishes the `rover` Homebrew formula to aaronbassett/homebrew-tap
        with  depends_on "chromium"                  [HOMEBREW_TAP_TOKEN]
```

**The load-bearing line is `git_release_enable = false`** in `release-plz.toml`:
it stops release-plz from creating the GitHub Release, so `dist` owns it and the
two tools don't collide.

### The sharp edge: release-plz must run under a non-default token

The tag release-plz pushes must **trigger** `dist`'s `release.yml`. GitHub's
default `GITHUB_TOKEN` deliberately **cannot trigger other workflow runs**, so a
tag pushed with it would silently fail to start the build. release-plz must
therefore run under **`RELEASE_PLZ_TOKEN`** (a PAT or GitHub App token) with
`contents: write` (push tags/commits) and `pull-requests: write` (open the
Release PR). This is the single most common way this combination breaks.

## Components

### 1. Crate rename (`Cargo.toml`)

- `name = "rover-mcp"` → `name = "rover-fetch"`; update the explanatory comment.
- `[lib] name = "rover"` and `[[bin]] name = "rover"` are **unchanged**.
- Sweep remaining `rover-mcp` references: `Cargo.lock`, `docs/releasing.md`,
  README, and anywhere `grep -rn rover-mcp` finds them (e.g. the old
  `cargo publish -p rover-mcp` invocation that is being deleted).

### 2. dist configuration (`[workspace.metadata.dist]` in `Cargo.toml`)

```toml
[workspace.metadata.dist]
cargo-dist-version = "<pin to current, e.g. 0.32.0>"
targets = [
  "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin", "aarch64-apple-darwin",
]
installers   = ["shell", "homebrew"]
publish-jobs = ["homebrew"]
tap     = "aaronbassett/homebrew-tap"
formula = "rover"

[package.metadata.dist]
features = ["headless"]      # distributed binary only; crate default stays []
default-features = false

[package.metadata.dist.dependencies.homebrew]
chromium = { version = "*", stage = ["run"] }   # → runtime depends_on "chromium"
```

Notes:
- `features` + `default-features = false` make `dist` build
  `--features headless --no-default-features` for the distributed binary **without**
  touching the crate's own `[features] default`.
- `stage = ["run"]` makes `chromium` a **runtime** `depends_on` in the generated
  formula (the default `build` stage would not appear in the published formula).
- `dist` builds `aarch64-unknown-linux-gnu` on **native arm64 GitHub runners**
  (default since dist v0.30.0) — this removes the `cross` step the old pipeline
  needed and sidesteps its cross-gcc workarounds.
- The exact config surface (file location `dist-workspace.toml` vs
  `Cargo.toml`, key spellings) is pinned to the chosen `dist` version at
  implementation time via `dist init`; the keys above reflect current docs.

### 3. dist-generated workflow (`.github/workflows/release.yml`)

Generated by `dist init` / `dist generate` (not hand-written). Triggers on the
`v*` tag (glob `*[0-9]+.[0-9]+.[0-9]+*`), which release-plz's pushed tag matches.
It is committed to the repo and is plain GitHub Actions YAML the project owns.

### 4. release-plz configuration (`release-plz.toml`)

```toml
[workspace]
git_release_enable = false   # dist owns the GitHub Release
# git_tag_enable defaults true; default tag name `v{{ version }}` already
# matches dist's trigger glob — no override needed.
```

- Release-PR flow (not release-on-every-push).
- release-plz takes over `CHANGELOG.md` going forward via git-cliff. Existing
  curated entries are **preserved**; the changelog template is tuned to stay
  close to the current Keep-a-Changelog style. Commits are already conventional
  (`feat(...)`, `docs(...)`, `fix(...)`), so version inference works out of the box.

### 5. release-plz workflow (`.github/workflows/release-plz.yml`)

Standard two-job workflow on push to `main`: a `release-pr` job and a `release`
job, both `uses: release-plz/action@v1`, both passing
`GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}` and
`CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}`.

## Secrets / one-time setup

All three custom secrets are **already created** on the repo with the correct
names (verified 2026-06-22). The default `GITHUB_TOKEN` is auto-provided.

| Secret | Used by | Purpose | Scope requirement |
| --- | --- | --- | --- |
| `CARGO_REGISTRY_TOKEN` | release-plz | `cargo publish` to crates.io | crates.io token, publish scope |
| `RELEASE_PLZ_TOKEN` | release-plz | push tags that **trigger** dist; open Release PR | PAT/App: `contents: write`, `pull-requests: write` |
| `HOMEBREW_TAP_TOKEN` | dist | push formula to `aaronbassett/homebrew-tap` | PAT with write to the tap repo |
| `GITHUB_TOKEN` (default) | dist | create the GitHub Release | auto-provided |

The old `GPG_PRIVATE_KEY` signing path is **dropped**: dist ships per-artifact
SHA256 checksums automatically. (GitHub artifact attestations are a possible
later add, not in scope.)

## Files

**Add:**
- `release-plz.toml`
- `.github/workflows/release-plz.yml`
- `.github/workflows/release.yml` (regenerated by `dist`, replacing the old one)
- dist config block in `Cargo.toml`

**Remove:**
- the old hand-rolled `.github/workflows/release.yml`
- `scripts/render-homebrew-formulas.sh` (and the now-empty `scripts/` dir if nothing else lives there)

**Rewrite:**
- `docs/releasing.md` — new flow: merge a Release PR; the difference between the
  prebuilt/Homebrew headless binary and `cargo install rover-fetch` (basic);
  new secret names.

**Light touch:**
- `docs/versioning.md` — the manual `-alpha.N` "validate the pipeline" dance is
  replaced by the Release-PR flow; the SemVer/MSRV policy itself is unchanged.

## First release / versioning

`rover-fetch` is unpublished, so release-plz's first Release PR establishes the
baseline version. Decision deferred to the implementation plan: either keep
`0.1.0-alpha.1` or cut a clean `0.1.0`. After the first release, release-plz
drives all version bumps from commit history.

## Testing / validation

This is CI/release plumbing — validated by exercising the pipeline, not by unit
tests:

1. `dist init` + `dist generate` run clean; `dist plan` shows the 4 targets, the
   shell + Homebrew installers, and `--features headless`.
2. `cargo build --release --no-default-features --features headless` succeeds for
   all 4 targets (the matrix already proves headless cross-compiles).
3. `release-plz` opens a Release PR on a push to `main` (observe; the dry-run is
   that the PR is a no-op to merge).
4. End-to-end: merging the first Release PR publishes `rover-fetch` to crates.io,
   pushes the tag, and the tag drives `dist` to create the Release + tap formula.
   `brew install aaronbassett/tap/rover` and the `curl | sh` installer both yield
   a working headless `rover`.

## Risks

- **dist maintenance intensity.** axodotdev's commercial side wound down; `dist`
  is still maintained (v0.32.0, May 2026, repo not archived) but recent activity
  is mostly dependency bumps. We use it only for GitHub-Releases distribution (the
  durable, non-hosted path), version-pinned. If upstream goes dormant, the
  generated `release.yml` is plain Actions YAML the project owns and can maintain
  by hand — lock-in is low. (The Astral fork is archived; axodotdev is canonical.)
- **Token misconfiguration.** If `RELEASE_PLZ_TOKEN` is secretly the default
  token, tags won't trigger `dist`. Mitigation: verify it's a PAT/App token with
  the required scopes; the first end-to-end release confirms it.

## Out of scope

- Windows targets; Linux `deb`/`rpm` packages.
- GPG/sigstore signing and artifact attestations.
- Changes to `ci.yml` / `smoketest.yml`.
- Restoring prebuilt non-headless variants (remain available via `cargo install`
  / source builds).

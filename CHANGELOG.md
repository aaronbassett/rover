# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0 stability expectations are spelled out in [`docs/versioning.md`](docs/versioning.md).

## [0.1.0] - 2026-06-22

### Added

- `SECURITY.md` with a private disclosure address and a pre-1.0 supported-versions
  policy (#22).
- Automated release pipeline: [release-plz](https://release-plz.dev) drives
  versioning, the changelog, the crates.io publish, and tagging;
  [dist](https://opensource.axo.dev/cargo-dist/) builds the four target binaries
  (`--features headless`), creates the GitHub Release with a `curl | sh`
  installer and SHA-256 checksums, and publishes a single `rover` Homebrew
  formula (`depends_on "chromium"`) to `aaronbassett/homebrew-tap`.
  `LICENSE-MIT` / `LICENSE-APACHE`; a release runbook (`docs/releasing.md`).

### Changed

- The crate is published on crates.io as `rover-fetch` (the `rover` name is held
  by an unrelated project). The installed binary and the importable library are
  both still named `rover`.
- Release builds now strip symbols (`[profile.release] strip = "symbols"`).

### Removed

- The `local-vision` Cargo feature and its `mistralrs`/SmolVLM image-captioning
  backend (`kind = "local"` captioners). It was unusable on the CPU backend the
  nightly runs on (vision-attention contiguity and encoder-cache bugs in
  mistralrs 0.8.x). Use a cloud captioner (`kind = "cloud"`) instead — including
  local OpenAI-compatible servers (ollama, LM Studio, …) via
  `provider = "openai_compat"`. Local *text* summarization (`local-inference`)
  is unaffected. See git history to restore the backend.

### Security

- Image-fetch helpers (`download_image_bytes`, `partial_fetch_dimensions`,
  `fetch_content_length`, `download_one`) now enforce the active `SsrfLevel`
  via a pre-flight address check plus the dial-time `SSRF_LEVEL` scope, closing
  an SSRF gap where a page could make Rover fetch private, loopback, or
  cloud-metadata addresses during image download or caption filtering (#23).

## 0.1.0-alpha.1 — 2026-05-28

First tagged pre-release. Summarises the work from the initial fetch path
(M1) through the pre-release hardening pass (#21).

### Added

- **M1** — single-URL fetch path: `rover fetch <url>` end-to-end, charset
  detection, `readabilityrs`-based extraction to clean Markdown (#1).
- **M2** — caching & storage: SQLite-backed cache with TTL handling and
  stale-while-revalidate; `rover cache` subcommands (#3).
- **M3** — MCP server mode: `rover mcp` over stdio, exposing the fetch tool
  surface to MCP-speaking agents (#4).
- **M4** — content enrichment: structured metadata extraction, table handling,
  image modes, and link absolutization (#5).
- **M5** — politeness: per-domain rate limiting and `robots.txt` honouring (#6).
- **M6** — long-running tasks & batching, with NDJSON-streamed progress; plus a
  polish pass collapsing unused enum variants and `Deps` structs (#7, #8).
- **M7** — summarization: extractive and cloud-LLM backends, a summary cache,
  and the `summarize` tool (#9).
- **M8** — SSRF levels (`strict`/`loopback`/`project`/`lan`/`none`),
  diagnostics, and polish (#10).
- **M9** — feature-flagged extras: `local-inference`, `local-vision`,
  `headless`, and cloud captioners (#11).
- ASCII banner on the top-level `--help` (#17).

### Changed

- Binary-size assertion moved to the nightly smoketest with a 75 MiB ceiling (#12).
- README rewritten for a release-ready first impression (#14).
- One-shot CLI subcommands default to warn-level logs (#16).
- Fast integration-test subset now runs on the blocking merge path (#21).

### Fixed

- Cache stale-while-revalidate bounded by a grace window, with synchronous
  refresh on the one-shot CLI path (#15).
- `doctor` loads the tokenizer before the `extractive_synthesis` check (#18).
- Shared `OUTPUT_DIR_TEST_MUTEX` across extractor test modules to eliminate
  env-var flake (#19).

### Security

- Closed a DNS-rebinding TOCTOU via a dial-time validating resolver and reduced
  the production panic surface (#13).
- HTTP `Authorization` credentials are scrubbed from tracing events (#20).

[0.1.0]: https://github.com/aaronbassett/rover/releases/tag/v0.1.0

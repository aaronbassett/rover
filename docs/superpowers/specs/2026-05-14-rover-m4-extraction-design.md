# Rover M4 — Metadata, Tables, Images, Links Design

> **Status:** approved design, pending implementation plan.
> **Scope:** Milestone M4 only. Sequels (M5–M9) get their own designs.
> **Companion docs:**
> - PRD: `docs/superpowers/prd/2026-05-07-rover-prd.md` (§6 extraction; §6.2 frontmatter; §6.3 tables; §6.4 images; §6.5 links; §6.6 metadata).
> - Design supplement: `docs/superpowers/specs/2026-05-07-rover-design.md` (§2.8 output paths).
> - Milestone manifest: `docs/superpowers/milestones/rover-milestones.md` (M4 section).
> - M3 design: `docs/superpowers/specs/2026-05-13-rover-m3-mcp-design.md` (for the typed-arg upgrade context).

## Goal

Add structured metadata extraction (JSON-LD + Open Graph + Twitter Cards),
table and image transformation modes, absolute-link rewriting, and a
`get_metadata` MCP tool. Tighten the `fetch` tool's `tables`/`images`/
`metadata` args from M3's `Option<serde_json::Value>` placeholders to typed
structs. Produce richer frontmatter so agents can budget tokens, follow
links without an extra base-URL hop, and consume large tables without
blowing context windows.

## Decisions on the manifest's open questions

The manifest flagged six planning-time questions plus several additional
scoping choices surfaced during brainstorming. All are resolved here so the
implementation plan doesn't need to re-litigate them.

1. **Table modes** — ship four: `Embed`, `Sample`, `CsvFile`, `Drop`.
   Defer `Summarize` to M7. The schema accepts `{mode: "summarize"}` and
   the M4 runtime rejects it with `RoverError::EXTRACT_FAILED` and a
   message pointing at M7. No stub code path.
2. **Image modes** — ship four: `Keep`, `AltTextOnly`, `Download`, `Drop`.
   Defer `CaptionVlm` to M9. Same schema-accept / runtime-reject pattern
   as tables Summarize.
3. **Sample strategies** — ship `HeadTail` and `RandomSeed`. Defer
   `Stratified` until a consumer demonstrates a need.
4. **Microdata** — drop from scope. JSON-LD has supplanted microdata for
   nearly all structured-data publishing; the `microdata` crate is a
   low-maintenance 0.1.x with ~1.4k downloads. Three sources ship: JSON-LD,
   Open Graph, Twitter Cards. Documented as a known limitation.
5. **`extraction_quality` heuristic** — text-density ratio plus
   structured-metadata bonus:
   ```
   density = (extracted_md_text_len / raw_html_text_len.max(1)).min(1.0)
   bonus  = 0.05 (if title present) + 0.10 (if any metadata extracted)
   score  = (density + bonus).clamp(0.0, 1.0)
   ```
6. **JSON-LD walker** — flatten with depth limit 8. Collect every node
   with `@type`. Pick the "primary" node as the first whose `@type` is in
   `{Article, NewsArticle, BlogPosting, WebPage, Product}`; fall back to
   the first node with any `@type`. Surface its scalar fields; surface all
   distinct `@type` values in `schema_types`.
7. **`<base href>` handling** — read with a pre-pass on raw HTML before
   `readabilityrs` (because readabilityrs may strip the `<base>` tag).
   Fall back to the final URL after redirects if `<base href>` is absent
   or relative.
8. **Link/image rewriting timing** — post-pass on the rendered markdown.
   Walk inline links, reference-style definitions, and inline images;
   resolve relative URLs against the pre-extracted base URL.
9. **Output directory** — `$XDG_DATA_HOME/rover/output/` by default; user
   can override via `[output] dir = "..."` config or `ROVER_OUTPUT_DIR`
   env var. Auto-created on first write.
10. **`get_metadata` MCP tool** — a dedicated tool returning only the
    metadata block. Reuses the cached fetch pipeline under the hood; never
    serialises the markdown body. Agents that only want metadata don't pay
    for the body.
11. **Typed args** — hard cutover. `tables: Option<TablesOpts>`,
    `images: Option<ImagesOpts>`, `metadata: Option<MetadataOpts>` with
    `#[serde(deny_unknown_fields)]`. M3 clients sending free-form JSON for
    those fields now receive `invalid_args`. Pre-release; no shim.
12. **Defaults when args are absent** — `tables: Embed`, `images:
    AltTextOnly`. Preserves content-by-default semantics.
13. **Embed oversize handling** — no auto-switch. If the agent wanted size
    control they should have chosen `Sample`, `CsvFile`, or `Drop`. The
    `tables_transformed` record always reports the actual mode applied.
14. **Frontmatter expansion** — add all listed fields at the top level,
    omit when empty. Matches the M1/M2/M3 envelope pattern.

## Architecture

M4 grows the existing extractor pipeline with four new responsibilities and
adds one MCP tool. No new top-level modules — everything plugs into
`src/extractor/`.

The extractor pipeline gains a **two-pass** shape:
1. **Pre-pass on raw HTML** (before `readabilityrs`): parse the document
   with `scraper`, read `<base href>` if present, extract structured
   metadata (JSON-LD, Open Graph, Twitter Cards).
2. **`readabilityrs` extraction** (unchanged from M1/M2/M3).
3. **Post-pass on the rendered markdown**: absolutize links/images using
   the resolved base URL, transform tables per the requested mode,
   transform image syntax per the requested mode.

A new `ExtractOptions` struct flows through the pipeline carrying the four
sub-options: `tables: TablesMode`, `images: ImagesMode`,
`metadata: MetadataMode`, plus `output_paths: Arc<OutputPaths>`. M3's MCP
`fetch` args become typed strictly with `#[serde(deny_unknown_fields)]`. A
new top-level `get_metadata` MCP tool wraps the metadata-extraction half
of the pipeline.

Output writes (Tables `CsvFile`, Images `Download`) land under
`$XDG_DATA_HOME/rover/output/{tables,images}/<host>/<sha8>.{csv,ext}` with
`[output] dir` config and `ROVER_OUTPUT_DIR` env overrides. Frontmatter
records absolute paths to those files.

### Module layout introduced by M4

```
src/extractor/
  metadata.rs                     # JSON-LD + OG + Twitter Cards walkers
  base_href.rs                    # peek <base href> from raw HTML pre-readabilityrs
  links.rs                        # markdown link/image post-pass absolutizer
  tables.rs                       # TablesMode + Embed/Sample/CsvFile/Drop
  images.rs                       # ImagesMode + Keep/AltTextOnly/Download/Drop
  quality.rs                      # extraction_quality scorer
  output.rs                       # OutputPaths resolution + sha8 path helpers

src/mcp/tools/
  get_metadata.rs                 # new MCP tool

# Modified
src/extractor/mod.rs              # add new modules
src/extractor/pipeline.rs         # extended to drive the two-pass shape
src/extractor/frontmatter.rs      # new optional fields; flat top-level
src/extractor/error.rs            # new variants (Metadata, Output, TableWrite, ImageDownload, ImageWrite)
src/mcp/tools/fetch.rs            # tighten tables/images/metadata to typed structs
src/mcp/mod.rs                    # +pub mod tools::get_metadata
src/mcp/handler.rs                # register get_metadata_tool in #[tool_router]
src/mcp/envelope.rs               # MetadataResponse
src/config.rs                     # [output] section
Cargo.toml                        # +csv, +rand (seedable RNG); +mime_guess if not transitively present
```

## Components

### `src/extractor/metadata.rs`

Public surface: `pub fn extract(html: &str, base: &Url) -> ExtractedMetadata`.

```rust
pub struct ExtractedMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub published: Option<String>,       // ISO-8601 or whatever the source provides
    pub modified: Option<String>,
    pub image: Option<String>,           // absolutized
    pub og_type: Option<String>,
    pub canonical: Option<String>,       // absolutized
    pub language: Option<String>,
    pub schema_types: Vec<String>,
}
```

Three walkers run in sequence, each producing a partial `ExtractedMetadata`;
results merge with first-wins precedence in this order:
**JSON-LD → Open Graph → Twitter Cards**. Reasoning: JSON-LD is the most
structured and intentional; OG and TC are usually metatag fallbacks.

- **JSON-LD walker**: parse every
  `<script type="application/ld+json">` block via
  `serde_json::from_str::<Value>`. Walk `@graph` arrays and nested objects
  up to depth 8. Collect every node with `@type`. Pick the "primary" node
  as the first whose `@type` is one of
  `{Article, NewsArticle, BlogPosting, WebPage, Product}`; if none match,
  the first node with any `@type`. Surface its scalar fields. All distinct
  `@type` values join into `schema_types`.
- **Open Graph walker**: `meta[property^="og:"]` via `scraper`. Maps
  `og:title`→title, `og:description`→description, `og:image`→image,
  `og:type`→og_type. `article:published_time`→published,
  `article:modified_time`→modified, `article:author`→author.
- **Twitter Cards walker**: `meta[name^="twitter:"]`. Maps `twitter:title`,
  `twitter:description`, `twitter:image`. Lower precedence; only fills
  holes.

Plus: `meta[name="description"]`→description (lowest precedence).
`html[lang]`→language. `link[rel="canonical"]`→canonical (absolutized via
the base URL).

Per-source walker failures are **soft**: a malformed JSON-LD script logs
warn and contributes nothing. If `scraper` itself errors on the document
(extreme degenerate input), returns an empty `ExtractedMetadata` and the
catastrophic case is surfaced via `ExtractorError::Metadata`.

### `src/extractor/base_href.rs`

`pub fn read_base_href(html: &str) -> Option<Url>`. Runs once on raw HTML
before readabilityrs. Selects `head > base[href]` and parses; returns
`Some(Url)` if absolute, `None` otherwise. Cheap, pre-extraction. If
multiple `<base>` tags exist (illegal per HTML5 but seen in the wild),
first one wins.

### `src/extractor/links.rs`

`pub fn absolutize(markdown: &str, base: &Url) -> String`. A regex-driven
post-pass over the rendered markdown that resolves any relative URL it
finds in:

- Inline links: `[text](href)`
- Inline images: `![alt](src)`
- Reference-style link definitions: `[id]: href "title"`

For each, `base.join(href)` if `href` is relative; leave alone if already
absolute. Errors from `Url::join` log at debug and leave the original text
in place.

**`<img srcset>` known limitation**: srcset lives in raw HTML, not
markdown, and `readabilityrs`'s markdown output emits a single `src` per
image — the srcset variants don't survive extraction. M4 does not attempt
to recover them. Documented limitation; a follow-up could lift the
highest-resolution srcset entry via the HTML pre-pass.

### `src/extractor/tables.rs`

```rust
pub enum TablesMode {
    Embed,
    Sample(SampleStrategy),
    CsvFile,
    Drop,
    // Summarize variant exists on the wire but errors at runtime.
}

pub enum SampleStrategy {
    HeadTail { head: usize, tail: usize },          // defaults: 5, 5
    RandomSeed { rows: usize, seed: u64 },          // defaults: 10, 42
}
```

Walks the markdown for pipe-table blocks (readabilityrs emits these for
HTML `<table>`). For each table:

- **Embed**: pass through unchanged.
- **Sample**: parse the rows, apply the strategy, re-render with a
  `_… N rows truncated …_` marker between the kept regions.
- **CsvFile**: serialise the table as CSV via the `csv` crate, write to
  `<output_dir>/tables/<host>/<sha8>.csv` where
  `sha8 = first 8 hex of sha256(absolute_url || "#" || table_ordinal)`,
  replace the table in the markdown with
  `_Table N saved to <absolute_path>_`.
- **Drop**: replace with `_Table N omitted_`.

Each transformation appends one entry to `tables_transformed:
Vec<TableTransform>` in the frontmatter, recording mode, source table
ordinal, (for Sample) the strategy parameters, and (for CsvFile) the file
path.

### `src/extractor/images.rs`

```rust
pub enum ImagesMode {
    Keep,
    AltTextOnly,         // default
    Download,
    Drop,
    // CaptionVlm variant exists on the wire but errors at runtime.
}
```

Walks the markdown's inline-image syntax. For each `![alt](src)`:

- **Keep**: unchanged.
- **AltTextOnly**: replace with the alt text (or remove entirely if alt is
  empty).
- **Download**: HTTP GET via the existing `reqwest::Client`, save to
  `<output_dir>/images/<host>/<sha8>.<ext>` where
  `sha8 = first 8 hex of sha256(absolute_url)` and `ext` is sniffed from
  `Content-Type` (fallback: extension from URL path; final fallback:
  `.bin`). Rewrite the markdown to `![alt](<absolute_local_path>)`.
  Failures log warn and leave the original markdown intact (soft fail).
- **Drop**: remove the image syntax entirely.

A download failure increments a per-fetch counter; the final
`images_failed: N` appears in frontmatter if N > 0.

### `src/extractor/quality.rs`

`pub fn score(extracted_md: &str, raw_html_text_len: usize, has_metadata: bool, has_title: bool) -> f32`.

```rust
let density = (extracted_md_text_len as f32
    / raw_html_text_len.max(1) as f32).min(1.0);
let bonus =
    if has_title { 0.05 } else { 0.0 }
    + if has_metadata { 0.10 } else { 0.0 };
(density + bonus).clamp(0.0, 1.0)
```

`extracted_md_text_len` is the markdown length minus the frontmatter and
any whitespace-only lines; `raw_html_text_len` is the visible-text length
of the original HTML (via scraper's `text()` API on `body`).

### `src/extractor/output.rs`

```rust
pub struct OutputPaths { root: PathBuf }

impl OutputPaths {
    pub fn resolve(config: &OutputConfig) -> Result<Self, ExtractorError>;
    pub fn table_path(&self, url: &Url, table_ordinal: usize) -> PathBuf;
    pub fn image_path(&self, url: &Url, ext: &str) -> PathBuf;
}

pub fn sha8(input: &str) -> String;  // first 8 hex of sha256
```

Precedence in `resolve`: `ROVER_OUTPUT_DIR` env var > `[output] dir`
config field > XDG default. Creates the root + `tables/` and `images/`
subdirs on first call. Per-host subdirs are created lazily by
`table_path`/`image_path` callers.

### `src/mcp/tools/get_metadata.rs`

```rust
pub struct GetMetadataArgs {
    pub url: String,
    pub force_refresh: Option<bool>,
    pub tokenizer: Option<String>,    // for extraction_quality bonus only; unused otherwise
}

// Returns mcp::envelope::MetadataResponse:
pub struct MetadataResponse {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub published: Option<String>,
    pub modified: Option<String>,
    pub image: Option<String>,
    pub og_type: Option<String>,
    pub canonical: Option<String>,
    pub language: Option<String>,
    pub schema_types: Vec<String>,
    pub extraction_quality: f32,
    pub url: String,
    pub content_hash: String,
    pub fetched_at: String,
    pub cache_status: CacheStatus,
}
```

Reuses `fetch_with_cache` + the metadata extractor. Never serialises the
markdown body. On cache hit with populated `pages.metadata_json`,
deserialises and returns immediately; on cache miss or empty metadata
column, runs the extractor and writes back to `metadata_json`.

### `src/config.rs`

```toml
[output]
dir = "/some/path"  # optional; default: $XDG_DATA_HOME/rover/output
```

`OutputConfig` mirrors the existing `FetchConfig`/`CacheConfig`/
`TokenizerConfig`/`McpConfig` pattern.

### `src/extractor/pipeline.rs` updates

New `ExtractOptions` carried through the pipeline:

```rust
pub struct ExtractOptions {
    pub tables: TablesMode,
    pub images: ImagesMode,
    pub metadata: MetadataMode,
    pub output_paths: Arc<OutputPaths>,
}

pub enum MetadataMode { Include, Skip }
```

`extract()` returns an enriched `ExtractedDoc { title, body_md,
metadata: ExtractedMetadata, tables_transformed: Vec<TableTransform>,
images_failed: usize }`.

## Data flow

### `fetch` tool call (M4 shape)

```
client → rmcp → RoverHandler::fetch(FetchArgs)
  → parse FetchArgs (now-typed tables/images/metadata)
  → fetch_with_cache(url, force_refresh)             [M2]
      → returns FetchedPage + cache_status
  → base_href = base_href::read_base_href(raw_html)
      .unwrap_or_else(|| final_url_after_redirects)
  → metadata = metadata::extract(raw_html, &base_href)   [pre-pass]
  → extracted = readabilityrs::extract(raw_html, base_href)   [unchanged]
  → markdown = extracted.body_md
  → markdown = links::absolutize(markdown, &base_href)         [post-pass 1]
  → (markdown, tables_transformed) = tables::apply(
        markdown, opts.tables, &output_paths, &base_href)
  → (markdown, images_failed) = images::apply(
        markdown, opts.images, &output_paths, &http_client, &base_href).await
  → quality = quality::score(
        &markdown, raw_html_text_len, !metadata.is_empty(),
        metadata.title.is_some())
  → tokens = tokenizer::count(&markdown, family)
  → max_tokens check (unchanged)
  → frontmatter = frontmatter::render(PageMeta {
        … +metadata fields, tables_transformed, images_failed, quality })
  → cache write: store extracted markdown + metadata_json
  → return FetchResponse { markdown, frontmatter, cache_status }
        (or CountResponse if count_only)
```

The cache write captures the post-extract metadata so a later
`cache_status: hit` call doesn't lose it. The `pages.metadata_json`
column was reserved in M2's `001_initial.sql` exactly for this — M4
starts populating it. On cache hit, deserialise it back; on cache miss,
store after extraction.

If `opts.tables == CsvFile` or `opts.images == Download`, side-effect
writes happen during the post-pass. These are idempotent (same `sha8`
produces the same path; existing files get overwritten with identical
content). No locking; concurrent fetches of the same URL by different
rover instances are tolerated.

### `get_metadata` tool call

```
client → rmcp → RoverHandler::get_metadata(GetMetadataArgs)
  → fetch_with_cache(url, force_refresh)
  → if cache hit and pages.metadata_json populated:
        metadata = deserialise(pages.metadata_json)
  → else:
        base_href = base_href::read_base_href(raw_html).unwrap_or(final_url)
        metadata = metadata::extract(raw_html, &base_href)
        pages.metadata_json = serialise(metadata)   [write-back]
  → quality = quality::score(
        &page.extracted_md, raw_html_text_len, …)
  → return MetadataResponse { …metadata, url, content_hash, fetched_at,
        cache_status, extraction_quality }
```

`get_metadata` deliberately does NOT run the table/image post-passes. It
only needs the metadata block. The cached `extracted_md` is read so the
quality score can be computed.

### Cache-row evolution

The M2 `pages` table already has a `metadata_json BLOB` column (reserved,
NULL today). M4 starts writing it: a serde-serialised `ExtractedMetadata`
blob as JSON. JSON is the right choice for v1: ~1KB per page,
debug-friendly via `sqlite3`, no compression dep needed.

**No schema change required. No migration.**

### Two-pass HTML walking — performance note

The pre-pass parses the raw HTML once with `scraper` for `<base href>` +
metadata extraction. `readabilityrs` then re-parses internally. M4 accepts
this double-parse: scraper is fast (parses ~10MB/s) and the alternative
(refactor readabilityrs to expose its parsed DOM) is out of scope. A
future optimisation could fork readabilityrs to share the parsed tree;
not worth it now.

## Error handling

### `ExtractorError` extensions

The existing `extractor::ExtractorError` gains M4 variants:

```rust
#[error("metadata extraction failed: {0}")]
Metadata(String),

#[error("output directory error at {path}: {source}")]
Output {
    path: String,
    #[source]
    source: std::io::Error,
},

#[error("could not write table {ordinal} to {path}: {source}")]
TableWrite {
    ordinal: usize,
    path: String,
    #[source]
    source: std::io::Error,
},

#[error("could not download image at {url}: {source}")]
ImageDownload {
    url: String,
    #[source]
    source: reqwest::Error,
},

#[error("could not write image at {path}: {source}")]
ImageWrite {
    path: String,
    #[source]
    source: std::io::Error,
},
```

`Metadata(String)` is the catastrophic-fail variant for the rare case
where scraper itself errors on the document. Soft-fail metadata partials
do not produce this variant; they log warn and contribute empty.

### Soft-fail policy

- **Metadata source walkers**: per-source failures are soft. A malformed
  JSON-LD script doesn't fail the fetch; the walker logs warn and
  contributes empty for that block.
- **Image downloads**: soft. A failed download leaves the original `![alt
  ](src)` syntax intact (behaves as `Keep` for that specific image).
  Frontmatter records `images_failed: N` if N > 0.

### Hard-fail

- **Output-directory resolution**: if `OutputPaths::resolve` cannot create
  or write to the output root, fail with `ExtractorError::Output`. Better
  to error early than to silently degrade Download/CsvFile to Keep/Embed.
- **Table CsvFile write**: if CSV serialisation succeeds but file write
  fails, fail with `ExtractorError::TableWrite`. If the agent asked for
  CsvFile mode, they need the file.

### MCP-tool translation

`McpError::Extractor` already routes to `RoverError::EXTRACT_FAILED` from
M3. New M4 sub-variants flow through the existing path. **No new wire
codes**; the `code` set stays at the eight constants from M3.

`get_metadata` errors translate the same way. The handful of
`get_metadata`-specific user errors (invalid URL, unknown tokenizer
family) reuse `McpError::InvalidUrl` / `McpError::InvalidArgs`.

### Argument validation

Typed M4 arg structs all carry `#[serde(deny_unknown_fields)]`. Schema-level
rejection produces `RoverError { code: "invalid_args" }` via the existing
M3 path.

Semantic validations enforced post-deserialise:
- `Sample(HeadTail { head, tail })`: both must be > 0.
- `Sample(RandomSeed { rows, .. })`: `rows` must be > 0.
- `[output] dir`: if relative, resolved against CWD. If the path exists
  but isn't a directory, `OutputPaths::resolve` fails with
  `ExtractorError::Output`.

### Wire-side rejection of deferred modes

- `TablesMode::Summarize` (in the wire schema but not implemented):
  `RoverError { code: "extract_failed", message: "tables summarize mode
  is not available until M7" }`.
- `ImagesMode::CaptionVlm`: `RoverError { code: "extract_failed",
  message: "images caption_vlm mode requires the vlm feature (M9)" }`.

These are user-input errors and could plausibly map to
`code: "invalid_args"`, but `extract_failed` reads better here because
the schema accepted the request — the extractor is what fails to honour
it. Documented intent.

### Logging

- One `tracing::info` per cache-miss extraction summarising:
  `tables_transformed.len()`, `images_seen`, `images_downloaded`,
  `images_failed`, `schema_types.len()`.
- One `tracing::warn` per soft-fail (malformed JSON-LD block, failed
  image download).
- One `tracing::debug` per typed-arg deserialisation failure (already
  covered by M3's argument logging).

## Testing

### Unit tests (inline `#[cfg(test)] mod tests`)

- **`metadata.rs`**: JSON-LD walker finds Article in `@graph`; prefers
  Article over WebPage; depth-8 cap (synthetic 20-deep object); merges
  scalar fields from primary node; OG walker reads og:* fields; Twitter
  fills holes left by OG; precedence JSON-LD > OG > Twitter; `html[lang]`
  populates language; canonical resolved against base; malformed JSON-LD
  doesn't fail walker.
- **`base_href.rs`**: absolute base parsed; relative/missing returns
  None; first wins on duplicate.
- **`links.rs`**: relative inline link absolutized; absolute left intact;
  reference-style definitions absolutized; inline image src absolutized;
  anchors absolutized; invalid URL components unchanged with debug log.
- **`tables.rs`**: pipe-table detection; Embed round-trips; HeadTail
  Sample with head=2/tail=2 on 10-row table produces 4 rows + marker;
  RandomSeed with fixed seed is deterministic; CsvFile writes correct CSV
  to expected path; Drop replaces with placeholder.
- **`images.rs`**: Keep round-trips; AltTextOnly substitutes alt text
  (empty alt removes); Drop removes entirely; Download writes file under
  expected path (against wiremock; `--features test-loopback`); Download
  failure leaves original markdown intact + increments images_failed.
- **`quality.rs`**: empty → 0.0; full coverage → 1.0 (clamped);
  density-only path produces expected ratio; score always in [0, 1].
- **`output.rs`**: `sha8` deterministic; `table_path`/`image_path`
  produce documented shape; `resolve` honours env var, config, XDG
  default; `resolve` creates directory if missing.
- **`config.rs`**: load + validate `[output]` section.
- **`mcp/tools/get_metadata.rs`**: schema completeness; deny_unknown_fields;
  missing-url rejection.
- **`mcp/tools/fetch.rs`**: schema now includes typed
  `tables`/`images`/`metadata` shapes; rejects bad shapes; rejects
  `tables.summarize` and `images.caption_vlm` at runtime with the
  documented messages.

### Integration tests (`tests/`)

```
tests/extractor_metadata.rs       # against fixture HTML
tests/extractor_tables.rs         # CsvFile + Sample modes end-to-end
tests/extractor_images.rs         # Download via wiremock; test-loopback feature
tests/extractor_links.rs          # absolutization across edge cases
tests/mcp_get_metadata.rs         # rmcp client + child process
```

**Fixture HTML pages** live in `tests/fixtures/m4/` and cover:
- Article with JSON-LD `Article` + OG + Twitter Cards
- News article with `@graph` containing `NewsArticle` + `Person`
- OG-only page (no JSON-LD)
- Page with no metadata at all
- Page with `<base href>` set
- Page with two tables, one large (200+ rows)
- Page with relative `src` images + alt text

**`tests/extractor_images.rs`** uses wiremock to serve test image responses;
requires `--features test-loopback` for SSRF.

**`tests/mcp_get_metadata.rs`** spawns `rover mcp` via `TokioChildProcess`
(same pattern as `mcp_smoke.rs`), exercises:
1. `tools/list` now includes `get_metadata_tool` alongside
   `fetch_tool` and `count_tokens_tool`.
2. `get_metadata` against a wiremock-served fixture page returns the
   expected fields.
3. `get_metadata` on a cached URL doesn't re-fetch (`cache_status:
   "hit"`).
4. Unknown family in `tokenizer` arg returns `invalid_args`.
5. Invalid URL returns `invalid_url`.

### Conventions carried forward

- `wiremock` for HTTP mocking; `test-loopback` feature for any SSRF
  interaction.
- Each integration test gets its own tempdir + `ROVER_DATA_DIR` +
  `ROVER_OUTPUT_DIR`.
- The `seed_default_tokenizer` helper from `mcp_smoke.rs` factors out
  into a shared `tests/common/` module so the new integration test
  doesn't duplicate it.
- TDD: each plan task starts with a failing test.

## Acceptance criteria

PRD §14 M4: "Complex pages produce well-structured frontmatter; large
tables don't blow token budgets; all links in output are absolute."

Concrete checks the plan must produce green:
- `cargo test --features test-loopback` passes the new suite (~30+ new
  unit/integration tests).
- Manual: fetch `https://en.wikipedia.org/wiki/Rust_(programming_language)`
  and confirm frontmatter has populated `description`, `image`,
  `language`, `extraction_quality`, `schema_types`, plus
  `tables_transformed` entries for any embedded tables.
- Manual: `rover fetch <url-with-relative-links>` produces markdown
  where every link in the body is an absolute URL.
- Manual: `rover fetch <url>` with `tables: { mode: "csv_file" }` writes
  CSVs under `$ROVER_OUTPUT_DIR/tables/<host>/` and frontmatter records
  absolute paths.

## Deferred from M4

- Tables `Summarize` mode body → M7. Schema accepts; runtime errors.
- Images `CaptionVlm` mode body → M9. Schema accepts; runtime errors.
- Microdata extraction → known limitation; revisit only if a real
  consumer asks.
- `Stratified` sample strategy → known limitation; HeadTail + RandomSeed
  cover the cases identified so far.
- `<img srcset>` recovery → known limitation; readabilityrs eats it.
- Sharing the parsed HTML tree between scraper and readabilityrs →
  performance optimisation; not worth it now.

## Out of scope (won't fix in M4)

- Streaming MCP responses for the new tools (rmcp doesn't natively
  stream; same M3 decision applies).
- Per-mode override of the default output_dir (e.g. `tables.output_dir`
  separate from `images.output_dir`). One `[output] dir` covers both.
- Background prefetch of `<img>` URLs to populate the cache before
  Download mode runs (would need M5/M6).

## Dependencies added in M4

- `csv` (table serialisation; latest stable at plan time)
- `rand = "0.9"` with `rand_chacha` for the seedable RNG used by
  `RandomSeed`. Confirm versions at plan time.
- `mime_guess` for image extension sniffing (if not transitively
  present; verify at plan time).

No new dev-deps; the existing `wiremock`, `tempfile`, `assert_cmd`,
`rmcp[client+transport-child-process]` cover the integration test surface.

## Forward-looking notes for later milestones

- **M5 (rate limiting)**: image Download requests should flow through the
  per-domain rate limiter once M5 lands. M4 issues these via the existing
  `reqwest::Client` directly. Document this as a known follow-up so M5's
  plan captures the integration point.
- **M6 (tasks)**: large image downloads could become deferred tasks if a
  page has dozens of images. M4 does them synchronously per-fetch. M6
  can later add a `batch_download_images` task if real workloads need it.
- **M7 (summarization)**: Tables `Summarize` mode wires in here, reusing
  the `[output] dir` and the same `tables_transformed` shape. The schema
  for `Summarize` is already in M4's wire surface.
- **M9 (VLM)**: Images `CaptionVlm` wires in here, replacing the M4
  runtime error. M9 adds the caption text as either a replacement or
  augmentation of the markdown image syntax (decision deferred to M9).
- **M8 (`rover doctor`)**: should validate that `$output_dir` is writable
  on startup; should warn if it contains stale files from a previous
  run (operator hygiene).

//! MCP `fetch` tool — wraps the M1/M2 pipeline behind a typed arg struct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::frontmatter::{PageMeta, render as render_frontmatter};
use crate::extractor::options::{ImagesMode, SampleStrategy, TablesMode};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{
    CacheStatus, CountResponse, CountSingleResponse, CountSource, FetchResponse,
};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

/// Wire-side `fetch` tool arguments.
///
/// Live in M3+M4+M7+M9: `url`, `force_refresh`, `count_only`, `tokenizer`,
/// `max_tokens`, `tables`, `images`, `metadata`, `summarize`, `headless`.
///
/// `tokenizer` is exposed as a string on the wire (rather than the
/// [`Tokenizer`] enum) so the JSON schema doesn't have to mirror the
/// enum's manual `Serialize`/`Deserialize` impls. Parsing happens inside
/// [`RoverHandler::fetch_inner`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    pub url: String,

    #[serde(default)]
    pub force_refresh: bool,

    #[serde(default)]
    pub count_only: bool,

    #[serde(default)]
    pub tokenizer: Option<String>,

    #[serde(default)]
    pub max_tokens: Option<usize>,

    #[serde(default)]
    pub tables: Option<TablesArg>,

    #[serde(default)]
    pub images: Option<ImagesArg>,

    #[serde(default)]
    pub metadata: Option<MetadataArg>,

    /// Inline summarize request. When present, the returned `markdown` is
    /// the summary of the extracted body (post tables/images passes), and
    /// `FetchResponse.summarized` is `true`. The shape mirrors
    /// [`crate::mcp::tools::summarize::SummarizeArgs`] minus the `url`.
    ///
    /// Any unset sub-field falls back to its default (mode/style/preserve/
    /// target_tokens/focus/backend from the `[summarization]` config; same
    /// defaults the standalone `summarize` tool uses).
    #[serde(default)]
    pub summarize: Option<InlineSummarizeArgs>,

    #[serde(default)]
    pub headless: Option<HeadlessArg>,
}

/// Inline `summarize` sub-arg for the `fetch` tool. Re-uses the same
/// enums as the standalone `summarize` tool so a single CLI/schema source
/// of truth covers both call sites.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InlineSummarizeArgs {
    #[serde(default)]
    pub target_tokens: Option<usize>,

    #[serde(default)]
    pub mode: Option<crate::mcp::tools::summarize::SummarizeMode>,

    #[serde(default)]
    pub focus: Option<String>,

    #[serde(default)]
    pub preserve: Vec<crate::mcp::tools::summarize::SummarizePreserve>,

    #[serde(default)]
    pub style: Option<crate::mcp::tools::summarize::SummarizeStyle>,

    #[serde(default)]
    pub backend: Option<String>,
}

/// Wire shape for `tables`.
///
/// Serializes via a custom flat shape (`{mode, strategy?, head?, tail?, rows?, seed?}`)
/// so that `deny_unknown_fields` semantics on the outer args still surface
/// stray keys inside the tables arg — `#[serde(flatten)]` is incompatible
/// with `deny_unknown_fields`, so we hand-roll the parser instead.
#[derive(Debug, Clone)]
pub enum TablesArg {
    Embed,
    Drop,
    CsvFile,
    Summarize,
    Sample { strategy: SampleArg },
}

#[derive(Debug, Clone)]
pub enum SampleArg {
    HeadTail { head: usize, tail: usize },
    RandomSeed { rows: usize, seed: u64 },
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct TablesArgWire {
    mode: TablesModeWire,
    #[serde(default)]
    strategy: Option<SampleStrategyWire>,
    #[serde(default)]
    head: Option<usize>,
    #[serde(default)]
    tail: Option<usize>,
    #[serde(default)]
    rows: Option<usize>,
    #[serde(default)]
    seed: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TablesModeWire {
    Embed,
    Drop,
    CsvFile,
    Summarize,
    Sample,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SampleStrategyWire {
    HeadTail,
    RandomSeed,
}

impl<'de> Deserialize<'de> for TablesArg {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let w = TablesArgWire::deserialize(deserializer)?;
        match w.mode {
            TablesModeWire::Embed => Ok(TablesArg::Embed),
            TablesModeWire::Drop => Ok(TablesArg::Drop),
            TablesModeWire::CsvFile => Ok(TablesArg::CsvFile),
            TablesModeWire::Summarize => Ok(TablesArg::Summarize),
            TablesModeWire::Sample => {
                let strategy = w.strategy.unwrap_or(SampleStrategyWire::HeadTail);
                let inner = match strategy {
                    SampleStrategyWire::HeadTail => SampleArg::HeadTail {
                        head: w.head.unwrap_or_else(default_head),
                        tail: w.tail.unwrap_or_else(default_tail),
                    },
                    SampleStrategyWire::RandomSeed => SampleArg::RandomSeed {
                        rows: w.rows.unwrap_or_else(default_random_rows),
                        seed: w.seed.unwrap_or_else(default_random_seed),
                    },
                };
                Ok(TablesArg::Sample { strategy: inner })
            }
        }
    }
}

impl Serialize for TablesArg {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let w = match self {
            TablesArg::Embed => TablesArgWire {
                mode: TablesModeWire::Embed,
                strategy: None,
                head: None,
                tail: None,
                rows: None,
                seed: None,
            },
            TablesArg::Drop => TablesArgWire {
                mode: TablesModeWire::Drop,
                strategy: None,
                head: None,
                tail: None,
                rows: None,
                seed: None,
            },
            TablesArg::CsvFile => TablesArgWire {
                mode: TablesModeWire::CsvFile,
                strategy: None,
                head: None,
                tail: None,
                rows: None,
                seed: None,
            },
            TablesArg::Summarize => TablesArgWire {
                mode: TablesModeWire::Summarize,
                strategy: None,
                head: None,
                tail: None,
                rows: None,
                seed: None,
            },
            TablesArg::Sample {
                strategy: SampleArg::HeadTail { head, tail },
            } => TablesArgWire {
                mode: TablesModeWire::Sample,
                strategy: Some(SampleStrategyWire::HeadTail),
                head: Some(*head),
                tail: Some(*tail),
                rows: None,
                seed: None,
            },
            TablesArg::Sample {
                strategy: SampleArg::RandomSeed { rows, seed },
            } => TablesArgWire {
                mode: TablesModeWire::Sample,
                strategy: Some(SampleStrategyWire::RandomSeed),
                head: None,
                tail: None,
                rows: Some(*rows),
                seed: Some(*seed),
            },
        };
        w.serialize(serializer)
    }
}

impl JsonSchema for TablesArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TablesArg".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::TablesArg").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <TablesArgWire as JsonSchema>::json_schema(generator)
    }
}

fn default_head() -> usize {
    5
}
fn default_tail() -> usize {
    5
}
fn default_random_rows() -> usize {
    10
}
fn default_random_seed() -> u64 {
    42
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "mode")]
pub enum ImagesArg {
    Keep,
    AltTextOnly,
    Download,
    Drop,
    /// Caption images via a configured captioner. Use `[image_captions]` /
    /// `[captioners.<name>]` in config; per-call override via `captioner`.
    Caption {
        #[serde(default)]
        captioner: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum MetadataArg {
    Include,
    Skip,
}

/// Wire shape for the `headless` arg.
///
/// All fields are optional; when omitted, `mode` falls back to the server's
/// `[headless] auto_detect_spa` config key (`Auto` when true, `Off` when false).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadlessArg {
    #[serde(default)]
    pub mode: Option<HeadlessModeWire>,
}

/// Wire variant for `headless.mode`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessModeWire {
    Off,
    On,
    Auto,
}

fn tables_mode(arg: Option<&TablesArg>) -> Result<TablesMode, McpError> {
    Ok(match arg {
        None | Some(TablesArg::Embed) => TablesMode::Embed,
        Some(TablesArg::Drop) => TablesMode::Drop,
        Some(TablesArg::CsvFile) => TablesMode::CsvFile,
        Some(TablesArg::Sample { strategy }) => match strategy {
            SampleArg::HeadTail { head, tail } => {
                if *head == 0 || *tail == 0 {
                    return Err(McpError::InvalidArgs(
                        "tables.sample head/tail must be > 0".into(),
                    ));
                }
                TablesMode::Sample(SampleStrategy::HeadTail {
                    head: *head,
                    tail: *tail,
                })
            }
            SampleArg::RandomSeed { rows, seed } => {
                if *rows == 0 {
                    return Err(McpError::InvalidArgs(
                        "tables.sample rows must be > 0".into(),
                    ));
                }
                TablesMode::Sample(SampleStrategy::RandomSeed {
                    rows: *rows,
                    seed: *seed,
                })
            }
        },
        Some(TablesArg::Summarize) => TablesMode::Summarize,
    })
}

fn images_mode(arg: Option<&ImagesArg>) -> Result<(ImagesMode, Option<String>), McpError> {
    Ok(match arg {
        None | Some(ImagesArg::AltTextOnly) => (ImagesMode::AltTextOnly, None),
        Some(ImagesArg::Keep) => (ImagesMode::Keep, None),
        Some(ImagesArg::Download) => (ImagesMode::Download, None),
        Some(ImagesArg::Drop) => (ImagesMode::Drop, None),
        Some(ImagesArg::Caption { captioner }) => (ImagesMode::Caption, captioner.clone()),
    })
}

/// Resolve `[image_captions]` defaults plus an optional per-call captioner
/// override into the budget knobs `extractor::images::apply` consumes.
fn build_caption_filters(
    cfg: &crate::config::ImageCaptionsConfig,
    override_name: Option<String>,
) -> crate::extractor::options::ImageCaptionFilters {
    crate::extractor::options::ImageCaptionFilters {
        max_per_page: cfg.max_per_page,
        min_width: cfg.min_width,
        min_height: cfg.min_height,
        max_bytes: cfg.max_bytes,
        max_tokens: cfg.max_tokens,
        captioner_override: override_name,
    }
}

/// Convert the optional wire `headless` arg into the fetcher's `HeadlessMode`.
///
/// When no arg (or no `mode` sub-field) is provided, the config's
/// `auto_detect_spa` flag drives the default: `Auto` when true, `Off` when
/// false.
fn resolve_headless(
    arg: Option<&HeadlessArg>,
    config: &crate::config::HeadlessConfig,
) -> crate::fetcher::cached::HeadlessMode {
    let mode = arg.and_then(|a| a.mode).map(|m| match m {
        HeadlessModeWire::Off => crate::fetcher::cached::HeadlessMode::Off,
        HeadlessModeWire::On => crate::fetcher::cached::HeadlessMode::On,
        HeadlessModeWire::Auto => crate::fetcher::cached::HeadlessMode::Auto,
    });
    mode.unwrap_or(if config.auto_detect_spa {
        crate::fetcher::cached::HeadlessMode::Auto
    } else {
        crate::fetcher::cached::HeadlessMode::Off
    })
}

/// One of the two response shapes the `fetch` tool can produce, depending
/// on `count_only`.
///
/// `JsonSchema` is implemented manually so the generated schema is rooted
/// at `type: "object"` with a `oneOf` of the two variants. The MCP spec
/// requires `outputSchema.type == "object"`, but the default schemars
/// derive for an `#[serde(untagged)]` enum emits a bare `oneOf` with no
/// root type, which rmcp's `schema_for_output` rejects at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FetchOutput {
    Full(FetchResponse),
    Count(CountResponse),
}

impl JsonSchema for FetchOutput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FetchOutput".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::FetchOutput").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let full = generator.subschema_for::<FetchResponse>();
        let count = generator.subschema_for::<CountResponse>();
        schemars::json_schema!({
            "type": "object",
            "oneOf": [full, count],
        })
    }
}

/// Per-call summarize bookkeeping carried through `fetch_inner` into the
/// response envelope.
#[derive(Debug, Clone)]
struct SummarizeOutcome {
    summarized: bool,
    fallback: Option<crate::mcp::envelope::SummarizerFallbackInfo>,
}

impl RoverHandler {
    /// Run the summarizer against `body_md` with `opts` and lift the result
    /// into the wire envelope's fallback shape.
    async fn run_compact(
        &self,
        body_md: &str,
        opts: &crate::summarizer::backend::CompactOpts,
    ) -> Result<(String, Option<crate::mcp::envelope::SummarizerFallbackInfo>), McpError> {
        let content_hash = format!(
            "sha256:{}",
            crate::fetcher::cached::sha256_hex(body_md.as_bytes()),
        );
        let r = self
            .summarizer
            .compact(&content_hash, body_md, opts)
            .await?;
        // FallbackInfo.reason is &'static str (stable enum-ish); on the wire
        // it's String because serde Deserialize needs an owned String
        // symmetrically.
        let fallback = r
            .fallback
            .map(|f| crate::mcp::envelope::SummarizerFallbackInfo {
                from: f.from,
                reason: f.reason.to_string(),
            });
        Ok((r.summary_md, fallback))
    }

    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    /// Task 11 wires this into the router; here it's a plain async method.
    pub async fn fetch_inner(&self, args: FetchArgs) -> Result<FetchOutput, McpError> {
        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        if matches!(args.max_tokens, Some(0)) {
            return Err(McpError::InvalidArgs(
                "max_tokens must be greater than 0".into(),
            ));
        }
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;

        let headless_mode = resolve_headless(args.headless.as_ref(), &self.config.headless);

        // M9 fix C1: lazily build a `HeadlessRenderer` the first time a
        // request actually wants one (`On`, or `Auto` — the cached fetcher
        // only re-renders under Auto when the SPA heuristic fires, but we
        // still hand it the renderer so it has the option). The renderer
        // lives in a process-shared `OnceCell` on the handler, so subsequent
        // fetches reuse the same Chromium instance.
        #[cfg(feature = "headless")]
        let headless: Option<std::sync::Arc<crate::fetcher::headless::HeadlessRenderer>> =
            if !matches!(headless_mode, crate::fetcher::cached::HeadlessMode::Off) {
                let headless_cfg = self.config.headless.clone();
                let renderer = self
                    .headless_renderer
                    .get_or_try_init(|| async move {
                        crate::fetcher::headless::HeadlessRenderer::new(&headless_cfg)
                            .await
                            .map(std::sync::Arc::new)
                    })
                    .await
                    .map_err(|e: crate::fetcher::headless::HeadlessError| {
                        McpError::Fetcher(crate::fetcher::FetcherError::Headless(e))
                    })?;
                Some(renderer.clone())
            } else {
                None
            };

        let result = fetch_with_cache(
            &self.db,
            &self.client,
            &self.pacer,
            &self.config.rate_limit,
            &self.config.robots,
            &url,
            &self.config.cache,
            FetchOptions {
                force_refresh: args.force_refresh,
                ssrf_level: self.ssrf_level,
                ssrf_project_root: self.ssrf_project_root.clone(),
                har_recorder: self.har_recorder.clone(),
                ignore_robots: false,
                user_agent: self.config.fetch.user_agent.clone(),
                #[cfg(feature = "headless")]
                headless,
                headless_mode,
                synchronous_revalidation: false,
            },
            |body, base| {
                let extracted =
                    extract(body, Some(base)).map_err(crate::fetcher::FetcherError::Extract)?;
                let content_hash = format!("sha256:{}", sha256_hex(extracted.body_md.as_bytes()));
                Ok(ExtractResult {
                    title: extracted.title,
                    body_md: extracted.body_md,
                    content_hash,
                    metadata: extracted.metadata,
                })
            },
        )
        .await?;

        tokenizer::ensure_loaded(family).await?;

        // Resolve per-request post-pass modes from the typed args.
        let output_paths = std::sync::Arc::new(
            crate::extractor::output::OutputPaths::resolve(self.config.output.dir.as_deref())
                .map_err(McpError::Extractor)?,
        );

        let tables_mode_resolved = tables_mode(args.tables.as_ref())?;
        let (images_mode_resolved, captioner_override) = images_mode(args.images.as_ref())?;
        let caption_filters =
            build_caption_filters(&self.config.image_captions, captioner_override);

        // Run the M4 post-passes against the cached (pre-pass) body. These
        // always run, even on cache hits: the cached `extracted_md` carries
        // links-absolutized but no tables/images transforms.
        let body_md = result.page.extracted_md.clone();
        let tables_hook: Option<crate::extractor::tables::TableSummarizeHook> =
            if matches!(tables_mode_resolved, TablesMode::Summarize) {
                let summarizer = self.summarizer.clone();
                let config = self.config.clone();
                Some(std::sync::Arc::new(move |table_text: &str| {
                    let summarizer = summarizer.clone();
                    let config = config.clone();
                    let table_text = table_text.to_string();
                    Box::pin(async move {
                        let defaults =
                            crate::summarizer::DefaultsHint::from_config(&config.summarization);
                        let opts = crate::summarizer::backend::CompactOpts {
                            mode: defaults.mode,
                            style: crate::summarizer::backend::Style::Bullet,
                            target_tokens: Some(config.summarization.tables.target_tokens),
                            focus: Some(config.summarization.tables.focus.clone()),
                            preserve: vec![],
                            backend_name: defaults.backend.clone(),
                        };
                        let content_hash = format!("sha256:{}", sha256_hex(table_text.as_bytes()));
                        summarizer
                            .compact(&content_hash, &table_text, &opts)
                            .await
                            .map(|r| {
                                let fb =
                                    r.fallback.map(|f| crate::extractor::tables::FallbackInfo {
                                        from: f.from,
                                        reason: f.reason.to_string(),
                                    });
                                if let Some(fb) = &fb {
                                    tracing::debug!(
                                        target: "rover::mcp",
                                        from = %fb.from,
                                        reason = %fb.reason,
                                        "table summarizer fell back to extractive",
                                    );
                                }
                                (r.summary_md, fb)
                            })
                            .map_err(|e| e.fallback_reason().to_string())
                    })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<
                                        Output = Result<
                                            (
                                                String,
                                                Option<crate::extractor::tables::FallbackInfo>,
                                            ),
                                            String,
                                        >,
                                    > + Send,
                            >,
                        >
                }))
            } else {
                None
            };
        let (body_md, tables_transformed) = crate::extractor::tables::apply_with_summarizer(
            &body_md,
            &tables_mode_resolved,
            &output_paths,
            &url,
            tables_hook.as_ref(),
        )
        .await
        .map_err(McpError::Extractor)?;

        let captioners_opt = if self.captioners.is_empty() {
            None
        } else {
            Some(self.captioners.as_ref())
        };
        let images_result = crate::extractor::images::apply(
            &body_md,
            &images_mode_resolved,
            &output_paths,
            &self.client,
            captioners_opt,
            &caption_filters,
            Some(&self.db),
            self.ssrf_level,
        )
        .await
        .map_err(McpError::Extractor)?;
        let body_md = images_result.markdown;

        // M7: optional inline `summarize` arg runs first against the
        // post-pass body. If the agent provided this, the returned
        // `markdown` is the summary.
        let (body_md, summarize_meta): (String, Option<SummarizeOutcome>) = if let Some(inline) =
            args.summarize.clone()
        {
            let defaults = crate::summarizer::DefaultsHint::from_config(&self.config.summarization);
            let opts = self.summarizer.resolve_defaults(
                inline.mode.map(Into::into),
                inline.style.map(Into::into),
                inline.target_tokens,
                inline.focus,
                inline.preserve.into_iter().map(Into::into).collect(),
                inline.backend,
                &defaults,
            );
            let (summary_md, fallback) = self.run_compact(&body_md, &opts).await?;
            (
                summary_md,
                Some(SummarizeOutcome {
                    summarized: true,
                    fallback,
                }),
            )
        } else {
            (body_md, None)
        };

        // Recompute tokens against the (possibly summarized) body; `max_tokens`
        // constrains what the agent will actually see.
        let tokens = tokenizer::count(&body_md, family)?;

        // M7: auto-summarize on `max_tokens` overflow. Single-shot: if the
        // resulting summary is still over budget, return MaxTokensExceeded.
        // If the agent already supplied an explicit `summarize` arg, don't
        // override that choice — surface the error directly.
        let (body_md, tokens, auto_meta): (String, usize, Option<SummarizeOutcome>) =
            if let Some(max) = args.max_tokens {
                if tokens <= max {
                    (body_md, tokens, None)
                } else if summarize_meta.is_some() {
                    return Err(McpError::MaxTokensExceeded {
                        actual: tokens,
                        max,
                        was_auto: false,
                    });
                } else if args.count_only {
                    // count_only is a probe: tell the agent the real size
                    // without auto-correction. Don't summarize — fall through
                    // to the count_only short-circuit which reports the real
                    // (over-budget) token count.
                    (body_md, tokens, None)
                } else {
                    let defaults =
                        crate::summarizer::DefaultsHint::from_config(&self.config.summarization);
                    let opts = self.summarizer.resolve_defaults(
                        None,
                        None,
                        Some(max),
                        None,
                        vec![],
                        None,
                        &defaults,
                    );
                    let (summary_md, fallback) = self.run_compact(&body_md, &opts).await?;
                    let new_tokens = tokenizer::count(&summary_md, family)?;
                    if new_tokens > max {
                        return Err(McpError::MaxTokensExceeded {
                            actual: new_tokens,
                            max,
                            was_auto: true,
                        });
                    }
                    (
                        summary_md,
                        new_tokens,
                        Some(SummarizeOutcome {
                            summarized: true,
                            fallback,
                        }),
                    )
                }
            } else {
                (body_md, tokens, None)
            };

        // Build the optional SWR envelope before lowering `cache_status` to the
        // unit-variant wire enum.
        let revalidation = match &result.cache_status {
            crate::fetcher::cached::CacheStatus::Stale {
                revalidation_task_id: Some(id),
            } => Some(crate::mcp::envelope::StaleRevalidation {
                task_id: id.clone(),
                monitor_command: format!("rover task {id} --monitor"),
                poll_command: format!("rover task {id}"),
                hint: "Optional. Revalidation runs in the background regardless.".into(),
            }),
            _ => None,
        };

        let cache_status: CacheStatus = result.cache_status.into();

        if args.count_only {
            // `result.page.content_hash` is already prefixed (`sha256:...`)
            // by the `extract_fn` above; pass it through verbatim.
            return Ok(FetchOutput::Count(CountResponse::Single(
                CountSingleResponse {
                    tokens,
                    tokenizer: family.as_str().to_string(),
                    source: CountSource::Url,
                    url: Some(url.as_str().to_string()),
                    content_hash: Some(result.page.content_hash.clone()),
                    fetched_at: Some(
                        jiff::Timestamp::from_second(result.page.fetched_at)
                            .map(|t| t.to_string())
                            .unwrap_or_default(),
                    ),
                    cache_status: Some(cache_status),
                },
            )));
        }

        let canonical = Url::parse(&result.page.canonical_url)
            .map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        // Recover the metadata persisted in the cache row. See cli/fetch.rs
        // for the rationale on the `raw_html_text_len` fallback used by the
        // quality scorer.
        let metadata: crate::extractor::ExtractedMetadata = result
            .page
            .metadata_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        // Honor MetadataArg::Skip: hide all metadata from the response.
        // (The cache row still carries metadata_json — only the wire output is blanked.)
        let metadata = match args.metadata.as_ref() {
            Some(MetadataArg::Skip) => crate::extractor::ExtractedMetadata::default(),
            _ => metadata,
        };
        let quality = crate::extractor::quality::score(
            &body_md,
            body_md.chars().count().max(1),
            !metadata.is_empty(),
            result.page.title.is_some(),
        );
        let frontmatter = render_frontmatter(&PageMeta {
            url: &url,
            canonical_url: &canonical,
            title: result.page.title.as_deref(),
            fetched_at: jiff::Timestamp::now(),
            body: &body_md,
            tokens,
            tokenizer_name: family.as_str(),
            description: metadata.description.as_deref(),
            author: metadata.author.as_deref(),
            published: metadata.published.as_deref(),
            modified: metadata.modified.as_deref(),
            image: metadata.image.as_deref(),
            og_type: metadata.og_type.as_deref(),
            language: metadata.language.as_deref(),
            schema_types: &metadata.schema_types,
            extraction_quality: quality,
            tables_transformed: &tables_transformed,
            images_seen: images_result.images_seen,
            images_downloaded: images_result.images_downloaded,
            images_failed: images_result.images_failed,
            images_processed: images_result.images_processed.clone(),
            prompt_injection: None,
        });

        let summarized_flag = summarize_meta.as_ref().map(|o| o.summarized);
        let auto_summarized_flag = auto_meta.as_ref().map(|o| o.summarized);
        let summarizer_fallback = summarize_meta
            .and_then(|o| o.fallback)
            .or_else(|| auto_meta.and_then(|o| o.fallback));

        Ok(FetchOutput::Full(FetchResponse {
            markdown: body_md,
            frontmatter,
            cache_status,
            revalidation,
            summarized: summarized_flag,
            auto_summarized: auto_summarized_flag,
            summarizer_fallback,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::tokenizer::Tokenizer;

    #[test]
    fn fetch_args_deserialize_minimal() {
        let v: FetchArgs = serde_json::from_str(r#"{"url":"https://example.com"}"#).unwrap();
        assert_eq!(v.url, "https://example.com");
        assert!(!v.force_refresh);
        assert!(!v.count_only);
        assert!(v.tokenizer.is_none());
        assert!(v.max_tokens.is_none());
    }

    #[test]
    fn fetch_args_headless_typed_mode_auto() {
        let v: FetchArgs = serde_json::from_str(
            r#"{
                "url":"https://example.com",
                "headless": { "mode": "auto" }
            }"#,
        )
        .unwrap();
        let h = v.headless.expect("headless parsed");
        assert!(matches!(h.mode, Some(HeadlessModeWire::Auto)));
    }

    #[test]
    fn fetch_args_parse_typed_summarize() {
        let v: FetchArgs = serde_json::from_str(
            r#"{
                "url":"https://example.com",
                "summarize":{
                    "target_tokens":500,
                    "mode":"extractive",
                    "style":"bullet",
                    "preserve":["code","tables"]
                }
            }"#,
        )
        .unwrap();
        let s = v.summarize.expect("summarize parsed");
        assert_eq!(s.target_tokens, Some(500));
        assert!(matches!(
            s.mode,
            Some(crate::mcp::tools::summarize::SummarizeMode::Extractive)
        ));
        assert!(matches!(
            s.style,
            Some(crate::mcp::tools::summarize::SummarizeStyle::Bullet)
        ));
        assert_eq!(s.preserve.len(), 2);
    }

    #[test]
    fn fetch_args_reject_unknown_summarize_field() {
        let r: Result<FetchArgs, _> =
            serde_json::from_str(r#"{"url":"https://x/","summarize":{"bogus":1}}"#);
        assert!(r.is_err());
    }

    #[test]
    fn fetch_args_reject_unknown_fields() {
        let r: Result<FetchArgs, _> =
            serde_json::from_str(r#"{"url":"https://example.com","bogus":1}"#);
        assert!(r.is_err());
    }

    #[test]
    fn fetch_args_parse_tokenizer_string() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://example.com","tokenizer":"claude"}"#).unwrap();
        assert_eq!(v.tokenizer.as_deref(), Some("claude"));
        // And the string parses to the enum variant we expect.
        let t = Tokenizer::from_str(v.tokenizer.as_deref().unwrap()).unwrap();
        assert_eq!(t, Tokenizer::Claude);
    }

    #[test]
    fn fetch_args_schema_contains_all_documented_fields() {
        let schema = schemars::schema_for!(FetchArgs);
        let json = serde_json::to_string(&schema).unwrap();
        for field in [
            "url",
            "force_refresh",
            "count_only",
            "tokenizer",
            "max_tokens",
            "headless",
            "tables",
            "images",
            "metadata",
            "summarize",
        ] {
            assert!(json.contains(field), "schema missing field: {field}");
        }
    }

    #[test]
    fn typed_tables_sample_parses() {
        let v: FetchArgs = serde_json::from_str(
            r#"{"url":"https://x/","tables":{"mode":"sample","strategy":"head_tail","head":3,"tail":2}}"#,
        )
        .unwrap();
        match v.tables.unwrap() {
            TablesArg::Sample {
                strategy: SampleArg::HeadTail { head, tail },
            } => {
                assert_eq!(head, 3);
                assert_eq!(tail, 2);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn typed_tables_rejects_unknown_field() {
        let r: Result<FetchArgs, _> =
            serde_json::from_str(r#"{"url":"https://x/","tables":{"mode":"embed","bogus":1}}"#);
        assert!(r.is_err());
    }

    #[test]
    fn typed_images_download_parses() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://x/","images":{"mode":"download"}}"#).unwrap();
        assert!(matches!(v.images, Some(ImagesArg::Download)));
    }

    #[test]
    fn typed_images_caption_parses_without_captioner() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://x/","images":{"mode":"caption"}}"#).unwrap();
        assert!(matches!(
            v.images,
            Some(ImagesArg::Caption { captioner: None })
        ));
    }

    #[test]
    fn typed_images_caption_parses_with_captioner_override() {
        let v: FetchArgs = serde_json::from_str(
            r#"{"url":"https://x/","images":{"mode":"caption","captioner":"gpt4o"}}"#,
        )
        .unwrap();
        assert!(matches!(
            v.images,
            Some(ImagesArg::Caption { captioner: Some(ref s) }) if s == "gpt4o"
        ));
    }

    #[test]
    fn typed_metadata_skip_parses() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://x/","metadata":"skip"}"#).unwrap();
        assert!(matches!(v.metadata, Some(MetadataArg::Skip)));
    }
}

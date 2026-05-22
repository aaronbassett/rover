//! MCP `fetch` tool — wraps the M1/M2 pipeline behind a typed arg struct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::frontmatter::{PageMeta, render as render_frontmatter};
use crate::extractor::options::{ImagesMode, SampleStrategy, TablesMode};
use crate::extractor::pipeline::extract;
use crate::fetcher::cached::{ExtractResult, FetchOptions, fetch_with_cache, sha256_hex};
use crate::mcp::envelope::{CacheStatus, CountResponse, CountSource, FetchResponse};
use crate::mcp::error::McpError;
use crate::mcp::handler::{RoverHandler, resolve_tokenizer};
use crate::tokenizer;

/// Wire-side `fetch` tool arguments.
///
/// Live in M3+M4+M7: `url`, `force_refresh`, `count_only`, `tokenizer`,
/// `max_tokens`, `tables`, `images`, `metadata`, `summarize`. Accept-no-op
/// (schema-stable, body-deferred): `headless`. Their values are accepted by
/// the schema and emit one `tracing::debug` line each.
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
    #[serde(default)]
    pub summarize: Option<InlineSummarizeArgs>,

    // ---- accept-no-op until later milestones ----
    #[serde(default)]
    pub headless: Option<serde_json::Value>,
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
    CaptionVlm,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum MetadataArg {
    Include,
    Skip,
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
        Some(TablesArg::Summarize) => {
            return Err(McpError::Extractor(
                crate::extractor::pipeline::ExtractorError::Metadata(
                    "tables summarize mode is not available until M7".into(),
                ),
            ));
        }
    })
}

fn images_mode(arg: Option<&ImagesArg>) -> Result<ImagesMode, McpError> {
    Ok(match arg {
        None | Some(ImagesArg::AltTextOnly) => ImagesMode::AltTextOnly,
        Some(ImagesArg::Keep) => ImagesMode::Keep,
        Some(ImagesArg::Download) => ImagesMode::Download,
        Some(ImagesArg::Drop) => ImagesMode::Drop,
        Some(ImagesArg::CaptionVlm) => {
            return Err(McpError::Extractor(
                crate::extractor::pipeline::ExtractorError::Metadata(
                    "images caption_vlm mode requires the vlm feature (M9)".into(),
                ),
            ));
        }
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

impl RoverHandler {
    /// Tool body, decoupled from the `#[tool]` macro for unit testing.
    /// Task 11 wires this into the router; here it's a plain async method.
    pub async fn fetch_inner(&self, args: FetchArgs) -> Result<FetchOutput, McpError> {
        log_deferred_args(&args);

        let url = Url::parse(&args.url).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
        let family = resolve_tokenizer(args.tokenizer.as_deref(), &self.config)?;

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
                ignore_robots: false,
                user_agent: self.config.fetch.user_agent.clone(),
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
        let images_mode_resolved = images_mode(args.images.as_ref())?;

        // Run the M4 post-passes against the cached (pre-pass) body. These
        // always run, even on cache hits: the cached `extracted_md` carries
        // links-absolutized but no tables/images transforms.
        let body_md = result.page.extracted_md.clone();
        let (body_md, tables_transformed) =
            crate::extractor::tables::apply(&body_md, &tables_mode_resolved, &output_paths, &url)
                .map_err(McpError::Extractor)?;

        let images_result = crate::extractor::images::apply(
            &body_md,
            &images_mode_resolved,
            &output_paths,
            &self.client,
        )
        .await
        .map_err(McpError::Extractor)?;
        let body_md = images_result.markdown;

        // M7: optional inline `summarize` arg runs first against the
        // post-pass body. If the agent provided this, the returned
        // `markdown` is the summary.
        let (body_md, summarize_meta): (
            String,
            Option<(bool, Option<crate::mcp::envelope::SummarizerFallbackInfo>)>,
        ) = if let Some(inline) = args.summarize.clone() {
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
            let content_hash = format!("sha256:{}", sha256_hex(body_md.as_bytes()));
            let r = self
                .summarizer
                .compact(&content_hash, &body_md, &opts)
                .await?;
            let fallback = r
                .fallback
                .map(|f| crate::mcp::envelope::SummarizerFallbackInfo {
                    from: f.from,
                    reason: f.reason.to_string(),
                });
            (r.summary_md, Some((true, fallback)))
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
        let (body_md, tokens, auto_meta): (
            String,
            usize,
            Option<(bool, Option<crate::mcp::envelope::SummarizerFallbackInfo>)>,
        ) = if let Some(max) = args.max_tokens {
            if tokens <= max {
                (body_md, tokens, None)
            } else if summarize_meta.is_some() {
                return Err(McpError::MaxTokensExceeded {
                    actual: tokens,
                    max,
                });
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
                let content_hash = format!("sha256:{}", sha256_hex(body_md.as_bytes()));
                let r = self
                    .summarizer
                    .compact(&content_hash, &body_md, &opts)
                    .await?;
                let new_tokens = tokenizer::count(&r.summary_md, family)?;
                if new_tokens > max {
                    return Err(McpError::MaxTokensExceeded {
                        actual: new_tokens,
                        max,
                    });
                }
                let fallback = r
                    .fallback
                    .map(|f| crate::mcp::envelope::SummarizerFallbackInfo {
                        from: f.from,
                        reason: f.reason.to_string(),
                    });
                (r.summary_md, new_tokens, Some((true, fallback)))
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
            return Ok(FetchOutput::Count(CountResponse {
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
            }));
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
        });

        let summarized_flag = summarize_meta.as_ref().map(|(b, _)| *b);
        let auto_summarized_flag = auto_meta.as_ref().map(|(b, _)| *b);
        let summarizer_fallback = summarize_meta
            .and_then(|(_, f)| f)
            .or_else(|| auto_meta.and_then(|(_, f)| f));

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

fn log_deferred_args(args: &FetchArgs) {
    if let Some(v) = &args.headless {
        tracing::debug!(target: "rover::mcp", arg = "headless", value = ?v, "ignored until M9");
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
    fn fetch_args_accept_deferred_keys_as_no_op() {
        let v: FetchArgs = serde_json::from_str(
            r#"{
                "url":"https://example.com",
                "headless":"auto"
            }"#,
        )
        .unwrap();
        assert!(v.headless.is_some());
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
    fn typed_metadata_skip_parses() {
        let v: FetchArgs =
            serde_json::from_str(r#"{"url":"https://x/","metadata":"skip"}"#).unwrap();
        assert!(matches!(v.metadata, Some(MetadataArg::Skip)));
    }
}

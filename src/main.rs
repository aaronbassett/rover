use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rover", version, about = "Web fetch & prep for LLM agents")]
struct Cli {
    /// Path to a TOML config file. If absent, defaults are used.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the MCP server (long-running). M3.
    Mcp(McpArgs),

    /// One-shot fetch, prints markdown to stdout.
    Fetch(FetchArgs),

    /// Inspect or monitor a batch_fetch task (alias for `rover task` with a kind check).
    Batch {
        id: String,
        #[arg(long)]
        monitor: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Stream events starting after this event id (use with --monitor).
        #[arg(long)]
        from_event: Option<i64>,
    },

    /// Inspect or monitor a long-running task.
    Task {
        id: String,
        #[arg(long)]
        monitor: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Stream events starting after this event id (use with --monitor).
        #[arg(long)]
        from_event: Option<i64>,
    },

    /// Cache operations (M2).
    #[command(subcommand)]
    Cache(CacheCmd),

    /// Verify the Rover environment (M8).
    Doctor(DoctorArgs),

    /// Inspect or modify config (M8).
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Manage local HuggingFace model cache (M9).
    #[cfg(any(feature = "local-inference", feature = "local-vision"))]
    #[command(subcommand)]
    Model(rover::cli::model::ModelCmd),
}

#[derive(Debug, clap::Args)]
struct FetchArgs {
    /// URL to fetch.
    url: String,

    /// Bypass the cache for this fetch and always go out to the network.
    #[arg(long)]
    force_refresh: bool,

    /// Skip the robots.txt gate for this fetch. CLI-only escape hatch.
    #[arg(long)]
    ignore_robots: bool,

    /// Override [rate_limit] requests_per_minute_per_domain.
    #[arg(long)]
    rate_limit_rpm: Option<u32>,

    /// Override [rate_limit] per_domain_concurrency.
    #[arg(long)]
    per_host_concurrency: Option<u32>,

    /// Override [rate_limit] global_concurrency.
    #[arg(long)]
    global_concurrency: Option<u32>,

    /// Override [rate_limit] max_retries.
    #[arg(long)]
    max_retries: Option<u8>,

    /// Auto-summarize when extracted markdown exceeds N tokens. **v1
    /// note:** the canonical auto-summarize path is the MCP `fetch` tool;
    /// the CLI parses and validates this flag but does not yet apply
    /// summarization here.
    #[arg(long)]
    max_tokens: Option<usize>,

    /// JSON `SummarizeOpts` blob (same shape as the MCP `summarize` args
    /// without `url`). **v1 note:** validated but not yet applied in the
    /// CLI path; use the MCP `summarize` tool for the canonical surface.
    #[arg(long, value_name = "JSON")]
    summarize: Option<String>,
}

#[derive(Debug, clap::Args)]
struct McpArgs {
    /// Disable the robots.txt gate for the lifetime of this server. All MCP
    /// fetch tools will skip the robots check.
    #[arg(long)]
    ignore_robots: bool,

    /// Override [rate_limit] requests_per_minute_per_domain.
    #[arg(long)]
    rate_limit_rpm: Option<u32>,

    /// Override [rate_limit] per_domain_concurrency.
    #[arg(long)]
    per_host_concurrency: Option<u32>,

    /// Override [rate_limit] global_concurrency.
    #[arg(long)]
    global_concurrency: Option<u32>,

    /// Override [rate_limit] max_retries.
    #[arg(long)]
    max_retries: Option<u8>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    Human,
    Ndjson,
}

impl From<OutputFormat> for rover::cli::task::OutputFormat {
    fn from(value: OutputFormat) -> Self {
        match value {
            OutputFormat::Human => rover::cli::task::OutputFormat::Human,
            OutputFormat::Ndjson => rover::cli::task::OutputFormat::Ndjson,
        }
    }
}

#[derive(Debug, Subcommand)]
enum CacheCmd {
    /// List cached URLs (most recent first).
    List {
        #[arg(long, default_value_t = 20)]
        limit: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Print the cached Markdown for a URL.
    Get { url: String },
    /// Delete cache entries matching a glob (`*`, `?`).
    Purge {
        pattern: String,
        /// Required to wipe the entire cache (`*` pattern).
        #[arg(long)]
        all: bool,
    },
    /// Show cache size, entry count, expired count.
    Stats,
}

impl CacheCmd {
    fn into_runtime_args(self) -> rover::cli::cache::Args {
        match self {
            CacheCmd::List { limit, offset } => rover::cli::cache::Args::List { limit, offset },
            CacheCmd::Get { url } => rover::cli::cache::Args::Get { url },
            CacheCmd::Purge { pattern, all } => rover::cli::cache::Args::Purge { pattern, all },
            CacheCmd::Stats => rover::cli::cache::Args::Stats,
        }
    }
}

#[derive(Debug, Subcommand)]
enum ConfigCmd {
    Show,
    Set { key: String, value: String },
}

#[derive(Debug, clap::Args)]
struct DoctorArgs {
    /// Output format: `human` (default) prints one line per check;
    /// `ndjson` emits one JSON object per line for scripting.
    #[arg(long, default_value = "human")]
    format: String,
}

/// Build a `SummarizerService` for CLI subcommands that need it.
///
/// The MCP server builds its own service inside `serve_stdio`; this helper
/// exists for CLI paths (e.g. a future `rover fetch --summarize`) so the
/// construction stays in one place. Currently unused outside the MCP path —
/// `#[allow(dead_code)]` keeps `warnings = deny` happy until M7's CLI
/// summarize wiring lands.
#[allow(dead_code)]
async fn build_summarizer_service(
    db: rover::storage::Db,
    config: &rover::config::Config,
) -> anyhow::Result<Arc<rover::summarizer::SummarizerService>> {
    let registry = Arc::new(
        rover::summarizer::registry::build(config, config.tokenizer.default)
            .map_err(anyhow::Error::from)?,
    );
    Ok(Arc::new(rover::summarizer::SummarizerService::new(
        db,
        registry,
        config.summarization.fallback_to_extractive,
    )))
}

fn main() -> ExitCode {
    rover::telemetry::init("info,rover=debug");
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(dispatch(cli))
}

async fn dispatch(cli: Cli) -> ExitCode {
    let result = match cli.command {
        Command::Fetch(args) => {
            rover::cli::fetch::run(args.into_runtime_args(), cli.config.as_deref()).await
        }
        Command::Cache(sub) => {
            let args = sub.into_runtime_args();
            rover::cli::cache::run(args, cli.config.as_deref()).await
        }
        Command::Mcp(args) => {
            rover::cli::mcp::run(args.into_runtime_args(), cli.config.as_deref()).await
        }
        Command::Task {
            id,
            monitor,
            cancel,
            format,
            from_event,
        } => {
            rover::cli::task::run(
                rover::cli::task::Args {
                    id,
                    monitor,
                    cancel,
                    format: format.into(),
                    from_event,
                    expect_kind: None,
                },
                cli.config.as_deref(),
            )
            .await
        }
        Command::Batch {
            id,
            monitor,
            cancel,
            format,
            from_event,
        } => {
            rover::cli::batch::run(
                rover::cli::task::Args {
                    id,
                    monitor,
                    cancel,
                    format: format.into(),
                    from_event,
                    expect_kind: Some("batch_fetch"),
                },
                cli.config.as_deref(),
            )
            .await
        }
        Command::Doctor(args) => {
            let cfg = match rover::config::load(cli.config.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("rover: loading config: {e}");
                    return ExitCode::from(1);
                }
            };
            return match rover::cli::doctor::run(
                rover::cli::doctor::Args {
                    format: args.format,
                },
                cfg,
            )
            .await
            {
                Ok(code) => ExitCode::from(code as u8),
                Err(e) => {
                    eprintln!("rover: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Config(cmd) => match cmd {
            ConfigCmd::Show => {
                let res = rover::cli::config::show(rover::cli::config::ShowArgs {
                    config_path: cli.config.clone(),
                });
                return match res {
                    Ok(code) => ExitCode::from(code as u8),
                    Err(e) => {
                        eprintln!("rover: {e}");
                        ExitCode::from(1)
                    }
                };
            }
            ConfigCmd::Set { key, value } => {
                let res = rover::cli::config::set(rover::cli::config::SetArgs {
                    config_path: cli.config.clone(),
                    key,
                    value,
                });
                return match res {
                    Ok(code) => ExitCode::from(code as u8),
                    Err(e) => {
                        eprintln!("rover: {e}");
                        ExitCode::from(1)
                    }
                };
            }
        },
        #[cfg(any(feature = "local-inference", feature = "local-vision"))]
        Command::Model(cmd) => rover::cli::model::run(cmd).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rover: {e}");
            ExitCode::from(1)
        }
    }
}

impl FetchArgs {
    fn into_runtime_args(self) -> rover::cli::fetch::Args {
        rover::cli::fetch::Args {
            url: self.url,
            force_refresh: self.force_refresh,
            ignore_robots: self.ignore_robots,
            rate_limit_rpm: self.rate_limit_rpm,
            per_host_concurrency: self.per_host_concurrency,
            global_concurrency: self.global_concurrency,
            max_retries: self.max_retries,
            max_tokens: self.max_tokens,
            summarize: self.summarize,
        }
    }
}

impl McpArgs {
    fn into_runtime_args(self) -> rover::cli::mcp::Args {
        rover::cli::mcp::Args {
            ignore_robots: self.ignore_robots,
            rate_limit_rpm: self.rate_limit_rpm,
            per_host_concurrency: self.per_host_concurrency,
            global_concurrency: self.global_concurrency,
            max_retries: self.max_retries,
        }
    }
}

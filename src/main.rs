use std::path::PathBuf;
use std::process::ExitCode;

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
    Mcp,

    /// One-shot fetch, prints markdown to stdout.
    Fetch(FetchArgs),

    /// Long-running batch status (M6).
    Batch {
        id: String,
        #[arg(long)]
        monitor: bool,
    },

    /// Generic task status (M6).
    Task {
        id: String,
        #[arg(long)]
        monitor: bool,
        #[arg(long)]
        cancel: bool,
    },

    /// Cache operations (M2).
    #[command(subcommand)]
    Cache(CacheCmd),

    /// Verify the Rover environment (M8).
    Doctor,

    /// Inspect or modify config (M8).
    #[command(subcommand)]
    Config(ConfigCmd),
}

#[derive(Debug, clap::Args)]
struct FetchArgs {
    /// URL to fetch.
    url: String,

    /// **Test-only.** Allow loopback addresses to satisfy SSRF checks. Used by
    /// the integration test suite against wiremock; never used in production.
    #[cfg(any(test, feature = "test-loopback"))]
    #[arg(long, hide = true)]
    ssrf_test_loopback: bool,
}

#[derive(Debug, Subcommand)]
enum CacheCmd {
    List,
    Get { url: String },
    Purge { pattern: String },
    Stats,
}

#[derive(Debug, Subcommand)]
enum ConfigCmd {
    Show,
    Set { key: String, value: String },
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
        Command::Mcp
        | Command::Batch { .. }
        | Command::Task { .. }
        | Command::Cache(_)
        | Command::Doctor
        | Command::Config(_) => {
            eprintln!("not yet implemented (planned for a later milestone)");
            return ExitCode::from(2);
        }
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
            #[cfg(any(test, feature = "test-loopback"))]
            ssrf_test_loopback: self.ssrf_test_loopback,
        }
    }
}

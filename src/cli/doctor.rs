//! `rover doctor` subcommand.

use std::sync::Arc;

use anyhow::Context;

use crate::config::Config;
use crate::doctor::{CheckCtx, CheckStatus, run_all};
use crate::storage::Db;

pub struct Args {
    pub format: String,
}

pub async fn run(args: Args, config: Config) -> anyhow::Result<i32> {
    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    let ctx = CheckCtx {
        config: Arc::new(config),
        db,
    };
    let (reports, summary) = run_all(&ctx).await;

    match args.format.as_str() {
        "ndjson" => {
            for r in &reports {
                println!(
                    "{}",
                    serde_json::to_string(r).context("serializing report")?
                );
            }
        }
        _ => {
            for r in &reports {
                let marker = match r.status {
                    CheckStatus::Ok => "✓",
                    CheckStatus::Fail => "✗",
                    CheckStatus::Skip => "-",
                };
                let detail = r.detail.as_deref().unwrap_or("");
                println!("{marker} {} {}", r.check, detail);
            }
            match summary {
                CheckStatus::Ok | CheckStatus::Skip => println!("all checks ok"),
                CheckStatus::Fail => println!("one or more checks failed"),
            }
        }
    }
    Ok(if summary == CheckStatus::Fail { 1 } else { 0 })
}

//! `rover batch <id>` is `rover task <id>` with `expect_kind="batch_fetch"`.
//!
//! The wrapper exists for namespace clarity and CLI parity with the
//! task subcommand. `expect_kind` is set by the dispatch in `main.rs`.

use std::path::Path;

pub async fn run(args: crate::cli::task::Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    crate::cli::task::run(args, config_path).await
}

//! `rover meta use <harness>` and the runtime hook handler `rover meta hook <harness>`.

use std::io::Read;

use anyhow::Context;

use crate::meta::{self, Harness, Scope};

#[derive(Debug, clap::Subcommand)]
pub enum MetaCommand {
    /// Wire Rover into an agent harness (MCP config, hooks, rules file).
    Use {
        /// Target harness.
        harness: Harness,
        /// Configuration scope (local, user, or project).
        #[arg(short = 's', long, default_value = "local")]
        scope: Scope,
    },
    /// Runtime hook handler invoked by the harness (reads the hook JSON on stdin).
    Hook {
        /// Harness whose hook contract to speak.
        harness: Harness,
    },
}

pub fn run(cmd: MetaCommand) -> anyhow::Result<i32> {
    match cmd {
        MetaCommand::Use { harness, scope } => {
            let root = std::env::current_dir().context("resolving the current directory")?;
            meta::run_use(harness, scope, &root)
        }
        MetaCommand::Hook { harness } => match harness {
            Harness::Claude => {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .context("reading hook input from stdin")?;
                let out = meta::hook::handle_claude_hook(&buf);
                if !out.is_empty() {
                    println!("{out}");
                }
                Ok(0)
            }
            Harness::General => {
                anyhow::bail!("`general` has no hooks; there is nothing for `meta hook` to do")
            }
        },
    }
}

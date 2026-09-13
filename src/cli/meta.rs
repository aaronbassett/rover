//! `rover meta use <harness>` and the runtime hook handler `rover meta hook <harness>`.

use std::io::Read;
use std::path::Path;

use anyhow::Context;

use crate::meta::{self, Harness, Scope, hook::Capabilities};

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

pub fn run(cmd: MetaCommand, config_path: Option<&Path>) -> anyhow::Result<i32> {
    // Steering is generated from what this install can actually do, so both
    // subcommands need the config. A broken config file is fatal for `use`
    // (it decides what gets written to disk) but must NOT be fatal for
    // `hook`: the hook runs inside every Claude Code session, and a
    // half-edited rover.toml should degrade the steering, not break the
    // user's session. `hook` therefore falls back to defaults and carries on.
    let caps = match cmd {
        MetaCommand::Use { .. } => Capabilities::detect(
            &crate::config::load_resolved(config_path).context("loading config")?,
        ),
        MetaCommand::Hook { .. } => {
            let cfg = crate::config::load_resolved(config_path).unwrap_or_else(|e| {
                tracing::warn!(
                    target: "rover::meta",
                    error = %e,
                    "could not load config for the hook; using defaults for capability detection",
                );
                crate::config::Config::default()
            });
            Capabilities::detect(&cfg)
        }
    };

    match cmd {
        MetaCommand::Use { harness, scope } => {
            let root = std::env::current_dir().context("resolving the current directory")?;
            meta::run_use(harness, scope, &root, caps)
        }
        MetaCommand::Hook { harness } => match harness {
            Harness::Claude => {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .context("reading hook input from stdin")?;
                let out = meta::hook::handle_claude_hook(&buf, caps);
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

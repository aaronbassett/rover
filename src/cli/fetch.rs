//! `rover fetch <url>` command.

use std::path::Path;
use anyhow::Context;
use jiff::Timestamp;
use url::Url;

use crate::config;
use crate::extractor::frontmatter::{PageMeta, render};
use crate::extractor::pipeline::extract;
use crate::fetcher::client::build_http_client;
use crate::fetcher::fetch::fetch_url;
use crate::fetcher::ssrf::SsrfLevel;

pub struct Args {
    pub url: String,

    #[cfg(any(test, feature = "test-loopback"))]
    pub ssrf_test_loopback: bool,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = config::load(config_path).context("loading config")?;
    let url = Url::parse(&args.url).context("parsing URL argument")?;

    let level = ssrf_level_for_args(&args);

    let client = build_http_client(&cfg.fetch.user_agent, cfg.fetch.timeout());
    let page = fetch_url(&client, &url, level).await.context("fetching URL")?;

    if !(200..300).contains(&page.status) {
        anyhow::bail!("HTTP {} from {}", page.status, page.final_url);
    }

    let extracted = extract(&page.body, Some(&page.final_url)).context("extracting article")?;

    let meta = PageMeta {
        url: &url,
        canonical_url: &page.canonical_url,
        title: extracted.title.as_deref(),
        fetched_at: Timestamp::now(),
        body: &extracted.body_md,
    };

    let envelope = render(&meta);
    print!("{envelope}");
    Ok(())
}

#[cfg(any(test, feature = "test-loopback"))]
fn ssrf_level_for_args(args: &Args) -> SsrfLevel {
    if args.ssrf_test_loopback {
        SsrfLevel::TestLoopback
    } else {
        SsrfLevel::Strict
    }
}

#[cfg(not(any(test, feature = "test-loopback")))]
fn ssrf_level_for_args(_args: &Args) -> SsrfLevel {
    SsrfLevel::Strict
}

//! `rover cache <subcommand>` body.

use anyhow::Context;
use jiff::Timestamp;
use std::path::Path;

use crate::config;
use crate::storage::Db;
use crate::storage::pages;

pub enum Args {
    List { limit: u64, offset: u64 },
    Get { url: String },
    Purge { pattern: String, all: bool },
    Stats,
}

pub async fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    let _cfg = config::load_resolved(config_path).context("loading config")?;
    let data_dir = crate::paths::data_dir();
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;
    let db = Db::open(data_dir.join("rover.db"))
        .await
        .context("opening cache database")?;

    match args {
        Args::List { limit, offset } => list(&db, limit, offset).await,
        Args::Get { url } => get(&db, &url).await,
        Args::Purge { pattern, all } => purge(&db, &pattern, all).await,
        Args::Stats => stats(&db).await,
    }
}

async fn list(db: &Db, limit: u64, offset: u64) -> anyhow::Result<()> {
    let entries = pages::list_paginated(db, offset, limit)
        .await
        .context("listing cache")?;
    let now = Timestamp::now().as_second();

    if entries.is_empty() {
        println!("(cache is empty)");
        return Ok(());
    }

    println!(
        "{:<60} {:>10} {:>14} {:>14}",
        "URL", "SIZE", "AGE", "EXPIRES_IN"
    );
    for e in entries {
        let age_s = (now - e.fetched_at).max(0);
        let expires_s = e.expires_at.map(|t| t - now).unwrap_or(0);
        println!(
            "{:<60} {:>10} {:>14} {:>14}",
            truncate(&e.url, 58),
            human_bytes(e.size_bytes as u64),
            human_seconds(age_s),
            if expires_s <= 0 {
                "expired".to_string()
            } else {
                human_seconds(expires_s)
            },
        );
    }
    Ok(())
}

async fn get(db: &Db, url: &str) -> anyhow::Result<()> {
    let hash = pages::url_hash(url);
    if let Some(p) = pages::get_by_url_hash(db, &hash).await? {
        print!("{}", p.extracted_md);
        return Ok(());
    }
    if let Some(p) = pages::get_by_url(db, url).await? {
        print!("{}", p.extracted_md);
        return Ok(());
    }
    anyhow::bail!("not found in cache: {url}");
}

async fn purge(db: &Db, pattern: &str, all: bool) -> anyhow::Result<()> {
    if pattern.is_empty() {
        anyhow::bail!("pattern is empty; refusing to purge");
    }
    if !all && (pattern == "*" || pattern == "**") {
        anyhow::bail!("refusing to purge entire cache without --all flag");
    }
    let like = glob_to_sql_like(pattern);
    let n = pages::delete_by_url_like(db, &like)
        .await
        .context("purging cache")?;
    println!("purged {n} entr{}", if n == 1 { "y" } else { "ies" });
    Ok(())
}

async fn stats(db: &Db) -> anyhow::Result<()> {
    let now = Timestamp::now().as_second();
    let s = pages::stats(db, now).await.context("fetching stats")?;
    println!("entries:       {}", s.entry_count);
    println!("total size:    {}", human_bytes(s.total_extracted_bytes));
    println!("expired:       {}", s.expired_count);
    Ok(())
}

/// Translate a shell-style glob to a SQL LIKE pattern using `\` as the escape.
fn glob_to_sql_like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    for c in pattern.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn human_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

fn human_seconds(s: i64) -> String {
    let s = s.max(0);
    if s >= 86400 {
        format!("{}d", s / 86400)
    } else if s >= 3600 {
        format!("{}h", s / 3600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_translation() {
        assert_eq!(glob_to_sql_like("https://x.com/*"), "https://x.com/%");
        assert_eq!(glob_to_sql_like("page?"), "page_");
        assert_eq!(glob_to_sql_like("100%"), "100\\%");
        assert_eq!(glob_to_sql_like("under_score"), "under\\_score");
        assert_eq!(glob_to_sql_like("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn human_seconds_formats() {
        assert_eq!(human_seconds(45), "45s");
        assert_eq!(human_seconds(120), "2m");
        assert_eq!(human_seconds(7200), "2h");
        assert_eq!(human_seconds(2 * 86400), "2d");
    }
}

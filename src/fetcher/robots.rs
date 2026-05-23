//! Robots.txt fetching, parsing, caching, and evaluation.
//!
//! See M5 design spec §3.7. The cache layer is `storage::robots`; the network
//! layer is a thin retry loop using `Pacer::acquire_global_only` so a robots
//! fetch does not take a per-host slot nor wait on the (unknown) Crawl-Delay
//! floor — that would be a chicken-and-egg cycle.

use std::time::Duration;

use jiff::Timestamp;
use robotxt::Robots;
use url::Url;

use crate::config::RobotsConfig;
use crate::fetcher::FetcherError;
use crate::fetcher::cache_control::CacheControl;
use crate::fetcher::concurrency::Pacer;
use crate::fetcher::fetch::ConditionalGet;
use crate::fetcher::retry;
use crate::fetcher::ssrf::SsrfLevel;
use crate::storage::Db;
use crate::storage::robots::{self as storage_robots, RobotsEntry, RobotsState};

/// Outcome of evaluating a (host, path, ua) against a `RobotsEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Disallowed,
}

/// `Crawl-Delay` value extracted from a parsed entry, if any.
pub fn crawl_delay(entry: &RobotsEntry, user_agent: &str) -> Option<Duration> {
    match entry.state {
        RobotsState::AllowAll | RobotsState::DisallowAll => None,
        RobotsState::Parsed => {
            let body = entry.body.as_deref()?;
            let robots = Robots::from_bytes(body.as_bytes(), user_agent);
            robots.crawl_delay()
        }
    }
}

/// Evaluate whether `path` is allowed for `user_agent` per `entry`.
pub fn evaluate(entry: &RobotsEntry, user_agent: &str, path: &str) -> Verdict {
    match entry.state {
        RobotsState::AllowAll => Verdict::Allowed,
        RobotsState::DisallowAll => Verdict::Disallowed,
        RobotsState::Parsed => {
            let Some(body) = entry.body.as_deref() else {
                return Verdict::Allowed; // defensive: treat missing body as allow-all
            };
            let robots = Robots::from_bytes(body.as_bytes(), user_agent);
            if robots.is_relative_allowed(path) {
                Verdict::Allowed
            } else {
                Verdict::Disallowed
            }
        }
    }
}

/// Look up or fetch+cache a robots.txt entry for `host`.
///
/// If a fresh cached entry exists, return it. Otherwise, fetch
/// `https://{host}/robots.txt`, classify the response per spec §3.7, write
/// the resulting entry to storage, and return it.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_entry(
    db: &Db,
    pacer: &Pacer,
    client: &reqwest::Client,
    cfg: &RobotsConfig,
    host: &str,
    ssrf_level: SsrfLevel,
    user_agent: &str,
    rate_limit_cfg: &crate::config::RateLimitConfig,
) -> Result<RobotsEntry, FetcherError> {
    // `user_agent` is accepted for API symmetry with `evaluate` / future
    // tracing; the actual fetch uses the configured UA on the reqwest client.
    let _ = user_agent;

    let now = Timestamp::now().as_second();
    if let Some(entry) = storage_robots::lookup(db, host).await? {
        if entry.expires_at > now {
            return Ok(entry);
        }
    }

    let robots_url =
        Url::parse(&format!("https://{host}/robots.txt")).map_err(FetcherError::Url)?;
    let result = retry_robots(
        pacer,
        client,
        &robots_url,
        ssrf_level,
        &ConditionalGet::default(),
        rate_limit_cfg,
    )
    .await;

    let entry = build_entry(host, result, cfg, now);
    storage_robots::upsert(db, entry.clone()).await?;
    Ok(entry)
}

/// Retry loop for robots.txt fetches. Mirrors `retry::with_retries` but uses
/// `Pacer::acquire_global_only` so it doesn't take a per-host slot or wait on
/// the (unknown) Crawl-Delay floor.
async fn retry_robots(
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    cfg: &crate::config::RateLimitConfig,
) -> Result<crate::fetcher::fetch::FetchedPage, FetcherError> {
    use crate::fetcher::fetch::fetch_url_conditional;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(crate::fetcher::ssrf::SsrfError::NoHost))?
        .to_string();
    let _guard = pacer.acquire_global_only(&host).await;

    let mut rng: StdRng = match cfg.jitter_seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_os_rng(),
    };

    let mut attempt: u8 = 0;
    loop {
        let result = fetch_url_conditional(client, url, level, None, None, cond).await;
        match result {
            Ok(page) if (200..300).contains(&page.status) || page.status == 304 => return Ok(page),
            Ok(page) => {
                let err = FetcherError::Status {
                    status: page.status,
                    url: page.final_url.to_string(),
                };
                let retryable =
                    matches!(page.status, 429 | 503) || (500..600).contains(&page.status);
                if !retryable {
                    return Err(err);
                }
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(err),
                    });
                }
                let retry_after = page
                    .retry_after
                    .as_deref()
                    .and_then(retry::parse_retry_after);
                let wait = match retry_after {
                    Some(d) => {
                        let (capped, clamped) = retry::compute_retry_after_wait(d, cfg);
                        if clamped {
                            tracing::warn!(
                                target: "rover::fetcher::robots",
                                requested_secs = d.as_secs(),
                                ceiling_secs = cfg.retry_after_ceiling.as_secs(),
                                "robots.txt Retry-After exceeded ceiling; clamping"
                            );
                        }
                        capped
                    }
                    None => retry::compute_jittered_backoff(attempt, cfg, &mut rng),
                };
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => {
                let retryable = match &e {
                    FetcherError::Http(re) => re.is_timeout() || re.is_connect(),
                    _ => false,
                };
                if !retryable {
                    return Err(e);
                }
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(e),
                    });
                }
                let wait = retry::compute_jittered_backoff(attempt, cfg, &mut rng);
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

fn build_entry(
    host: &str,
    result: Result<crate::fetcher::fetch::FetchedPage, FetcherError>,
    cfg: &RobotsConfig,
    now: i64,
) -> RobotsEntry {
    match result {
        Ok(page) => {
            // Parsed response. TTL from Cache-Control or config default.
            let ttl_secs = page
                .cache_control
                .as_deref()
                .and_then(|h| CacheControl::parse(h).max_age)
                .map(|s| s as i64)
                .unwrap_or(cfg.default_ttl.as_secs() as i64);
            RobotsEntry {
                host: host.to_string(),
                body: Some(page.body),
                fetched_at: now,
                expires_at: now + ttl_secs,
                state: RobotsState::Parsed,
            }
        }
        Err(FetcherError::Status { status, .. }) if (400..500).contains(&status) => {
            // 4xx → allow-all, full TTL.
            RobotsEntry {
                host: host.to_string(),
                body: None,
                fetched_at: now,
                expires_at: now + cfg.default_ttl.as_secs() as i64,
                state: RobotsState::AllowAll,
            }
        }
        // 5xx (including RetryExhausted carrying a 5xx) or any transport error.
        // Fail-closed with short TTL.
        Err(_) => RobotsEntry {
            host: host.to_string(),
            body: None,
            fetched_at: now,
            expires_at: now + cfg.failure_ttl.as_secs() as i64,
            state: RobotsState::DisallowAll,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_entry(body: &str) -> RobotsEntry {
        RobotsEntry {
            host: "example.com".into(),
            body: Some(body.into()),
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::Parsed,
        }
    }

    #[test]
    fn evaluate_allow_all_sentinel() {
        let e = RobotsEntry {
            host: "x".into(),
            body: None,
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::AllowAll,
        };
        assert_eq!(evaluate(&e, "Rover/0.1", "/anything"), Verdict::Allowed);
    }

    #[test]
    fn evaluate_disallow_all_sentinel() {
        let e = RobotsEntry {
            host: "x".into(),
            body: None,
            fetched_at: 0,
            expires_at: i64::MAX,
            state: RobotsState::DisallowAll,
        };
        assert_eq!(evaluate(&e, "Rover/0.1", "/anything"), Verdict::Disallowed);
    }

    #[test]
    fn evaluate_disallow_admin() {
        let body = std::fs::read_to_string("tests/fixtures/m5/robots-disallow-admin.txt").unwrap();
        let e = parsed_entry(&body);
        assert_eq!(evaluate(&e, "Rover/0.1", "/articles/x"), Verdict::Allowed);
        assert_eq!(
            evaluate(&e, "Rover/0.1", "/admin/users"),
            Verdict::Disallowed
        );
    }

    #[test]
    fn evaluate_allow_articles_with_disallow_admin() {
        let body = std::fs::read_to_string("tests/fixtures/m5/robots-allow-articles.txt").unwrap();
        let e = parsed_entry(&body);
        assert_eq!(evaluate(&e, "Rover/0.1", "/articles/x"), Verdict::Allowed);
        assert_eq!(
            evaluate(&e, "Rover/0.1", "/admin/users"),
            Verdict::Disallowed
        );
    }

    #[test]
    fn evaluate_ua_specific_rule_wins() {
        let body = std::fs::read_to_string("tests/fixtures/m5/wide-ua-rules.txt").unwrap();
        let e = parsed_entry(&body);
        assert_eq!(evaluate(&e, "Rover", "/admin"), Verdict::Allowed);
        assert_eq!(evaluate(&e, "Other", "/admin"), Verdict::Disallowed);
    }

    #[test]
    fn crawl_delay_extraction() {
        let body = std::fs::read_to_string("tests/fixtures/m5/robots-with-crawldelay.txt").unwrap();
        let e = parsed_entry(&body);
        assert_eq!(crawl_delay(&e, "Rover"), Some(Duration::from_secs(2)));
    }

    #[test]
    fn crawl_delay_none_when_unset() {
        let e = parsed_entry("User-agent: *\nAllow: /\n");
        assert_eq!(crawl_delay(&e, "Rover"), None);
    }

    #[test]
    fn build_entry_4xx_is_allow_all() {
        let err = FetcherError::Status {
            status: 404,
            url: "https://x/robots.txt".into(),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::AllowAll);
        assert!(e.body.is_none());
        assert_eq!(e.expires_at, 100 + (24 * 3600));
    }

    #[test]
    fn build_entry_5xx_is_disallow_all() {
        let err = FetcherError::Status {
            status: 500,
            url: "https://x/robots.txt".into(),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::DisallowAll);
        assert!(e.body.is_none());
        assert_eq!(e.expires_at, 100 + 300);
    }

    #[test]
    fn build_entry_retry_exhausted_is_disallow_all() {
        let inner = FetcherError::Status {
            status: 503,
            url: "https://x/robots.txt".into(),
        };
        let err = FetcherError::RetryExhausted {
            attempts: 4,
            last: Box::new(inner),
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Err(err), &cfg, 100);
        assert_eq!(e.state, RobotsState::DisallowAll);
    }

    #[test]
    fn build_entry_2xx_with_max_age_honors_header() {
        use crate::fetcher::charset::Detected;
        let page = crate::fetcher::fetch::FetchedPage {
            final_url: Url::parse("https://x/robots.txt").unwrap(),
            canonical_url: Url::parse("https://x/robots.txt").unwrap(),
            status: 200,
            content_type: Some("text/plain".into()),
            body: "User-agent: *\nDisallow:\n".into(),
            charset: Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: Some("max-age=3600".into()),
            expires: None,
            retry_after: None,
        };
        let cfg = RobotsConfig::default();
        let e = build_entry("x", Ok(page), &cfg, 100);
        assert_eq!(e.state, RobotsState::Parsed);
        assert_eq!(e.expires_at, 100 + 3600);
        assert!(e.body.is_some());
    }
}

//! Retry wrapper over `fetch_url_conditional`.
//!
//! See M5 design spec §3.5 for the full algorithm. The classifier covers:
//! - 2xx, 304 → Done
//! - 429, 503 with Retry-After → RetryAfter(parsed)
//! - 429, 503 without Retry-After, other 5xx → Backoff
//! - Other 4xx → Fatal
//! - reqwest network errors (is_timeout / is_connect) → Backoff
//! - SSRF / URL / storage errors → Fatal
//! - extractor errors do not flow through retry (they happen after the HTTP
//!   layer has already produced a body).

use std::time::Duration;

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use url::Url;

use crate::config::RateLimitConfig;
use crate::fetcher::FetcherError;
use crate::fetcher::concurrency::{Pacer, PacerGuard};
use crate::fetcher::fetch::{ConditionalGet, FetchedPage, fetch_url_conditional};
use crate::fetcher::ssrf::SsrfLevel;

/// One pass of the classifier.
#[derive(Debug)]
enum Class {
    Done(Box<FetchedPage>),
    Fatal(FetcherError),
    Backoff(FetcherError),
    RetryAfter(Duration, FetcherError),
}

/// Run `fetch_url_conditional` against the retry policy and pacer.
///
/// `crawl_delay` is forwarded to `Pacer::acquire` so the Crawl-Delay floor is
/// applied once at the start; in-loop `Retry-After` sleeps consume the same
/// guard, so we never double-pace.
pub async fn with_retries(
    pacer: &Pacer,
    client: &reqwest::Client,
    url: &Url,
    level: SsrfLevel,
    cond: &ConditionalGet,
    crawl_delay: Option<Duration>,
    cfg: &RateLimitConfig,
) -> Result<FetchedPage, FetcherError> {
    let host = url
        .host_str()
        .ok_or(FetcherError::Ssrf(crate::fetcher::ssrf::SsrfError::NoHost))?
        .to_string();
    let _guard: PacerGuard<'_> = pacer.acquire(&host, crawl_delay).await;

    let mut rng: StdRng = match cfg.jitter_seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_os_rng(),
    };

    let mut attempt: u8 = 0;
    loop {
        let result = fetch_url_conditional(client, url, level, cond).await;
        let class = classify(result, cfg);
        match class {
            Class::Done(page) => return Ok(*page),
            Class::Fatal(err) => return Err(err),
            Class::Backoff(err) => {
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(err),
                    });
                }
                let base = cfg
                    .initial_backoff
                    .saturating_mul(2u32.saturating_pow(attempt as u32));
                let capped = base.min(cfg.max_backoff);
                let jitter_ms = rng.random_range(0..=(capped.as_millis() as u64 / 2));
                let wait = capped + Duration::from_millis(jitter_ms);
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Class::RetryAfter(d, err) => {
                if attempt >= cfg.max_retries {
                    return Err(FetcherError::RetryExhausted {
                        attempts: attempt + 1,
                        last: Box::new(err),
                    });
                }
                let capped = d.min(cfg.retry_after_ceiling);
                if d > cfg.retry_after_ceiling {
                    tracing::warn!(
                        target: "rover::fetcher::retry",
                        requested_secs = d.as_secs(),
                        ceiling_secs = cfg.retry_after_ceiling.as_secs(),
                        "Retry-After exceeded ceiling; clamping"
                    );
                }
                tokio::time::sleep(capped).await;
                attempt += 1;
            }
        }
    }
}

fn classify(result: Result<FetchedPage, FetcherError>, _cfg: &RateLimitConfig) -> Class {
    match result {
        Ok(page) => {
            // 304 is "Done" — cached.rs handles the freshness extension.
            if page.status == 304 || (200..300).contains(&page.status) {
                return Class::Done(Box::new(page));
            }
            classify_non_2xx(page)
        }
        Err(e) => classify_err(e),
    }
}

fn classify_non_2xx(page: FetchedPage) -> Class {
    let status = page.status;
    let retry_after = page.retry_after.as_deref().and_then(parse_retry_after);
    let err = FetcherError::Status {
        status,
        url: page.final_url.to_string(),
    };
    match status {
        429 | 503 => match retry_after {
            Some(d) => Class::RetryAfter(d, err),
            None => Class::Backoff(err),
        },
        500 | 502 | 504 => Class::Backoff(err),
        s if (500..600).contains(&s) => Class::Backoff(err),
        _ => Class::Fatal(err),
    }
}

fn classify_err(e: FetcherError) -> Class {
    match &e {
        FetcherError::Http(re) => {
            if re.is_timeout() || re.is_connect() {
                Class::Backoff(e)
            } else {
                Class::Fatal(e)
            }
        }
        FetcherError::Ssrf(_)
        | FetcherError::Url(_)
        | FetcherError::Decode
        | FetcherError::Storage(_)
        | FetcherError::Status { .. }
        | FetcherError::Dns { .. } => Class::Fatal(e),
        // The retry layer never sees Extract/Robots/Retry variants in practice
        // (they originate above this layer), but classify defensively.
        FetcherError::Extract(_)
        | FetcherError::RetryExhausted { .. }
        | FetcherError::RateLimited { .. }
        | FetcherError::RobotsDisallowed { .. }
        | FetcherError::RobotsFetchFailed { .. } => Class::Fatal(e),
    }
}

/// Parse a `Retry-After` header value. RFC 9110 allows either integer seconds
/// or an HTTP-date.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    if let Ok(t) = httpdate::parse_http_date(trimmed) {
        let now = std::time::SystemTime::now();
        if let Ok(d) = t.duration_since(now) {
            return Some(d);
        }
        // Past date → treat as "ready now".
        return Some(Duration::from_secs(0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RateLimitConfig {
        RateLimitConfig {
            requests_per_minute_per_domain: 6000,
            per_domain_concurrency: 2,
            global_concurrency: 8,
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_secs(1),
            retry_after_ceiling: Duration::from_secs(60),
            jitter_seed: Some(0),
        }
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("0"), Some(Duration::from_secs(0)));
    }

    #[test]
    fn parse_retry_after_http_date_future() {
        // Construct an HTTP-date one hour in the future.
        let t = std::time::SystemTime::now() + Duration::from_secs(3600);
        let s = httpdate::fmt_http_date(t);
        let d = parse_retry_after(&s).unwrap();
        // Some scheduler slack; expect ~3600.
        assert!(d.as_secs() > 3500 && d.as_secs() < 3700, "got {d:?}");
    }

    #[test]
    fn parse_retry_after_http_date_past() {
        let t = std::time::SystemTime::now() - Duration::from_secs(60);
        let s = httpdate::fmt_http_date(t);
        let d = parse_retry_after(&s).unwrap();
        assert_eq!(d, Duration::from_secs(0));
    }

    #[test]
    fn parse_retry_after_garbage_returns_none() {
        assert_eq!(parse_retry_after("not a date or number"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn classify_2xx_is_done() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 200,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        match classify(Ok(page), &cfg()) {
            Class::Done(_) => {}
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn classify_429_with_retry_after_is_retry_after() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 429,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: Some("3".to_string()),
        };
        match classify(Ok(page), &cfg()) {
            Class::RetryAfter(d, _) => assert_eq!(d, Duration::from_secs(3)),
            other => panic!("expected RetryAfter, got {other:?}"),
        }
    }

    #[test]
    fn classify_500_is_backoff() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 500,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        assert!(matches!(classify(Ok(page), &cfg()), Class::Backoff(_)));
    }

    #[test]
    fn classify_404_is_fatal() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com").unwrap(),
            canonical_url: Url::parse("https://example.com").unwrap(),
            status: 404,
            content_type: None,
            body: String::new(),
            charset: crate::fetcher::charset::Detected::default(),
            link_header: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            expires: None,
            retry_after: None,
        };
        assert!(matches!(classify(Ok(page), &cfg()), Class::Fatal(_)));
    }
}

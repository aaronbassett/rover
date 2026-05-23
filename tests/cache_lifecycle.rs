//! End-to-end cache lifecycle test for M2.
//!
//! Exercises the full PRD §14 acceptance criterion: repeated fetches hit
//! cache, force-refresh bypasses, expired entries re-fetch with conditional
//! headers, and `cache purge` removes entries.

use assert_cmd::Command;
use predicates::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ARTICLE_HTML: &str = r#"
<!doctype html>
<html lang="en">
<head><title>Sample article about caching behavior</title></head>
<body>
  <article>
    <h2>How to do the thing</h2>
    <meta http-equiv="Content-Language" content="en" />
    <p>Body paragraph one with enough text to clear readabilityrs's character threshold of 500 characters by default. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.</p>
  </article>
</body>
</html>
"#;

fn rover() -> Command {
    Command::cargo_bin("rover").unwrap()
}

/// Custom matcher: request *has* the given header.
struct HasHeader(&'static str);

impl wiremock::Match for HasHeader {
    fn matches(&self, request: &Request) -> bool {
        request.headers.get(self.0).is_some()
    }
}

/// Custom matcher: request does *not* have the given header.
struct MissingHeader(&'static str);

impl wiremock::Match for MissingHeader {
    fn matches(&self, request: &Request) -> bool {
        request.headers.get(self.0).is_none()
    }
}

#[tokio::test]
async fn cache_hit_then_force_refresh_and_purge() {
    let server = MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(move |_req: &Request| {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .set_body_string(ARTICLE_HTML)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cache-control", "max-age=3600")
        })
        .mount(&server)
        .await;

    let url = format!("{}/article", server.uri());
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("rover.toml");
    std::fs::write(&cfg_path, "[ssrf]\nlevel = \"loopback\"\n").unwrap();

    // First fetch -- miss, hits the network.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--ignore-robots",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "first fetch should hit network"
    );

    // Second fetch -- hit, no network.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--ignore-robots",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "second fetch should hit cache"
    );

    // Force refresh -- bypass cache, hit network again.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--force-refresh",
            "--ignore-robots",
        ])
        .assert()
        .success();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "force-refresh should hit network"
    );

    // Stats: 1 entry.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries:       1"));

    // Purge.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "purge", &format!("{}/*", server.uri())])
        .assert()
        .success()
        .stdout(predicate::str::contains("purged 1 entry"));

    // Stats after purge: 0.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["cache", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries:       0"));
}

#[tokio::test]
async fn stale_entry_serves_immediately_under_swr() {
    // M6 SWR semantics: an expired cache row is served immediately and the
    // CLI does not issue a conditional GET. The (Task 11) revalidate worker
    // will hit the network later out-of-band; the CLI path observes one HTTP
    // request total (the initial 200 that populated the cache).
    let server = MockServer::start().await;
    let etag = "\"abc-123\"";

    // Defensive: if anything sent a conditional GET we'd see this fire and
    // bump the recorded-request count past 1.
    Mock::given(method("GET"))
        .and(path("/news"))
        .and(HasHeader("if-none-match"))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(1)
        .mount(&server)
        .await;

    // First response: 200 with short max-age and an ETag.
    Mock::given(method("GET"))
        .and(path("/news"))
        .and(MissingHeader("if-none-match"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(ARTICLE_HTML)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cache-control", "max-age=1")
                .insert_header("etag", etag),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    let url = format!("{}/news", server.uri());
    let tmp = tempfile::tempdir().unwrap();

    // The default min_ttl floor is 5 minutes, which would dominate the
    // server's max-age=1 and prevent the entry from going stale within the
    // test. Drop the floor (and default_ttl, which must stay >= min_ttl) so
    // the server-supplied max-age=1 wins.
    let cfg_path = tmp.path().join("rover.toml");
    std::fs::write(
        &cfg_path,
        "[cache]\nmin_ttl = \"1s\"\ndefault_ttl = \"1s\"\n\n[ssrf]\nlevel = \"loopback\"\n",
    )
    .unwrap();

    // First fetch -- miss + populate.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--ignore-robots",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));

    // Wait so the entry expires (max-age=1).
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Second fetch -- under SWR this returns the stale row directly and
    // enqueues a revalidate task; no synchronous network request is made.
    rover()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--ignore-robots",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("How to do the thing"));

    // Verify exactly one request reached the server: the initial 200. The
    // second CLI fetch served stale-from-cache without touching the network.
    let received = server
        .received_requests()
        .await
        .expect("request recording is enabled by default");
    assert_eq!(
        received.len(),
        1,
        "expected exactly 1 request (initial); SWR serves stale without network"
    );
    assert!(
        received[0].headers.get("if-none-match").is_none(),
        "first request should not include If-None-Match"
    );
}

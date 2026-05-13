//! End-to-end CLI test for `rover fetch`.

use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetch_prints_markdown_with_frontmatter() {
    let server = MockServer::start().await;
    let body = r#"
<!doctype html>
<html lang="en">
<head><title>Sample</title></head>
<body>
  <article>
    <h1>How to do the thing</h1>
    <p>Body paragraph one with enough text to clear readabilityrs's character threshold of 500 characters by default. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.</p>
  </article>
</body>
</html>
"#;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let url = format!("{}/article", server.uri());
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("rover")
        .unwrap()
        .env("ROVER_DATA_DIR", tmp.path())
        .args(["fetch", &url, "--ssrf-test-loopback"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("---\n"))
        .stdout(predicate::str::contains("url:"))
        .stdout(predicate::str::contains("content_hash: \"sha256:"))
        .stdout(predicate::str::contains("How to do the thing"));
}

#[test]
fn fetch_help_lists_args() {
    Command::cargo_bin("rover")
        .unwrap()
        .args(["fetch", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<URL>"))
        .stdout(predicate::str::contains("URL to fetch"));
}

#[test]
fn unknown_subcommand_errors() {
    Command::cargo_bin("rover")
        .unwrap()
        .args(["nope"])
        .assert()
        .code(2);
}

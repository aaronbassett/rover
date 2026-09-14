//! End-to-end CLI test for `rover fetch`.

mod common;

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
    let cfg_path = tmp.path().join("rover.toml");
    std::fs::write(&cfg_path, "[ssrf]\nlevel = \"loopback\"\n").unwrap();

    Command::cargo_bin("rover")
        .unwrap()
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
        .stdout(predicate::str::starts_with("---\n"))
        .stdout(predicate::str::contains("url:"))
        .stdout(predicate::str::contains("content_hash: \"sha256:"))
        .stdout(predicate::str::contains("How to do the thing"));
}

/// Issue #54 split `frontmatter::render` so the MCP `fetch` tool can take the
/// frontmatter alone. `rover fetch` still prints the combined render: the
/// frontmatter block, one blank line, then the body exactly once.
#[tokio::test]
async fn fetch_prints_the_body_once_after_the_frontmatter() {
    const SENTENCE: &str = "The quick amber lighthouse keeper counted seventeen herons at dawn.";
    let server = MockServer::start().await;
    let body = format!(
        "<html><head><title>Once</title></head><body><article>\
         <h1>Lighthouse</h1>\
         <p>{SENTENCE} The rest of this paragraph gives the extractor enough prose \
         to treat the article as real content rather than boilerplate.</p>\
         <p>A second paragraph, with different words, pads the article out a little \
         further so readability keeps it.</p>\
         </article></body></html>"
    );
    Mock::given(method("GET"))
        .and(path("/once"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let url = format!("{}/once", server.uri());
    let tmp = tempfile::tempdir().unwrap();
    common::seed_default_tokenizer(tmp.path());
    let cfg_path = tmp.path().join("rover.toml");
    std::fs::write(&cfg_path, "[ssrf]\nlevel = \"loopback\"\n").unwrap();

    let out = Command::cargo_bin("rover")
        .unwrap()
        .env("ROVER_DATA_DIR", tmp.path())
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "fetch",
            &url,
            "--ignore-robots",
        ])
        .output()
        .expect("run rover fetch");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();

    assert_eq!(stdout.matches(SENTENCE).count(), 1, "{stdout}");
    let rest = stdout.strip_prefix("---\n").expect("opens with ---");
    let (frontmatter, body) = rest.split_once("\n---\n\n").expect("closing ---");
    assert!(frontmatter.contains("content_hash: \"sha256:"), "{stdout}");
    assert!(!frontmatter.contains(SENTENCE), "{stdout}");
    assert!(body.contains(SENTENCE), "{stdout}");
    assert!(
        body.ends_with('\n') && !body.ends_with("\n\n"),
        "{stdout:?}"
    );
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

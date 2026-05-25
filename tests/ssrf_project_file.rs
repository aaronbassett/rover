//! SSRF Project-level `file://` URL handling.
#![cfg(feature = "test-loopback")]

use rover::fetcher::ssrf::{SsrfError, SsrfLevel, validate_url_with_project_root};
use std::fs;
use std::os::unix::fs::symlink;
use tempfile::tempdir;
use url::Url;

#[test]
fn file_inside_project_root_is_allowed_at_project_level() {
    let tmp = tempdir().unwrap();
    let inside = tmp.path().join("inside.txt");
    fs::write(&inside, "hello").unwrap();
    let url = Url::from_file_path(&inside).unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(r.is_ok(), "file inside root should be allowed: {r:?}");
}

#[test]
fn file_outside_project_root_is_rejected_at_project_level() {
    let tmp = tempdir().unwrap();
    let root_dir = tmp.path().join("root");
    fs::create_dir(&root_dir).unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "leak").unwrap();
    let url = Url::from_file_path(&outside).unwrap();
    let root = fs::canonicalize(&root_dir).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(
        matches!(r, Err(SsrfError::FileOutsideProjectRoot { .. })),
        "expected FileOutsideProjectRoot, got {r:?}",
    );
}

#[test]
fn symlink_pointing_outside_project_root_is_rejected() {
    let tmp = tempdir().unwrap();
    let root_dir = tmp.path().join("root");
    fs::create_dir(&root_dir).unwrap();
    let outside = tmp.path().join("secret.txt");
    fs::write(&outside, "secret").unwrap();
    let link = root_dir.join("link.txt");
    symlink(&outside, &link).unwrap();
    let url = Url::from_file_path(&link).unwrap();
    let root = fs::canonicalize(&root_dir).unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, Some(&root));
    assert!(
        matches!(r, Err(SsrfError::FileOutsideProjectRoot { .. })),
        "expected symlink rejection, got {r:?}",
    );
}

#[test]
fn file_scheme_rejected_at_strict_or_loopback() {
    let url = Url::parse("file:///etc/hosts").unwrap();
    for level in [SsrfLevel::Strict, SsrfLevel::Loopback] {
        let r = validate_url_with_project_root(&url, level, None);
        assert!(
            matches!(r, Err(SsrfError::FileSchemeNotAllowed { .. })),
            "expected file:// rejection at {level:?}, got {r:?}",
        );
    }
}

#[test]
fn missing_project_root_at_project_level_is_an_error() {
    let url = Url::parse("file:///tmp/x").unwrap();
    let r = validate_url_with_project_root(&url, SsrfLevel::Project, None);
    assert!(
        matches!(r, Err(SsrfError::ProjectRootMissing)),
        "expected ProjectRootMissing, got {r:?}",
    );
}

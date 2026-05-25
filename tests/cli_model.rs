//! Integration test for `rover model`. We avoid the real HuggingFace API
//! by setting HF_HOME to a temp dir and asserting against `list` + `remove`
//! behavior on a fake cache layout we materialize manually.
//!
//! `download` is exercised in the smoketest workflow against a tiny real
//! repo (`HuggingFaceTB/SmolLM2-135M-Instruct`, ~270 MB) — see Task 53.

#![cfg(any(feature = "local-inference", feature = "local-vision"))]

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

fn rover_bin() -> Command {
    Command::cargo_bin("rover").unwrap()
}

#[test]
fn list_empty_when_cache_missing() {
    let tmp = tempfile::tempdir().unwrap();
    rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("(no models cached at"));
}

#[test]
fn list_shows_fake_cached_model() {
    let tmp = tempfile::tempdir().unwrap();
    let hub = tmp.path().join("hub").join("models--FakeOwner--FakeModel");
    fs::create_dir_all(&hub).unwrap();
    fs::write(hub.join("config.json"), "{}").unwrap();
    fs::write(hub.join("model.safetensors"), &[0u8; 1_500_000][..]).unwrap();

    rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("FakeOwner/FakeModel"))
        .stderr(predicate::str::contains("MB").or(predicate::str::contains("KB")));
}

#[test]
fn remove_deletes_cached_model_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let hub = tmp.path().join("hub").join("models--FakeOwner--FakeModel");
    fs::create_dir_all(&hub).unwrap();
    fs::write(hub.join("config.json"), "{}").unwrap();

    rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "remove", "FakeOwner/FakeModel"])
        .assert()
        .success();
    assert!(!hub.exists());
}

#[test]
fn remove_idempotent_on_missing_repo() {
    let tmp = tempfile::tempdir().unwrap();
    rover_bin()
        .env("HF_HOME", tmp.path())
        .args(["model", "remove", "FakeOwner/Missing"])
        .assert()
        .success()
        .stderr(predicate::str::contains("nothing to remove"));
}

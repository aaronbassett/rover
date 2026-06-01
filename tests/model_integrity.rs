//! Integration coverage for the model-integrity gate (red-team finding #6).
//!
//! Exercises the public, `HF_HOME`-resolving API that the local model loaders
//! call (`enforce`) — the unit tests in the module deliberately drive the
//! lower-level `*_at` helpers, so this is where the env-resolution and the
//! `ROVER_UNSAFE_DISABLE_MODEL_INTEGRITY_CHECK` bypass are verified end to end.
//! Each integration test runs in its own process, so mutating `HF_HOME` /
//! the disable env var here is isolated.

#![cfg(feature = "local-inference")]

use std::sync::Mutex;

use rover::model_integrity::{self, IntegrityError};

/// Tests in this file mutate the process-global `HF_HOME` (and the disable env
/// var). Integration tests in one file share a process and run in parallel, so
/// serialise them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Lay down a minimal HF-style cache under `$HF_HOME/hub` and return the
/// snapshot dir so the test can tamper with files.
fn seed_cache(hf_home: &std::path::Path, repo: &str, rev: &str) -> std::path::PathBuf {
    let model = hf_home
        .join("hub")
        .join(format!("models--{}", repo.replace('/', "--")));
    std::fs::create_dir_all(model.join("refs")).unwrap();
    std::fs::write(model.join("refs").join("main"), rev).unwrap();
    let snap = model.join("snapshots").join(rev);
    std::fs::create_dir_all(&snap).unwrap();
    std::fs::write(snap.join("config.json"), b"{}").unwrap();
    std::fs::write(snap.join("model.safetensors"), b"original-weights").unwrap();
    snap
}

#[test]
fn tampering_makes_the_next_load_fail_with_typed_error() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serialised by ENV_LOCK within this binary.
    unsafe { std::env::set_var("HF_HOME", tmp.path()) };

    let repo = "Acme/tiny";
    let snap = seed_cache(tmp.path(), repo, "rev1");

    // Fresh download would record the manifest; simulate that.
    model_integrity::bootstrap(repo).unwrap();
    // An intact cache loads fine.
    model_integrity::enforce(repo).unwrap();

    // Tamper with the weights as an attacker with cache write access would.
    std::fs::write(snap.join("model.safetensors"), b"backdoored").unwrap();

    let err =
        model_integrity::enforce(repo).expect_err("a tampered weight file must abort the load");
    match err {
        IntegrityError::ModelIntegrityFailure { file, .. } => {
            assert_eq!(file, "model.safetensors");
        }
        other => panic!("expected ModelIntegrityFailure, got {other:?}"),
    }
}

#[test]
fn unsafe_disable_env_bypasses_verification() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serialised by ENV_LOCK within this binary.
    unsafe { std::env::set_var("HF_HOME", tmp.path()) };

    let repo = "Acme/tiny";
    let snap = seed_cache(tmp.path(), repo, "rev1");
    model_integrity::bootstrap(repo).unwrap();

    // Tamper, then flip the unsafe bypass — the load must succeed despite the
    // mismatch.
    std::fs::write(snap.join("model.safetensors"), b"backdoored").unwrap();
    unsafe { std::env::set_var(model_integrity::DISABLE_ENV, "1") };
    assert!(
        model_integrity::enforce(repo).is_ok(),
        "the unsafe bypass must skip verification"
    );

    // And without the bypass it fails again.
    unsafe { std::env::remove_var(model_integrity::DISABLE_ENV) };
    assert!(model_integrity::enforce(repo).is_err());
}

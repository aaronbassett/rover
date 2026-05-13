//! Integration test for tokenizer download + load. Hits HuggingFace, so
//! it's `#[ignore]` by default. Run with: `cargo test --test
//! tokenizer_integration -- --ignored --nocapture`.

use rover::tokenizer::{self, Tokenizer};

#[tokio::test]
#[ignore = "hits HuggingFace network"]
async fn ensure_loaded_then_count_works_for_all_families() {
    let tmp = tempfile::tempdir().unwrap();
    // Direct the XDG root at a tempdir so the test doesn't pollute the user.
    // SAFETY: `set_var` in Rust 2024 is unsafe; this test is single-threaded
    // (#[tokio::test] flavor=current_thread) so no other tests are racing.
    unsafe { std::env::set_var("ROVER_DATA_DIR", tmp.path()) };

    for family in Tokenizer::ALL {
        tokenizer::ensure_loaded(family)
            .await
            .unwrap_or_else(|e| panic!("ensure_loaded({family}) failed: {e}"));
        let n = tokenizer::count("hello world", family).expect("count");
        assert!(n > 0, "expected non-zero tokens for {family}, got {n}");
    }
}

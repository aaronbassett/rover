//! Smoketest for the local-inference feature. Loads a real (small) model
//! from HuggingFace and runs one summarization call. `#[ignore]` by
//! default — opt in via `cargo test --features local-inference -- --ignored`.
//!
//! CI: the smoketest workflow runs these nightly. Local devs can run
//! them on demand. The test caches models in HF_HOME, so subsequent runs
//! are fast (the first run downloads ~1.6 GB).

#![cfg(feature = "local-inference")]

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::local::LocalMistralRs;
use rover::tokenizer::Tokenizer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn loads_qwen_and_summarizes_short_input() {
    let model_id = std::env::var("ROVER_CI_TEST_MODEL")
        .unwrap_or_else(|_| "Qwen/Qwen3.5-0.8B".to_string());
    let be = LocalMistralRs::new("test", &model_id, Tokenizer::O200k);
    let opts = CompactOpts {
        mode: CompactMode::Abstractive,
        style: Style::Prose,
        target_tokens: Some(60),
        focus: None,
        preserve: vec![],
        backend_name: "test".to_string(),
    };
    let content = "Rover is a polite scraper that fetches web pages and turns \
                   them into clean Markdown for LLM agents. It caches what it \
                   fetches and summarizes long pages on demand.";
    let summary = be.compact(content, &opts).await.expect("compact ok");
    assert!(!summary.is_empty(), "summary must be non-empty");
    assert!(summary.len() < content.len() * 2, "summary should not balloon");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bogus_repo_id_yields_unavailable_error() {
    use rover::summarizer::error::BackendError;
    let be = LocalMistralRs::new("test", "Nonsense/DoesNotExist-XX", Tokenizer::O200k);
    let opts = CompactOpts {
        mode: CompactMode::Abstractive, style: Style::Prose,
        target_tokens: None, focus: None, preserve: vec![],
        backend_name: "test".to_string(),
    };
    let r = be.compact("anything", &opts).await;
    assert!(matches!(r, Err(BackendError::Unavailable(_))), "got {r:?}");
}

//! Wall-clock test for per-table parallelization.
#![cfg(feature = "test-loopback")]

use rover::extractor::options::TablesMode;
use rover::extractor::output::OutputPaths;
use rover::extractor::tables::{FallbackInfo, TableSummarizeHook, apply_with_summarizer};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

#[tokio::test]
async fn eight_tables_run_in_parallel() {
    let md = (0..8)
        .map(|i| format!("| A | B |\n|---|---|\n| {i} | x |\n"))
        .collect::<Vec<_>>()
        .join("\n\n");

    let hook: TableSummarizeHook = Arc::new(|_text: &str| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<(String, Option<FallbackInfo>), String>(("(summary)".to_string(), None))
        })
    });

    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: race-prone if other tests parallel-set ROVER_OUTPUT_DIR; mitigated
    // by using a unique tempdir.
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp.path()) };
    let paths = OutputPaths::resolve(None).unwrap();
    let url = Url::parse("https://example.com/").unwrap();

    let start = Instant::now();
    let (_out, recs) =
        apply_with_summarizer(&md, &TablesMode::Summarize, &paths, &url, Some(&hook))
            .await
            .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(recs.len(), 8);
    assert!(
        elapsed < Duration::from_millis(400),
        "8 tables × 100ms each should complete < 400ms with concurrency=4, took {elapsed:?}",
    );
}

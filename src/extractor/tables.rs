//! Table transformation modes.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extractor::options::{SampleStrategy, TablesMode};
use crate::extractor::output::OutputPaths;
use crate::extractor::pipeline::ExtractorError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTransform {
    pub ordinal: usize,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_rows: Option<usize>,
    /// Per-table summary body emitted by `TablesMode::Summarize`. Absent
    /// for every other mode and for tables where the summarizer hook
    /// returned an error (see `fallback_reason`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_md: Option<String>,
    /// Reason the per-table summarizer hook failed for this table; the
    /// original markdown is preserved verbatim in the body when this is
    /// set. Mirrors `SummarizerError::fallback_reason()` strings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

/// Async hook the `TablesMode::Summarize` path invokes for each detected
/// table. Receives the plaintext-rendered table block and returns either
/// the summary string or an error reason (the string is recorded in
/// `TableTransform.fallback_reason`).
pub type TableSummarizeHook = Arc<
    dyn for<'a> Fn(&'a str) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>
        + Send
        + Sync,
>;

/// Returns the transformed markdown plus per-table records.
pub fn apply(
    markdown: &str,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
) -> Result<(String, Vec<TableTransform>), ExtractorError> {
    let mut out = String::with_capacity(markdown.len());
    let mut records = Vec::new();
    let mut ordinal: usize = 0;
    let mut iter = markdown.lines().peekable();

    while let Some(line) = iter.next() {
        if is_pipe_table_start(line, iter.peek().copied()) {
            // Collect all consecutive table lines.
            let mut rows: Vec<String> = vec![line.to_string()];
            while let Some(next) = iter.peek().copied() {
                if next.trim_start().starts_with('|') {
                    rows.push(next.to_string());
                    iter.next();
                } else {
                    break;
                }
            }
            let (replacement, record) =
                transform_table(rows, ordinal, mode, output_paths, base_url)?;
            out.push_str(&replacement);
            out.push('\n');
            if let Some(r) = record {
                records.push(r);
            }
            ordinal += 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok((out, records))
}

/// Async-aware variant of [`apply`] that routes `TablesMode::Summarize`
/// through `hook`. All other modes delegate to the sync `apply` path.
///
/// For each detected pipe-table the hook is invoked with the table's
/// raw markdown block; on `Ok(summary)` the block is replaced with the
/// summary text and `TableTransform.summary_md` is populated. On
/// `Err(reason)` the original table is preserved verbatim and
/// `fallback_reason` records `reason`.
pub async fn apply_with_summarizer(
    markdown: &str,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
    hook: Option<&TableSummarizeHook>,
) -> Result<(String, Vec<TableTransform>), ExtractorError> {
    if !matches!(mode, TablesMode::Summarize) {
        return apply(markdown, mode, output_paths, base_url);
    }
    let Some(hook) = hook else {
        return Err(ExtractorError::Metadata(
            "internal: apply_with_summarizer requires a hook for TablesMode::Summarize".into(),
        ));
    };

    let mut out = String::with_capacity(markdown.len());
    let mut records = Vec::new();
    let mut ordinal: usize = 0;
    let mut iter = markdown.lines().peekable();

    while let Some(line) = iter.next() {
        if is_pipe_table_start(line, iter.peek().copied()) {
            let mut rows: Vec<String> = vec![line.to_string()];
            while let Some(next) = iter.peek().copied() {
                if next.trim_start().starts_with('|') {
                    rows.push(next.to_string());
                    iter.next();
                } else {
                    break;
                }
            }
            let table_text = rows.join("\n");
            match hook(&table_text).await {
                Ok(summary) => {
                    out.push_str(&summary);
                    out.push('\n');
                    records.push(TableTransform {
                        ordinal,
                        mode: "summarize".into(),
                        path: None,
                        kept_rows: None,
                        truncated_rows: None,
                        summary_md: Some(summary),
                        fallback_reason: None,
                    });
                }
                Err(reason) => {
                    out.push_str(&table_text);
                    out.push('\n');
                    records.push(TableTransform {
                        ordinal,
                        mode: "summarize".into(),
                        path: None,
                        kept_rows: None,
                        truncated_rows: None,
                        summary_md: None,
                        fallback_reason: Some(reason),
                    });
                }
            }
            ordinal += 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok((out, records))
}

fn is_pipe_table_start(line: &str, next: Option<&str>) -> bool {
    if !line.trim_start().starts_with('|') {
        return false;
    }
    let Some(n) = next else {
        return false;
    };
    // Markdown pipe-table separator: |---|---|... or |:---:|...
    let nt = n.trim_start();
    nt.starts_with('|') && nt.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn transform_table(
    rows: Vec<String>,
    ordinal: usize,
    mode: &TablesMode,
    output_paths: &OutputPaths,
    base_url: &Url,
) -> Result<(String, Option<TableTransform>), ExtractorError> {
    match mode {
        TablesMode::Embed => Ok((
            rows.join("\n"),
            Some(TableTransform {
                ordinal,
                mode: "embed".into(),
                path: None,
                kept_rows: None,
                truncated_rows: None,
                summary_md: None,
                fallback_reason: None,
            }),
        )),
        TablesMode::Drop => Ok((
            format!("_Table {ordinal} omitted_"),
            Some(TableTransform {
                ordinal,
                mode: "drop".into(),
                path: None,
                kept_rows: None,
                truncated_rows: None,
                summary_md: None,
                fallback_reason: None,
            }),
        )),
        TablesMode::Sample(strategy) => {
            // rows[0] = header, rows[1] = separator, rows[2..] = data
            if rows.len() < 3 {
                return Ok((rows.join("\n"), None));
            }
            let header = &rows[0];
            let sep = &rows[1];
            let data: Vec<&String> = rows[2..].iter().collect();
            let (kept, truncated) = sample_rows(&data, strategy);
            let mut out = vec![header.clone(), sep.clone()];
            for r in &kept {
                out.push((*r).clone());
            }
            if truncated > 0 {
                out.push(format!("_… {truncated} rows truncated …_"));
            }
            Ok((
                out.join("\n"),
                Some(TableTransform {
                    ordinal,
                    mode: "sample".into(),
                    path: None,
                    kept_rows: Some(kept.len()),
                    truncated_rows: Some(truncated),
                    summary_md: None,
                    fallback_reason: None,
                }),
            ))
        }
        TablesMode::CsvFile => {
            let path = output_paths.table_path(base_url, ordinal);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| ExtractorError::TableWrite {
                    ordinal,
                    path: parent.display().to_string(),
                    source,
                })?;
            }
            write_csv(&path, &rows, ordinal)?;
            let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
            Ok((
                format!("_Table {ordinal} saved to {}_", abs.display()),
                Some(TableTransform {
                    ordinal,
                    mode: "csv_file".into(),
                    path: Some(abs),
                    kept_rows: None,
                    truncated_rows: None,
                    summary_md: None,
                    fallback_reason: None,
                }),
            ))
        }
        TablesMode::Summarize => Err(ExtractorError::Metadata(
            "internal: TablesMode::Summarize must go through apply_with_summarizer".into(),
        )),
    }
}

fn sample_rows<'a>(data: &[&'a String], strategy: &SampleStrategy) -> (Vec<&'a String>, usize) {
    let total = data.len();
    match strategy {
        SampleStrategy::HeadTail { head, tail } => {
            if total <= head + tail {
                return (data.to_vec(), 0);
            }
            let mut kept: Vec<&String> = data.iter().take(*head).copied().collect();
            kept.extend(data.iter().rev().take(*tail).rev().copied());
            let truncated = total - kept.len();
            (kept, truncated)
        }
        SampleStrategy::RandomSeed { rows, seed } => {
            if total <= *rows {
                return (data.to_vec(), 0);
            }
            let mut rng = ChaCha8Rng::seed_from_u64(*seed);
            let mut indices: Vec<usize> = (0..total).collect();
            indices.shuffle(&mut rng);
            indices.truncate(*rows);
            indices.sort();
            let kept: Vec<&String> = indices.iter().map(|i| data[*i]).collect();
            let truncated = total - kept.len();
            (kept, truncated)
        }
    }
}

fn parse_pipe_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.trim_start_matches('|').trim_end_matches('|');
    line.split('|').map(|c| c.trim().to_string()).collect()
}

fn write_csv(
    path: &std::path::Path,
    rows: &[String],
    ordinal: usize,
) -> Result<(), ExtractorError> {
    let file = std::fs::File::create(path).map_err(|source| ExtractorError::TableWrite {
        ordinal,
        path: path.display().to_string(),
        source,
    })?;
    let mut wtr = csv::Writer::from_writer(file);
    for (i, row) in rows.iter().enumerate() {
        if i == 1 {
            continue; // skip separator
        }
        let cells = parse_pipe_row(row);
        wtr.write_record(&cells)
            .map_err(|e| ExtractorError::TableWrite {
                ordinal,
                path: path.display().to_string(),
                source: std::io::Error::other(e.to_string()),
            })?;
    }
    wtr.flush().map_err(|source| ExtractorError::TableWrite {
        ordinal,
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn paths() -> OutputPaths {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        // SAFETY: serialized by TEST_MUTEX in each test
        unsafe { std::env::set_var("ROVER_OUTPUT_DIR", &dir) };
        OutputPaths::resolve(None).unwrap()
    }

    fn url() -> Url {
        Url::parse("https://example.com/").unwrap()
    }

    const TABLE_3ROWS: &str = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";

    #[test]
    fn embed_mode_passes_through() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::Embed, &paths(), &url()).unwrap();
        assert!(out.contains("| 1 | 2 |"));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].mode, "embed");
    }

    #[test]
    fn drop_mode_replaces_with_marker() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::Drop, &paths(), &url()).unwrap();
        assert!(out.contains("_Table 0 omitted_"));
        assert!(!out.contains("| 1 | 2 |"));
        assert_eq!(recs[0].mode, "drop");
    }

    #[test]
    fn sample_head_tail_keeps_head_plus_tail() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let strategy = SampleStrategy::HeadTail { head: 1, tail: 1 };
        let (out, recs) =
            apply(TABLE_3ROWS, &TablesMode::Sample(strategy), &paths(), &url()).unwrap();
        assert!(out.contains("| 1 | 2 |"));
        assert!(out.contains("| 5 | 6 |"));
        assert!(out.contains("_… 1 rows truncated …_"));
        assert_eq!(recs[0].kept_rows, Some(2));
        assert_eq!(recs[0].truncated_rows, Some(1));
    }

    #[test]
    fn sample_random_seed_is_deterministic() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let strat = SampleStrategy::RandomSeed { rows: 2, seed: 42 };
        let (out_a, _) = apply(
            TABLE_3ROWS,
            &TablesMode::Sample(strat.clone()),
            &paths(),
            &url(),
        )
        .unwrap();
        let (out_b, _) = apply(TABLE_3ROWS, &TablesMode::Sample(strat), &paths(), &url()).unwrap();
        assert_eq!(out_a, out_b);
    }

    #[test]
    fn csv_file_writes_table_to_disk_and_replaces_markdown() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (out, recs) = apply(TABLE_3ROWS, &TablesMode::CsvFile, &paths(), &url()).unwrap();
        assert!(out.contains("_Table 0 saved to "));
        let p = recs[0].path.as_ref().unwrap();
        let csv = std::fs::read_to_string(p).unwrap();
        assert!(csv.contains("A,B"));
        assert!(csv.contains("1,2"));
        assert!(csv.contains("5,6"));
    }

    #[tokio::test]
    async fn summarize_mode_invokes_hook_per_table() {
        // `paths()` sets `ROVER_OUTPUT_DIR`; serialize against other tests
        // in this module. Drop the guard before awaiting.
        let paths = {
            let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            paths()
        };
        let md = "Intro.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nMiddle.\n\n| X | Y |\n|---|---|\n| 9 | 8 |\n";
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let hook: TableSummarizeHook = std::sync::Arc::new(move |_text: &str| {
            let counter_clone = counter_clone.clone();
            Box::pin(async move {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<String, String>("(summary)".to_string())
            })
        });
        let (out, recs) =
            apply_with_summarizer(md, &TablesMode::Summarize, &paths, &url(), Some(&hook))
                .await
                .unwrap();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(recs.len(), 2);
        assert!(recs.iter().all(|r| r.mode == "summarize"));
        assert!(
            recs.iter()
                .all(|r| r.summary_md.as_deref() == Some("(summary)"))
        );
        assert!(recs.iter().all(|r| r.fallback_reason.is_none()));
        assert!(out.contains("(summary)"));
        assert!(!out.contains("| 1 | 2 |"));
        assert!(!out.contains("| 9 | 8 |"));
    }

    #[tokio::test]
    async fn summarize_mode_records_fallback_when_hook_fails() {
        let paths = {
            let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            paths()
        };
        let hook: TableSummarizeHook = std::sync::Arc::new(|_text: &str| {
            Box::pin(async move { Err::<String, String>("auth_failed".to_string()) })
        });
        let (out, recs) = apply_with_summarizer(
            TABLE_3ROWS,
            &TablesMode::Summarize,
            &paths,
            &url(),
            Some(&hook),
        )
        .await
        .unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].mode, "summarize");
        assert!(recs[0].summary_md.is_none());
        assert_eq!(recs[0].fallback_reason.as_deref(), Some("auth_failed"));
        assert!(out.contains("| 1 | 2 |"));
        assert!(out.contains("| 5 | 6 |"));
    }

    #[test]
    fn non_table_content_passes_through_unchanged() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let md = "Just some text\n\nNo tables here.\n";
        let (out, recs) = apply(md, &TablesMode::Drop, &paths(), &url()).unwrap();
        assert_eq!(out, md);
        assert!(recs.is_empty());
    }
}

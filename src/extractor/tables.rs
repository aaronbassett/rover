//! Table transformation modes.

use std::path::PathBuf;

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
}

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
                }),
            ))
        }
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

    #[test]
    fn non_table_content_passes_through_unchanged() {
        let _g = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let md = "Just some text\n\nNo tables here.\n";
        let (out, recs) = apply(md, &TablesMode::Drop, &paths(), &url()).unwrap();
        assert_eq!(out, md);
        assert!(recs.is_empty());
    }
}

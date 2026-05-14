mod common;

use rover::extractor::options::TablesMode;
use rover::extractor::output::OutputPaths;
use rover::extractor::tables;
use url::Url;

fn fixture_paths(tmp: &std::path::Path) -> OutputPaths {
    // SAFETY: each integration test is its own process; env var setting is safe.
    unsafe { std::env::set_var("ROVER_OUTPUT_DIR", tmp) };
    OutputPaths::resolve(None).unwrap()
}

#[test]
fn csv_file_mode_writes_csv_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(tmp.path());
    let url = Url::parse("https://example.com/tables").unwrap();
    let md = "Lead-in.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nMore text.\n";
    let (out, recs) = tables::apply(md, &TablesMode::CsvFile, &paths, &url).unwrap();
    assert!(out.contains("_Table 0 saved to "));
    assert_eq!(recs.len(), 1);
    let p = recs[0].path.as_ref().unwrap();
    let csv = std::fs::read_to_string(p).unwrap();
    assert!(csv.contains("A,B"));
    assert!(csv.contains("1,2"));
    assert!(csv.contains("3,4"));
}

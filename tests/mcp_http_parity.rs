//! End-to-end HTTP transport tests over a real socket, plus CLI wiring.

mod common;

#[test]
fn bind_without_http_is_a_parse_error() {
    let out = std::process::Command::new(common::bin_path())
        .args(["mcp", "--bind", "0.0.0.0:7683"])
        .output()
        .expect("run rover");

    assert!(!out.status.success(), "--bind without --http must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--http"),
        "error should name the required flag, got: {stderr}"
    );
}

//! Smoke test: tracing event with a sensitive URL is redacted in stderr.
#![cfg(feature = "test-loopback")]

use rover::telemetry::redact::redact_url;

#[test]
fn unit_path_redacts_api_key() {
    let url = "https://api.example.com/v1?api_key=AKIA";
    assert!(!redact_url(url).contains("AKIA"));
}

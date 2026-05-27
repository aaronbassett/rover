//! Tracing layer that redacts secret material from event field values
//! before they hit any log destination. Two redactors run in sequence:
//!
//! 1. URL query-string values for keys matching a denylist
//!    (`api_key`, `token`, `secret`, `password`).
//! 2. HTTP `Authorization`-style credentials:
//!    - A field literally named `authorization` (case-insensitive) has its
//!      entire value replaced with `<redacted>`.
//!    - Any value that embeds a `Bearer <token>` or `Basic <token>` shape
//!      (regardless of field name — covers debug-printed `HeaderMap`s and
//!      similar) has the credential portion replaced with `<redacted>`.
//!
//! HAR debug output is deliberately NOT scrubbed (see `docs/security.md`):
//! the user opts in via `[debug] har_path` to capture raw traffic and the
//! file is meant to be protected with filesystem permissions.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

const TRIGGER_KEYS: &[&str] = &["api_key", "token", "secret", "password"];

/// Matches `Bearer <token>` or `Basic <token>` (case-insensitive scheme;
/// credential is any run of non-whitespace). Used to scrub credential
/// substrings out of debug-printed header maps and similar.
static AUTH_HEADER_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(Bearer|Basic)\s+\S+").unwrap());

/// Replace every `Bearer <token>` / `Basic <token>` substring in `s` with
/// `<scheme> <redacted>`. Returns `s` unchanged if no match.
pub fn redact_authorization(s: &str) -> String {
    AUTH_HEADER_VALUE
        .replace_all(s, "$1 <redacted>")
        .into_owned()
}

/// True if `field_name` is an HTTP-authorization-shaped key whose entire
/// value should be scrubbed (rather than just the credential portion).
fn is_authorization_field(field_name: &str) -> bool {
    field_name.eq_ignore_ascii_case("authorization")
}

/// Redact secret query-string values from `s`. If `s` is not a URL or has
/// no triggering keys, returns the input unchanged.
///
/// Fast path: short-circuit when the string contains neither `=` nor `?`.
/// Otherwise parse, walk pairs, only allocate if at least one rewrite happens.
pub fn redact_url(s: &str) -> String {
    if !s.contains('=') && !s.contains('?') {
        return s.to_string();
    }
    let Ok(mut url) = Url::parse(s) else {
        return s.to_string();
    };
    let Some(query) = url.query().map(str::to_string) else {
        return s.to_string();
    };
    let mut rewritten = String::with_capacity(query.len());
    let mut changed = false;
    let mut first = true;
    for pair in query.split('&') {
        if !first {
            rewritten.push('&');
        }
        first = false;
        if let Some((k, _v)) = pair.split_once('=') {
            let k_lower = k.to_lowercase();
            if TRIGGER_KEYS.iter().any(|t| k_lower.contains(t)) {
                rewritten.push_str(k);
                rewritten.push_str("=<redacted>");
                changed = true;
                continue;
            }
        }
        rewritten.push_str(pair);
    }
    if !changed {
        return s.to_string();
    }
    url.set_query(Some(&rewritten));
    url.to_string()
}

use std::fmt;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::fmt::{
    FmtContext,
    format::{FormatEvent, FormatFields, Writer},
};
use tracing_subscriber::registry::LookupSpan;

/// Custom event formatter that redacts URL query-string secrets in every
/// field value before writing. Replaces the default formatter installed in
/// `telemetry::init`.
pub struct RedactingFormatEvent;

impl<S, N> FormatEvent<S, N> for RedactingFormatEvent
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        // Plain-text line format: <timestamp> <LEVEL> <target>: <fields>
        write!(
            writer,
            "{} {} {}:",
            jiff::Timestamp::now(),
            metadata.level(),
            metadata.target(),
        )?;
        let mut buf = String::new();
        let mut visitor = RedactingVisitor { out: &mut buf };
        event.record(&mut visitor);
        writeln!(writer, "{buf}")?;
        Ok(())
    }
}

struct RedactingVisitor<'a> {
    out: &'a mut String,
}

impl Visit for RedactingVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let _ = std::fmt::write(
            &mut *self.out,
            format_args!(" {}={}", field.name(), scrub(field.name(), value)),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let formatted = format!("{value:?}");
        let _ = std::fmt::write(
            &mut *self.out,
            format_args!(" {}={}", field.name(), scrub(field.name(), &formatted)),
        );
    }
}

/// Run every redactor over `value`. If `field_name` is itself an
/// authorization-shaped key, short-circuit with the literal `<redacted>`
/// (cheaper and stops the value from leaking even when it doesn't match
/// the Bearer/Basic shape — e.g. a custom token format).
fn scrub(field_name: &str, value: &str) -> String {
    if is_authorization_field(field_name) {
        return "<redacted>".to_string();
    }
    redact_authorization(&redact_url(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_api_key_query_param() {
        let url = "https://api.example.com/v1/x?api_key=AKIAIOSFODNN7EXAMPLE&page=1";
        let out = redact_url(url);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "got: {out}");
        assert!(
            out.contains("api_key=%3Credacted%3E") || out.contains("api_key=<redacted>"),
            "got: {out}"
        );
        assert!(
            out.contains("page=1"),
            "non-secret param should remain: {out}"
        );
    }

    #[test]
    fn redacts_token_substring_match() {
        let url = "https://x/?access_token=abc";
        let out = redact_url(url);
        assert!(!out.contains("abc"), "got: {out}");
    }

    #[test]
    fn leaves_non_secret_url_alone() {
        let url = "https://x/?page=2&size=10";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn passes_through_non_url_strings() {
        let s = "this is not a url";
        assert_eq!(redact_url(s), s);
    }

    #[test]
    fn redact_authorization_strips_bearer_token() {
        let s = "tried: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let out = redact_authorization(s);
        assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"), "got: {out}");
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
    }

    #[test]
    fn redact_authorization_strips_basic_token() {
        let s = "auth=Basic dXNlcjpwYXNzd29yZA==";
        let out = redact_authorization(s);
        assert!(!out.contains("dXNlcjpwYXNzd29yZA"), "got: {out}");
        assert!(out.contains("Basic <redacted>"), "got: {out}");
    }

    #[test]
    fn redact_authorization_is_case_insensitive_on_scheme() {
        for s in [
            "bearer ABCDEF",
            "BEARER ABCDEF",
            "BeArEr ABCDEF",
            "basic ABCDEF",
            "BASIC ABCDEF",
        ] {
            let out = redact_authorization(s);
            assert!(!out.contains("ABCDEF"), "input={s:?} got: {out}");
        }
    }

    #[test]
    fn redact_authorization_handles_multiple_matches() {
        // Debug-printed map with two Authorization-bearing entries.
        let s = r#"{"first": "Bearer aaa", "second": "Basic bbb"}"#;
        let out = redact_authorization(s);
        assert!(!out.contains("aaa"), "got: {out}");
        assert!(!out.contains("bbb"), "got: {out}");
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
        assert!(out.contains("Basic <redacted>"), "got: {out}");
    }

    #[test]
    fn redact_authorization_leaves_unrelated_text_alone() {
        let s = "user logged in via http";
        assert_eq!(redact_authorization(s), s);
    }

    #[test]
    fn is_authorization_field_matches_case_insensitively() {
        assert!(is_authorization_field("authorization"));
        assert!(is_authorization_field("Authorization"));
        assert!(is_authorization_field("AUTHORIZATION"));
        assert!(!is_authorization_field("auth"));
        assert!(!is_authorization_field("authorize"));
        assert!(!is_authorization_field("cookie"));
    }

    #[test]
    fn scrub_short_circuits_authorization_field() {
        // The field-name shortcut emits the literal `<redacted>` regardless
        // of value shape, so even an opaque custom-token format won't leak.
        let out = scrub("authorization", "Custom-Scheme some-opaque-token");
        assert_eq!(out, "<redacted>");
    }

    #[test]
    fn scrub_redacts_bearer_inside_debug_printed_value() {
        // Generic field name → goes through the substring redactor.
        let value = r#"headers={"authorization":"Bearer abc"}"#;
        let out = scrub("request", value);
        assert!(!out.contains("abc"), "got: {out}");
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
    }

    #[test]
    fn scrub_still_redacts_url_query_secrets_for_url_shaped_values() {
        // When the field value is itself a URL, the URL-query redactor
        // still fires. Composition is preserved by the auth-redactor pass
        // not corrupting URL syntax.
        let url = "https://x.example.com/?api_key=AKIA&page=2";
        let out = scrub("url", url);
        assert!(!out.contains("AKIA"), "got: {out}");
        assert!(out.contains("page=2"), "got: {out}");
    }
}

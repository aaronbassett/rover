//! Tracing layer that redacts URL query-string values for keys in a
//! hardcoded denylist (`api_key`, `token`, `secret`, `password`).

use url::Url;

const TRIGGER_KEYS: &[&str] = &["api_key", "token", "secret", "password"];

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
        let redacted = redact_url(value);
        let _ = std::fmt::write(
            &mut *self.out,
            format_args!(" {}={}", field.name(), redacted),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let formatted = format!("{value:?}");
        let redacted = redact_url(&formatted);
        let _ = std::fmt::write(
            &mut *self.out,
            format_args!(" {}={}", field.name(), redacted),
        );
    }
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
}

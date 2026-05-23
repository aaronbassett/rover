//! Tracing initialization.

pub mod redact;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// Initialize tracing.
///
/// Writes structured logs to stderr. Honors the `RUST_LOG` env var; falls back
/// to `default_filter` (typically "info,rover=debug") when unset. URL query-
/// string values for `api_key`/`token`/`secret`/`password` keys are redacted
/// in the output.
///
/// Calling this more than once in the same process is a no-op (subsequent
/// calls return without re-initializing).
pub fn init(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let layer = fmt::layer()
        .with_writer(std::io::stderr)
        .event_format(redact::RedactingFormatEvent);

    // try_init: if already initialized (e.g. in tests), this is a no-op.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init("info");
        init("debug");
        // No assertion needed: the second call must not panic.
    }
}

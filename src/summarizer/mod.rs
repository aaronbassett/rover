//! Summarization subsystem.
//!
//! Exposes a `SummarizerBackend` trait and three concrete impls — `Extractive`
//! (TextRank, offline), `Cloud` (wraps `genai::Client`), and (M9-future)
//! `LocalMistralRs`. The `SummarizerService` (Task 7) wraps a `Registry`
//! (Task 6) plus the storage handle and owns the cache hot path.

pub mod backend;
pub mod error;

pub use backend::{CompactMode, CompactOpts, PreserveSection, Style, SummarizerBackend};
pub use error::{BackendError, SummarizerError};

use sha2::{Digest, Sha256};

/// Record separator used to disambiguate hash inputs.
const RS: char = '\u{1E}';

/// Deterministic params_hash for `summary_cache` lookups. Inputs are
/// serialized as plain strings — never via serde — so reorderings or
/// crate version changes can't shift the hash.
pub fn params_hash(opts: &CompactOpts, model_id: &str) -> String {
    let target = opts
        .target_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_string());
    let focus = opts
        .focus
        .as_deref()
        .map(|s| s.trim())
        .unwrap_or("")
        .to_string();
    let mut preserve_sorted: Vec<&'static str> = opts.preserve.iter().map(|p| p.as_str()).collect();
    preserve_sorted.sort();
    preserve_sorted.dedup();
    let preserve_csv = preserve_sorted.join(",");

    let serialized = format!(
        "{name}{RS}{model}{RS}{mode}{RS}{target}{RS}{focus}{RS}{preserve}{RS}{style}",
        name = opts.backend_name,
        model = model_id,
        mode = opts.mode.as_str(),
        target = target,
        focus = focus,
        preserve = preserve_csv,
        style = opts.style.as_str(),
    );

    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write as _;
        write!(s, "{b:02x}").expect("write to String never fails");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn baseline() -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(500),
            focus: Some("api shape".to_string()),
            preserve: vec![PreserveSection::Code, PreserveSection::Tables],
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn hash_is_deterministic_for_same_inputs() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o-mini");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn hash_changes_when_backend_name_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let mut other = baseline();
        other.backend_name = "smart".to_string();
        let b = params_hash(&other, "gpt-4o-mini");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_changes_when_model_id_changes() {
        let a = params_hash(&baseline(), "gpt-4o-mini");
        let b = params_hash(&baseline(), "gpt-4o");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_invariant_to_preserve_ordering() {
        let mut a_opts = baseline();
        a_opts.preserve = vec![PreserveSection::Code, PreserveSection::Tables];
        let mut b_opts = baseline();
        b_opts.preserve = vec![PreserveSection::Tables, PreserveSection::Code];
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_treats_target_none_as_null_string() {
        let mut o = baseline();
        o.target_tokens = None;
        let _ = params_hash(&o, "m");
        // Implicit: no panic; the difference vs Some(500) is exercised below.
        let h_none = params_hash(&o, "m");
        o.target_tokens = Some(500);
        let h_some = params_hash(&o, "m");
        assert_ne!(h_none, h_some);
    }

    #[test]
    fn focus_whitespace_normalization_collapses_to_same_hash() {
        let mut a_opts = baseline();
        a_opts.focus = Some("api shape".to_string());
        let mut b_opts = baseline();
        b_opts.focus = Some("  api shape  ".to_string());
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_eq!(a, b);
    }
}

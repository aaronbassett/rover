//! Summarization subsystem.
//!
//! Exposes a `SummarizerBackend` trait and three concrete impls — `Extractive`
//! (TextRank, offline), `Cloud` (wraps `genai::Client`), and (M9-future)
//! `LocalMistralRs`. The `SummarizerService` (Task 7) wraps a `Registry`
//! (Task 6) plus the storage handle and owns the cache hot path.

pub mod backend;
pub mod cloud;
pub mod error;
pub mod extractive;
pub mod prompts;

pub use backend::{CompactMode, CompactOpts, PreserveSection, Style, SummarizerBackend};
pub use error::{BackendError, SummarizerError};

use sha2::{Digest, Sha256};

/// Deterministic params_hash for `summary_cache` lookups. Inputs are
/// serialized as plain strings — never via serde — so reorderings or
/// crate version changes can't shift the hash. Length-prefix framing
/// (`{byte_len}:{content}`) makes the format unambiguous regardless of
/// whether any field contains delimiter-like bytes.
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

    let mut serialized = String::new();
    for s in [
        opts.backend_name.as_str(),
        model_id,
        opts.mode.as_str(),
        target.as_str(),
        focus.as_str(),
        preserve_csv.as_str(),
        opts.style.as_str(),
    ] {
        serialized.push_str(&format!("{}:{}", s.len(), s));
    }

    let mut h = Sha256::new();
    h.update(serialized.as_bytes());
    let bytes = h.finalize();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        write!(hex, "{b:02x}").expect("write to string never fails");
    }
    hex
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

    #[test]
    fn hash_resists_focus_delimiter_injection() {
        // Two distinct inputs must NOT collide even if focus contains
        // characters that resemble the framing.
        let mut a_opts = baseline();
        a_opts.focus = Some("a:b".to_string());
        a_opts.preserve = vec![];
        let mut b_opts = baseline();
        b_opts.focus = Some("a".to_string());
        b_opts.preserve = vec![PreserveSection::Code]; // arbitrary distinct value
        let a = params_hash(&a_opts, "m");
        let b = params_hash(&b_opts, "m");
        assert_ne!(a, b);

        // And U+001E specifically (the old separator) must not collide either.
        let mut c_opts = baseline();
        c_opts.focus = Some("a\u{1E}b".to_string());
        c_opts.preserve = vec![];
        let mut d_opts = baseline();
        d_opts.focus = Some("a".to_string());
        d_opts.preserve = vec![];
        let c = params_hash(&c_opts, "m");
        let d = params_hash(&d_opts, "m");
        assert_ne!(c, d);
    }
}

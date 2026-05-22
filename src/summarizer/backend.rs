//! Backend trait and compaction options.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::summarizer::error::BackendError;

/// Compaction modes (PRD §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactMode {
    Extractive,
    Abstractive,
    Headlines,
}

impl CompactMode {
    /// Stable string for params_hash and config parsing.
    pub fn as_str(self) -> &'static str {
        match self {
            CompactMode::Extractive => "extractive",
            CompactMode::Abstractive => "abstractive",
            CompactMode::Headlines => "headlines",
        }
    }
}

/// Compaction style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Bullet,
    Prose,
    Executive,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Style::Bullet => "bullet",
            Style::Prose => "prose",
            Style::Executive => "executive",
        }
    }
}

/// Section kinds the summarizer is asked to preserve verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreserveSection {
    Code,
    Tables,
    Quotes,
    Lists,
}

impl PreserveSection {
    pub fn as_str(self) -> &'static str {
        match self {
            PreserveSection::Code => "code",
            PreserveSection::Tables => "tables",
            PreserveSection::Quotes => "quotes",
            PreserveSection::Lists => "lists",
        }
    }
}

/// One summarization request after defaults have been merged.
///
/// `target_tokens` counts via the configured tokenizer family on the
/// service side; backends treat it as advisory text in the prompt
/// (Abstractive) or as a hard greedy cap (Extractive/Headlines).
///
/// Note: the derived `PartialEq`/`Eq` is order-sensitive on `preserve`
/// (it's a `Vec`), but the `params_hash` cache key is order-invariant —
/// it sorts and dedups `preserve` before hashing. Two `CompactOpts` that
/// compare non-equal under `==` may still produce the same cache key.
/// Keep this divergence in mind if you ever switch the cache key to
/// derive from the struct directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOpts {
    pub mode: CompactMode,
    pub style: Style,
    pub target_tokens: Option<usize>,
    pub focus: Option<String>,
    pub preserve: Vec<PreserveSection>,
    /// The resolved backend's config-key name (e.g. "default", "fast").
    /// Filled in by `SummarizerService::compact` from the registry; backends
    /// see it for logging only.
    pub backend_name: String,
}

#[async_trait]
pub trait SummarizerBackend: Send + Sync {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError>;

    /// Config-key name (e.g. "default", "fast").
    fn name(&self) -> &str;

    /// Resolved model identifier for `params_hash`. The default `""` is
    /// intended only for the extractive backend (which has no model). All
    /// cloud / network-backed backends MUST override this with their
    /// resolved model id so the cache key partitions across model versions.
    fn model_id(&self) -> &str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_as_str() {
        assert_eq!(CompactMode::Extractive.as_str(), "extractive");
        assert_eq!(CompactMode::Abstractive.as_str(), "abstractive");
        assert_eq!(CompactMode::Headlines.as_str(), "headlines");
        assert_eq!(Style::Bullet.as_str(), "bullet");
        assert_eq!(Style::Prose.as_str(), "prose");
        assert_eq!(Style::Executive.as_str(), "executive");
        assert_eq!(PreserveSection::Code.as_str(), "code");
    }

    #[test]
    fn compact_opts_is_clonable() {
        let o = CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(500),
            focus: Some("api shape".to_string()),
            preserve: vec![PreserveSection::Code],
            backend_name: "fast".to_string(),
        };
        let cloned = o.clone();
        assert_eq!(o, cloned);
    }
}

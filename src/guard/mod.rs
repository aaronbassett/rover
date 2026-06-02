//! Prompt-injection guard for content-returning MCP tools.
//!
//! See `docs/superpowers/specs/2026-06-02-prompt-injection-guard-design.md`.

pub mod allowlist;
pub mod normalize;
pub mod patterns;
pub mod wrap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Output-guard response level. A single configured level governs the action
/// taken on any detector hit (the action is detector-aware: span-level for
/// pattern hits, window-level for model hits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardLevel {
    /// Drop the entire body; return the warning only.
    Strict,
    /// Remove matched spans / offending windows.
    High,
    /// Wrap matched spans / windows in `<DANGER>…</DANGER>` + preamble warning.
    Moderate,
    /// Content intact; preamble warning only.
    Low,
    /// No detection (the wrapper still applies unless allowlisted).
    Disabled,
}

impl GuardLevel {
    pub fn parse(s: &str) -> Result<Self, GuardError> {
        match s {
            "strict" => Ok(Self::Strict),
            "high" => Ok(Self::High),
            "moderate" => Ok(Self::Moderate),
            "low" => Ok(Self::Low),
            "disabled" => Ok(Self::Disabled),
            other => Err(GuardError::UnknownLevel {
                level: other.to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::High => "high",
            Self::Moderate => "moderate",
            Self::Low => "low",
            Self::Disabled => "disabled",
        }
    }
}

/// One of the three guard methods, used as a key for allowlists and overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Wrap,
    Patterns,
    Model,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wrap => "wrap",
            Self::Patterns => "patterns",
            Self::Model => "model",
        }
    }
}

/// Which detector produced a [`Detection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    Patterns,
    Model,
}

/// A single detection. Byte offsets are into the **original** (pre-normalize)
/// text. Pattern detections carry a `technique` tag and a tight span; model
/// detections carry no technique and a 512-token-window byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub detector: Detector,
    pub technique: Option<String>,
    pub start: usize,
    pub end: usize,
}

/// Result of scanning a body with the enabled detectors.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub detections: Vec<Detection>,
    pub model_score: Option<f32>,
}

impl ScanResult {
    pub fn detected(&self) -> bool {
        !self.detections.is_empty()
    }
}

/// Structured telemetry surfaced in the trusted preamble (one-line summary),
/// the frontmatter `prompt_injection` block, and `MetadataResponse`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GuardTelemetry {
    pub scanned: bool,
    pub detected: bool,
    /// The level applied, e.g. `"moderate"`.
    pub action: String,
    /// Detectors that ran and hit, e.g. `["patterns", "model"]`.
    pub detectors: Vec<String>,
    pub techniques: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_score: Option<f32>,
    /// Methods skipped because the URL matched an allowlist.
    pub allowlisted: Vec<String>,
    /// Ungranted overrides the agent tried to set.
    pub overrides_attempted: Vec<String>,
}

/// Optional MCP `security` arg on each covered tool. Each field is honored
/// **only if** its corresponding `[prompt_injection.agent_overrides]` grant
/// is `true`; otherwise it is ignored and recorded in
/// `GuardTelemetry.overrides_attempted`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SecurityArg {
    #[serde(default)]
    pub disable_wrap: Option<bool>,
    #[serde(default)]
    pub disable_patterns: Option<bool>,
    #[serde(default)]
    pub disable_model: Option<bool>,
    /// Override the output level (e.g. `"low"`). Parsed via `GuardLevel::parse`.
    #[serde(default)]
    pub level: Option<String>,
}

#[derive(Debug, Error)]
pub enum GuardError {
    #[error(
        "unknown prompt_injection level `{level}` (expected one of: strict, high, moderate, low, disabled)"
    )]
    UnknownLevel { level: String },

    #[error("unknown prompt_injection model preset `{model}`")]
    UnknownModel { model: String },

    #[error("prompt_injection model `{model}` requires the `injection-model` cargo feature")]
    ModelFeatureNotCompiled { model: String },

    #[error("prompt_injection model load failed: {0}")]
    ModelLoad(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_level_round_trips() {
        for (s, lvl) in [
            ("strict", GuardLevel::Strict),
            ("high", GuardLevel::High),
            ("moderate", GuardLevel::Moderate),
            ("low", GuardLevel::Low),
            ("disabled", GuardLevel::Disabled),
        ] {
            assert_eq!(GuardLevel::parse(s).unwrap(), lvl);
            assert_eq!(lvl.as_str(), s);
        }
    }

    #[test]
    fn guard_level_rejects_unknown() {
        let err = GuardLevel::parse("paranoid").unwrap_err();
        assert!(matches!(err, GuardError::UnknownLevel { .. }));
    }

    #[test]
    fn method_as_str_table() {
        assert_eq!(Method::Wrap.as_str(), "wrap");
        assert_eq!(Method::Patterns.as_str(), "patterns");
        assert_eq!(Method::Model.as_str(), "model");
    }

    #[test]
    fn security_arg_parses_partial() {
        let a: SecurityArg =
            serde_json::from_str(r#"{"disable_patterns": true, "level": "low"}"#).unwrap();
        assert_eq!(a.disable_patterns, Some(true));
        assert_eq!(a.level.as_deref(), Some("low"));
        assert_eq!(a.disable_wrap, None);
        assert_eq!(a.disable_model, None);
    }

    #[test]
    fn security_arg_rejects_unknown_field() {
        let r: Result<SecurityArg, _> = serde_json::from_str(r#"{"bogus": 1}"#);
        assert!(r.is_err());
    }

    #[test]
    fn security_arg_default_is_all_none() {
        let a = SecurityArg::default();
        assert!(a.disable_wrap.is_none() && a.disable_patterns.is_none());
        assert!(a.disable_model.is_none() && a.level.is_none());
    }
}

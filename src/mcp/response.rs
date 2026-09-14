//! How a tool's result is laid out in its `CallToolResult`.
//!
//! rmcp's `Json<T>` puts the whole result in `structuredContent` *and*
//! serializes it again as JSON text in `content`, so every response carries
//! the result twice. Rover sends it once: `structuredContent` holds the
//! result and `content` holds a short [`STRUCTURED_CONTENT_HINT`]. A client
//! that can't show `structuredContent` to its model asks for the old layout
//! per call with `"compatibility_mode": "on"`.
//!
//! `structuredContent` is sent in both modes. Every tool advertises an
//! `outputSchema`, and the MCP spec requires a structured result whenever
//! one is advertised; the TypeScript SDK client throws without it.
//!
//! This module's [`Json`] replaces rmcp's `Json<T>` as the tools' return
//! type; see its docs for why it shares the name.

use rmcp::ErrorData;
use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// `content` in the default mode, in place of the result. It names the
/// argument and repeats the advice in the argument's schema description, so
/// whichever one an agent reads first, it learns the same thing.
pub const STRUCTURED_CONTENT_HINT: &str = "This tool responds with all data in `structuredContent`. \
     If you are unable to view the `structuredContent` fields in your client, call the tool \
     again with `\"compatibility_mode\": \"on\"`, and set it on every later call to any Rover tool.";

// A string enum rather than a bool, matching `headless: { mode: "on" | "off" }`
// and the wording of `STRUCTURED_CONTENT_HINT`. There is deliberately no config
// default: a shared HTTP server serves clients with different needs, so the
// choice belongs to the calling client. (Plain comments, not doc comments:
// schemars publishes doc comments to agents in every tool's `inputSchema`.)
//
// `inline` makes each tool's `compatibility_mode` property a self-contained
// `{oneOf, description, default}` rather than a `$ref` with siblings: draft-07
// ignores `$ref` siblings and some client converters drop them when inlining,
// which would lose the description exactly for the older clients it is for.
//
// `Deserialize` is implemented by hand below so only the strings "on" and
// "off" are accepted. The derived impl would also take the externally tagged
// map form (`{"on": null}`), which the schema does not advertise.
/// Whether a tool also returns its full result as JSON text in `content`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
pub enum CompatibilityMode {
    /// The full result as JSON text in `content`, plus `structuredContent`.
    On,
    /// The result in `structuredContent` only; `content` holds a short notice.
    #[default]
    Off,
}

impl<'de> Deserialize<'de> for CompatibilityMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const VARIANTS: &[&str] = &["on", "off"];
        match String::deserialize(deserializer)?.as_str() {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => Err(serde::de::Error::unknown_variant(other, VARIANTS)),
        }
    }
}

/// A tool result plus the caller's [`CompatibilityMode`].
///
/// The name is load-bearing. rmcp's `#[tool]` macro derives a tool's
/// `outputSchema` only when the return type's last path segment is literally
/// `Json` (`Json<T>` or `Result<Json<T>, E>`), emitting
/// `schema_for_output::<T>()`. Naming this wrapper `Json` keeps that
/// derivation, so each tool's advertised schema always comes from the type it
/// really returns; a differently named wrapper gets no `outputSchema` at all,
/// silently. It is not rmcp's `Json` because that one always duplicates the
/// result into `content`. (This doc comment is never published: the wrapper
/// has no `JsonSchema` impl, and the schema is `T`'s.)
pub struct Json<T> {
    value: T,
    compatibility_mode: CompatibilityMode,
}

impl<T> Json<T> {
    pub fn new(value: T, compatibility_mode: CompatibilityMode) -> Self {
        Self {
            value,
            compatibility_mode,
        }
    }
}

impl<T: Serialize> IntoCallToolResult for Json<T> {
    fn into_call_tool_result(self) -> Result<CallToolResult, ErrorData> {
        let value = serde_json::to_value(self.value).map_err(|e| {
            ErrorData::internal_error(format!("Failed to serialize structured content: {e}"), None)
        })?;
        Ok(match self.compatibility_mode {
            // Exactly what rmcp's `Json<T>` produces.
            CompatibilityMode::On => CallToolResult::structured(value),
            CompatibilityMode::Off => {
                let mut result =
                    CallToolResult::success(vec![Content::text(STRUCTURED_CONTENT_HINT)]);
                result.structured_content = Some(value);
                result
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Json as RmcpJson;

    #[derive(Serialize, JsonSchema)]
    struct Sample {
        page: &'static str,
    }

    fn text(result: &CallToolResult) -> Vec<&str> {
        result
            .content
            .iter()
            .map(|c| c.as_text().expect("text content").text.as_str())
            .collect()
    }

    #[test]
    fn default_mode_sends_the_result_once_with_a_hint() {
        let result = Json::new(Sample { page: "body" }, CompatibilityMode::default())
            .into_call_tool_result()
            .unwrap();
        assert_eq!(text(&result), [STRUCTURED_CONTENT_HINT]);
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "page": "body" }))
        );
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn compatibility_mode_matches_rmcp_json() {
        let ours = Json::new(Sample { page: "body" }, CompatibilityMode::On)
            .into_call_tool_result()
            .unwrap();
        let rmcp = RmcpJson(Sample { page: "body" })
            .into_call_tool_result()
            .unwrap();
        assert_eq!(ours, rmcp);
    }

    #[test]
    fn wire_values_are_on_and_off() {
        assert_eq!(
            serde_json::from_str::<CompatibilityMode>(r#""on""#).unwrap(),
            CompatibilityMode::On
        );
        assert_eq!(
            serde_json::from_str::<CompatibilityMode>(r#""off""#).unwrap(),
            CompatibilityMode::Off
        );
        // Strings only: the derived impl would also take the externally
        // tagged map form, which the schema does not advertise.
        for bad in [
            r#""yes""#,
            "true",
            r#""ON""#,
            "null",
            r#"{"on":null}"#,
            r#"{"off":null}"#,
        ] {
            assert!(
                serde_json::from_str::<CompatibilityMode>(bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn schema_is_exactly_the_two_strings() {
        let schema = serde_json::to_value(schemars::schema_for!(CompatibilityMode)).unwrap();
        let values: Vec<&serde_json::Value> = schema["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("no oneOf: {schema}"))
            .iter()
            .map(|v| {
                assert_eq!(v["type"], "string", "{schema}");
                &v["const"]
            })
            .collect();
        assert_eq!(values, ["on", "off"], "{schema}");
    }

    #[test]
    fn hint_names_the_argument_and_its_value() {
        assert!(STRUCTURED_CONTENT_HINT.contains(r#""compatibility_mode": "on""#));
        assert!(STRUCTURED_CONTENT_HINT.contains("structuredContent"));
    }
}

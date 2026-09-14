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
//! [`ToolResponse`] replaces `Json<T>` as the tools' return type. rmcp's
//! `#[tool]` macro only derives `outputSchema` from a return type whose last
//! path segment is literally `Json`, so every tool returning a
//! `ToolResponse` must name its schema with
//! `#[tool(output_schema = output_schema::<T>())]` — without it the schema
//! silently disappears from `tools/list`.

use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::{CallToolResult, Content, JsonObject};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
/// Whether a tool also returns its full result as JSON text in `content`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompatibilityMode {
    /// The full result as JSON text in `content`, plus `structuredContent`.
    On,
    /// The result in `structuredContent` only; `content` holds the hint.
    #[default]
    Off,
}

/// A tool result plus the caller's [`CompatibilityMode`].
pub struct ToolResponse<T> {
    value: T,
    compatibility_mode: CompatibilityMode,
}

impl<T> ToolResponse<T> {
    pub fn new(value: T, compatibility_mode: CompatibilityMode) -> Self {
        Self {
            value,
            compatibility_mode,
        }
    }
}

impl<T: Serialize> IntoCallToolResult for ToolResponse<T> {
    fn into_call_tool_result(self) -> Result<CallToolResult, ErrorData> {
        let value = serde_json::to_value(self.value).map_err(|e| {
            ErrorData::internal_error(format!("Failed to serialize structured content: {e}"), None)
        })?;
        Ok(match self.compatibility_mode {
            // Exactly what `Json<T>` produces.
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

/// The `outputSchema` for a tool returning `ToolResponse<T>` — the same
/// schema `#[tool]` derives for `Json<T>`, and it panics on an invalid schema
/// the same way.
pub fn output_schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    rmcp::handler::server::tool::schema_for_output::<T>().unwrap_or_else(|e| {
        panic!(
            "Invalid output schema for {}: {e}",
            std::any::type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Json;

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
        let result = ToolResponse::new(Sample { page: "body" }, CompatibilityMode::default())
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
        let ours = ToolResponse::new(Sample { page: "body" }, CompatibilityMode::On)
            .into_call_tool_result()
            .unwrap();
        let rmcp = Json(Sample { page: "body" })
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
        for bad in [r#""yes""#, "true", r#""ON""#] {
            assert!(
                serde_json::from_str::<CompatibilityMode>(bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn hint_names_the_argument_and_its_value() {
        assert!(STRUCTURED_CONTENT_HINT.contains(r#""compatibility_mode": "on""#));
        assert!(STRUCTURED_CONTENT_HINT.contains("structuredContent"));
    }
}

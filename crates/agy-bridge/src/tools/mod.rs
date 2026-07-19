//! Custom tool registration for the Antigravity SDK bridge.
//!
//! This module re-exports types from the `llm_tool` crate, which provides
//! framework-agnostic tool definitions. The explicit re-exports ensure backward
//! compatibility — existing `use agy_bridge::tools::ToolRegistry` imports
//! continue to work, while giving this crate control over its public API surface.

// Re-export proc-macro helpers used by `#[llm_tool]` generated code.
// These are `#[doc(hidden)]` in the `llm_tool` crate and should not
// appear in user-facing documentation.
#[doc(hidden)]
pub use llm_tool::__private;
pub use llm_tool::{
    EmptyParams, Json, JsonSchema, RustTool, ToolContext, ToolDefinition, ToolError, ToolOutput,
    ToolRegistry, definition_of,
};

// ── Available tool discovery types ──────────────────────────────────

/// Where a tool originates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    /// SDK builtin tool (e.g. `view_file`, `run_command`) — implemented by
    /// the Antigravity SDK backend, not by user code.
    Builtin,
    /// Custom Rust tool registered via [`ToolRegistry`].
    Custom,
    /// Tool discovered from a connected MCP server.
    Mcp,
}

impl std::fmt::Display for ToolSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => f.write_str("builtin"),
            Self::Custom => f.write_str("custom"),
            Self::Mcp => f.write_str("mcp"),
        }
    }
}

/// A tool available to an agent, with metadata about its origin.
///
/// Returned by [`AgentHandle::available_tools()`](crate::agent::AgentHandle::available_tools).
/// Includes tools from all sources: SDK builtins, custom Rust tools, and MCP
/// server tools.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AvailableTool {
    /// Tool name as seen by the model (e.g. `"view_file"`, `"get_weather"`).
    pub name: String,
    /// Human-readable description. Empty if the tool source didn't provide one.
    pub description: String,
    /// JSON Schema for the tool's parameters. `Value::Null` if unavailable.
    pub parameter_schema: serde_json::Value,
    /// Where this tool originates from.
    pub source: ToolSource,
}

impl std::fmt::Display for AvailableTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]", self.name, self.source)
    }
}

// ── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ToolSource tests ──────────────────────────────────────────────

    #[test]
    fn tool_source_display() {
        assert_eq!(ToolSource::Builtin.to_string(), "builtin");
        assert_eq!(ToolSource::Custom.to_string(), "custom");
        assert_eq!(ToolSource::Mcp.to_string(), "mcp");
    }

    #[test]
    fn tool_source_serde_roundtrip() {
        for source in [ToolSource::Builtin, ToolSource::Custom, ToolSource::Mcp] {
            let json = serde_json::to_string(&source).unwrap();
            let parsed: ToolSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, source);
        }
    }

    #[test]
    fn tool_source_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&ToolSource::Builtin).unwrap(),
            "\"builtin\""
        );
        assert_eq!(
            serde_json::to_string(&ToolSource::Custom).unwrap(),
            "\"custom\""
        );
        assert_eq!(serde_json::to_string(&ToolSource::Mcp).unwrap(), "\"mcp\"");
    }

    // ── AvailableTool tests ───────────────────────────────────────────

    #[test]
    fn available_tool_display() {
        let tool = AvailableTool {
            name: "get_weather".to_owned(),
            description: "Gets weather.".to_owned(),
            parameter_schema: serde_json::Value::Null,
            source: ToolSource::Mcp,
        };
        assert_eq!(tool.to_string(), "get_weather [mcp]");
    }

    #[test]
    fn available_tool_serde_roundtrip() {
        let tool = AvailableTool {
            name: "view_file".to_owned(),
            description: "Read file contents.".to_owned(),
            parameter_schema: serde_json::json!({"type": "object"}),
            source: ToolSource::Builtin,
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: AvailableTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "view_file");
        assert_eq!(parsed.source, ToolSource::Builtin);
        assert!(!parsed.description.is_empty());
    }
}

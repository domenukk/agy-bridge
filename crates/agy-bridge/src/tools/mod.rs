//! Custom tool registration for the Antigravity SDK bridge.
//!
//! This module re-exports types from the `llm_tool` crate, which provides
//! framework-agnostic tool definitions. The explicit re-exports ensure backward
//! compatibility — existing `use agy_bridge::tools::ToolRegistry` imports
//! continue to work, while giving this crate control over its public API surface.

pub use llm_tool::{
    EmptyParams, Json, JsonSchema, RustTool, ToolContext, ToolDefinition, ToolError, ToolOutput,
    ToolRegistry, definition_of,
};

// Re-export proc-macro helpers used by `#[llm_tool]` generated code.
// These are `#[doc(hidden)]` in the `llm_tool` crate and should not
// appear in user-facing documentation.
#[doc(hidden)]
pub use llm_tool::__private;

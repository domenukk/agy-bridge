//! Agent configuration bridge types.
//!
//! Centralizes all configuration structures for the Antigravity SDK bridge:
//! model selection, tool capabilities, MCP server setup, and agent parameters.

pub mod agent;
pub mod capabilities;
pub mod mcp;
pub mod mcp_json;
pub mod models;

pub use agent::*;
pub use capabilities::*;
pub use mcp::*;
pub use mcp_json::*;
pub use models::*;

/// Default primary model name.
pub const DEFAULT_MODEL: &str = "gemini-3.5-flash";
/// Default image generation model name.
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gemini-3.1-flash-image-preview";

const DEFAULT_MCP_TIMEOUT_SECS: f64 = 30.0;
const DEFAULT_MCP_SSE_READ_TIMEOUT_SECS: f64 = 300.0;

pub(crate) fn default_image_model() -> String {
    DEFAULT_IMAGE_GENERATION_MODEL.to_owned()
}
pub(crate) const fn default_mcp_timeout() -> f64 {
    DEFAULT_MCP_TIMEOUT_SECS
}
pub(crate) const fn default_mcp_sse_read_timeout() -> f64 {
    DEFAULT_MCP_SSE_READ_TIMEOUT_SECS
}
pub(crate) const fn default_true() -> bool {
    true
}

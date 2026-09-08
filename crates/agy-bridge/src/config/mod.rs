//! Agent configuration bridge types.
//!
//! Centralizes all configuration structures for the Antigravity SDK bridge:
//! model selection, tool capabilities, MCP server setup, and agent parameters.

pub mod agent;
pub mod budget;
pub mod capabilities;
pub mod mcp;
pub mod mcp_json;
pub mod models;
pub mod subagents;

pub use agent::*;
pub use budget::*;
pub use capabilities::*;
pub use mcp::*;
pub use mcp_json::*;
pub use models::*;
pub use subagents::*;

/// Default primary model name.
pub const DEFAULT_MODEL: &str = "gemini-3.8-flash";
/// Default image generation model name.
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gemini-3.1-flash-lite-image";

/// Environment variable name for the Gemini API key.
pub const ENV_GEMINI_API_KEY: &str = "GEMINI_API_KEY";
/// Environment variable name for the Gemini API base URL (e.g. proxy endpoint).
pub const ENV_GEMINI_API_BASE_URL: &str = "GEMINI_API_BASE_URL";
/// Sentinel API key value used when routing through a proxy or gateway
/// that handles authentication externally (e.g. via LOAS/gcert or mTLS).
pub const PROXY_AUTH_SENTINEL: &str = "__agy_proxy_auth__";

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

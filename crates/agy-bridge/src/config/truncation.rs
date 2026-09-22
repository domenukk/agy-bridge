//! Configuration for truncating large tool outputs.

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// Configuration for truncating large tool outputs.
///
/// When a tool's output exceeds `max_tokens`, the harness preserves the prefix
/// of the output up to the limit and truncates the remainder (tail), appending
/// a notice informing the model that the output was truncated.
///
/// Setting `max_tokens` to 0 explicitly disables truncation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TypedBuilder, Default,
)]
#[builder(field_defaults(default))]
pub struct ToolOutputTruncationConfig {
    /// Maximum number of tokens allowed for a single tool output.
    /// Setting to 0 disables truncation.
    #[builder(default)]
    pub max_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_output_truncation_config_defaults() {
        let default_cfg = ToolOutputTruncationConfig::default();
        assert_eq!(default_cfg.max_tokens, 0);

        let custom = ToolOutputTruncationConfig::builder()
            .max_tokens(2048)
            .build();
        assert_eq!(custom.max_tokens, 2048);
    }

    #[test]
    fn test_tool_output_truncation_config_serde_roundtrip() {
        let custom = ToolOutputTruncationConfig::builder()
            .max_tokens(1024)
            .build();
        let json = serde_json::to_string(&custom).expect("serialize");
        let parsed: ToolOutputTruncationConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(custom, parsed);
    }
}

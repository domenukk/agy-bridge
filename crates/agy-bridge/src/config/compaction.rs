//! Configuration for conversation trajectory compaction and context limits.

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// Default compaction threshold for 1M-token context models (950,000 tokens).
pub const DEFAULT_COMPACTION_TOKEN_THRESHOLD: u32 = 950_000;

/// Default maximum context window for 1M-token context models (1,000,000 tokens).
pub const DEFAULT_MAX_CONTEXT_TOKENS: u32 = 1_000_000;

/// Configuration for conversation trajectory compaction and context limits.
///
/// Antigravity manages context by compacting older conversation history when
/// the active trajectory exceeds `token_threshold`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TypedBuilder, Default)]
#[builder(field_defaults(default))]
pub struct CompactionConfig {
    /// Token ceiling allowed for the conversation history before compaction occurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub token_threshold: Option<u32>,
    /// Interval in tokens at which checkpoints are created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub checkpoint_interval_tokens: Option<u32>,
    /// Maximum allowed context tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_context_tokens: Option<u32>,
    /// Optional custom prompt used when summarizing compacted history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub summary_prompt_override: Option<String>,
    /// Whether to use the model-reported token count when evaluating compaction thresholds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub use_model_reported_token_count: Option<bool>,
}

impl CompactionConfig {
    /// Creates a `CompactionConfig` configured for a full 1M-token context window
    /// (`token_threshold = 950_000`, `max_context_tokens = 1_000_000`).
    #[must_use]
    pub const fn one_million() -> Self {
        Self {
            token_threshold: Some(DEFAULT_COMPACTION_TOKEN_THRESHOLD),
            checkpoint_interval_tokens: None,
            max_context_tokens: Some(DEFAULT_MAX_CONTEXT_TOKENS),
            summary_prompt_override: None,
            use_model_reported_token_count: None,
        }
    }

    /// Sets an optional custom prompt used when summarizing compacted history.
    #[must_use]
    pub fn with_summary_prompt_override(mut self, prompt: impl Into<String>) -> Self {
        self.summary_prompt_override = Some(prompt.into());
        self
    }

    /// Sets whether to use the model-reported token count when evaluating compaction thresholds.
    #[must_use]
    pub const fn with_use_model_reported_token_count(mut self, enabled: bool) -> Self {
        self.use_model_reported_token_count = Some(enabled);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_config_builder_and_defaults() {
        let default_cfg = CompactionConfig::default();
        assert_eq!(default_cfg.token_threshold, None);
        assert_eq!(default_cfg.checkpoint_interval_tokens, None);
        assert_eq!(default_cfg.max_context_tokens, None);

        let custom = CompactionConfig::builder()
            .token_threshold(50_000)
            .checkpoint_interval_tokens(10_000)
            .max_context_tokens(100_000)
            .build();
        assert_eq!(custom.token_threshold, Some(50_000));
        assert_eq!(custom.checkpoint_interval_tokens, Some(10_000));
        assert_eq!(custom.max_context_tokens, Some(100_000));
    }

    #[test]
    fn test_compaction_config_serde_roundtrip() {
        let custom = CompactionConfig::builder().token_threshold(40_000).build();
        let json = serde_json::to_string(&custom).expect("serialize");
        let parsed: CompactionConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(custom, parsed);
    }
}

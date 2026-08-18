//! Execution budget and quota limits for agent runs.

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// Execution budget limits for an agent session.
///
/// Setting these limits causes the harness to automatically halt execution
/// with a `StopReason` when any ceiling is breached.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TypedBuilder, Default,
)]
#[builder(field_defaults(default))]
pub struct BudgetConfig {
    /// Maximum number of model inference calls allowed for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_model_calls: Option<u32>,
    /// Maximum number of tool executions allowed for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_tool_calls: Option<u32>,
    /// Maximum input/prompt tokens consumed across the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_input_tokens: Option<u64>,
    /// Maximum output/candidate tokens generated across the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_output_tokens: Option<u64>,
    /// Maximum total tokens (prompt + output + thinking) across the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub max_total_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_config_builder_and_defaults() {
        let budget = BudgetConfig::default();
        assert_eq!(budget.max_model_calls, None);
        assert_eq!(budget.max_tool_calls, None);
        assert_eq!(budget.max_input_tokens, None);
        assert_eq!(budget.max_output_tokens, None);
        assert_eq!(budget.max_total_tokens, None);

        let custom = BudgetConfig::builder()
            .max_model_calls(10)
            .max_tool_calls(50)
            .max_input_tokens(100_000)
            .max_output_tokens(20_000)
            .max_total_tokens(120_000)
            .build();

        assert_eq!(custom.max_model_calls, Some(10));
        assert_eq!(custom.max_tool_calls, Some(50));
        assert_eq!(custom.max_input_tokens, Some(100_000));
        assert_eq!(custom.max_output_tokens, Some(20_000));
        assert_eq!(custom.max_total_tokens, Some(120_000));
    }

    #[test]
    fn test_budget_config_serde_roundtrip() {
        let custom = BudgetConfig::builder()
            .max_model_calls(5)
            .max_total_tokens(50_000)
            .build();

        let json = serde_json::to_string(&custom).expect("serialize");
        let parsed: BudgetConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(custom, parsed);
    }
}

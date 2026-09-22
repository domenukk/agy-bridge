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
    /// Evaluation scope for this budget configuration. Defaults to [`BudgetScope::Lifetime`].
    #[serde(default)]
    #[builder(default)]
    pub scope: BudgetScope,
}

/// Evaluation scope for budget limits and caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BudgetScope {
    /// Budget is evaluated against cumulative spend from the start of the session.
    #[default]
    Lifetime,
    /// Budget is evaluated against spend starting from when the budget was configured.
    ForwardLooking,
}

impl BudgetScope {
    /// Return the wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lifetime => "LIFETIME",
            Self::ForwardLooking => "FORWARD_LOOKING",
        }
    }

    /// Return the protobuf integer value.
    #[must_use]
    pub const fn to_proto_i32(self) -> i32 {
        match self {
            Self::Lifetime => 1,
            Self::ForwardLooking => 2,
        }
    }

    /// Convert from protobuf integer value.
    #[must_use]
    pub const fn from_proto_i32(val: i32) -> Option<Self> {
        match val {
            1 => Some(Self::Lifetime),
            2 => Some(Self::ForwardLooking),
            _ => None,
        }
    }
}

impl std::fmt::Display for BudgetScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for BudgetScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "LIFETIME" => Ok(Self::Lifetime),
            "FORWARD_LOOKING" => Ok(Self::ForwardLooking),
            other => Err(format!("Unknown budget scope: {other}")),
        }
    }
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
        assert_eq!(budget.scope, BudgetScope::Lifetime);

        let custom = BudgetConfig::builder()
            .max_model_calls(10)
            .max_tool_calls(50)
            .max_input_tokens(100_000)
            .max_output_tokens(20_000)
            .max_total_tokens(120_000)
            .scope(BudgetScope::ForwardLooking)
            .build();

        assert_eq!(custom.max_model_calls, Some(10));
        assert_eq!(custom.max_tool_calls, Some(50));
        assert_eq!(custom.max_input_tokens, Some(100_000));
        assert_eq!(custom.max_output_tokens, Some(20_000));
        assert_eq!(custom.max_total_tokens, Some(120_000));
        assert_eq!(custom.scope, BudgetScope::ForwardLooking);
    }

    #[test]
    fn test_budget_config_serde_roundtrip() {
        let custom = BudgetConfig::builder()
            .max_model_calls(5)
            .max_total_tokens(50_000)
            .scope(BudgetScope::ForwardLooking)
            .build();

        let json = serde_json::to_string(&custom).expect("serialize");
        let parsed: BudgetConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(custom, parsed);
    }

    #[test]
    fn test_budget_scope_proto_and_str() {
        assert_eq!(BudgetScope::Lifetime.as_str(), "LIFETIME");
        assert_eq!(BudgetScope::ForwardLooking.as_str(), "FORWARD_LOOKING");
        assert_eq!(BudgetScope::Lifetime.to_proto_i32(), 1);
        assert_eq!(BudgetScope::ForwardLooking.to_proto_i32(), 2);
        assert_eq!(BudgetScope::from_proto_i32(1), Some(BudgetScope::Lifetime));
        assert_eq!(
            BudgetScope::from_proto_i32(2),
            Some(BudgetScope::ForwardLooking)
        );
        assert_eq!(BudgetScope::from_proto_i32(0), None);
    }
}

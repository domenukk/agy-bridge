//! Custom subagent definitions and capabilities.

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use super::{AgentBehavior, BuiltinTools, SystemInstructions, capabilities::RunCommandConfig};

/// Capabilities configuration specifically for a custom subagent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder, Default)]
#[builder(field_defaults(default))]
pub struct SubagentCapabilities {
    /// Behavioral mode of the subagent.
    #[serde(default)]
    pub agent_behavior: AgentBehavior,
    /// Whitelist of subagent names this subagent is allowed to spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub allowed_subagents: Option<Vec<String>>,
    /// Builtin tools enabled for this subagent (allowlist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub enabled_tools: Option<Vec<BuiltinTools>>,
    /// Builtin tools disabled for this subagent (denylist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(strip_option))]
    pub disabled_tools: Option<Vec<BuiltinTools>>,
    /// Configuration for the `run_command` builtin tool for this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub run_command_config: Option<RunCommandConfig>,
    /// Configuration for truncating large tool outputs for this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub tool_output_truncation_config: Option<super::truncation::ToolOutputTruncationConfig>,
}

/// Configuration for defining a custom subagent available to the primary agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct SubagentConfig {
    /// Unique name of the subagent.
    #[builder(setter(into))]
    pub name: String,
    /// Description of the subagent's role and purpose.
    #[builder(setter(into))]
    pub description: String,
    /// System instructions specific to this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub system_instructions: Option<SystemInstructions>,
    /// Capabilities and tool restrictions for this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub capabilities: Option<SubagentCapabilities>,
    /// Custom tool names or definitions enabled for this subagent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default, setter(transform = |v: impl IntoIterator<Item = impl Into<String>>| v.into_iter().map(Into::into).collect()))]
    pub tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_config_builder() {
        let sub = SubagentConfig::builder()
            .name("researcher")
            .description("Performs deep web and code research")
            .system_instructions("You are a specialized researcher.")
            .capabilities(
                SubagentCapabilities::builder()
                    .agent_behavior(AgentBehavior::Autonomous)
                    .allowed_subagents(vec!["summarizer".to_string()])
                    .enabled_tools(vec![BuiltinTools::SearchWeb, BuiltinTools::ReadUrlContent])
                    .build(),
            )
            .tools(vec!["custom_scraper".to_string()])
            .build();

        assert_eq!(sub.name, "researcher");
        assert_eq!(sub.description, "Performs deep web and code research");
        assert!(sub.capabilities.is_some());
        let caps = sub.capabilities.unwrap();
        assert_eq!(caps.agent_behavior, AgentBehavior::Autonomous);
        assert_eq!(caps.allowed_subagents, Some(vec!["summarizer".to_string()]));
        assert_eq!(sub.tools, vec!["custom_scraper"]);
    }

    #[test]
    fn test_subagent_config_serde_roundtrip() {
        let sub = SubagentConfig::builder()
            .name("coder")
            .description("Writes code")
            .build();

        let json = serde_json::to_string(&sub).expect("serialize");
        let parsed: SubagentConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sub.name, parsed.name);
        assert_eq!(sub.description, parsed.description);
    }
}

//! Custom subagent definitions, capabilities, and inline skill configurations.

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use super::{AgentBehavior, BuiltinTools, SystemInstructions, capabilities::RunCommandConfig};

/// An inline skill definition provided directly in memory rather than loaded from disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Default)]
#[builder(field_defaults(default))]
pub struct InlineSkill {
    /// Unique identifier for the skill.
    #[builder(setter(into))]
    pub name: String,
    /// Concise summary of when and how the skill should be used.
    #[builder(setter(into))]
    pub description: String,
    /// Full Markdown instruction body of the skill.
    #[builder(setter(into))]
    pub content: String,
    /// Optional allowlist of tools the skill is permitted to invoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<String>>| v.into_iter().map(Into::into).collect()))]
    pub allowed_tools: Vec<String>,
    /// Tools that must be enabled for this skill to function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<String>>| v.into_iter().map(Into::into).collect()))]
    pub dependent_tools: Vec<String>,
    /// Other skills that this skill depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<String>>| v.into_iter().map(Into::into).collect()))]
    pub dependent_skills: Vec<String>,
    /// Arbitrary key-value metadata attached to the skill.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Controls how a custom subagent inherits or overrides parent skills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentSkillsConfig {
    /// Inherit parent skills (optionally filtered by `skill_names`) and append `extra_skills_paths`.
    Inherit {
        /// Specific parent skill names to inherit (empty means inherit all).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skill_names: Vec<String>,
        /// Additional skill directories to load for this subagent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_skills_paths: Vec<PathBuf>,
    },
    /// Disable all skills for this subagent.
    None,
    /// Replace parent skills with an explicit set of skill paths and/or inline skills.
    Override {
        /// Filesystem paths to skills for this subagent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills_paths: Vec<PathBuf>,
        /// Inline skills provided directly to this subagent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inline_skills: Vec<InlineSkill>,
    },
}

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
    /// Optional model name override for this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub model: Option<String>,
    /// Optional skill inheritance or override rules for this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub skills_config: Option<SubagentSkillsConfig>,
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

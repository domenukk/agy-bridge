//! Agent configuration, system instructions, and local agent config.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use super::{
    DEFAULT_MODEL, capabilities::CapabilitiesConfig, mcp::McpServer, models::GeminiConfig,
};
use crate::{
    hooks::HookEntry, policies::PolicyRule, tools::ToolDefinition, triggers::TriggerEntry,
};

/// A section within a system instruction, with a label and body text.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SystemInstructionSection {
    /// The content of the section.
    pub content: String,
    /// The title/label for this section.
    #[serde(default = "default_section_title")]
    pub title: String,
}

fn default_section_title() -> String {
    "user_system_instructions".to_owned()
}

fn default_model_name() -> String {
    DEFAULT_MODEL.to_owned()
}

/// System instruction configuration, mirroring the Python SDK's union type.
///
/// Uses internal tagging via `#[serde(untagged)]` so each variant is
/// distinguishable by its `"mode"` field in JSON (e.g. `{"mode": "Custom", "text": "..."}`).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemInstructions {
    /// Completely replace the default system instructions (advanced usage).
    Custom(String),
    /// Override identity and/or append sections to the defaults (recommended).
    Templated {
        /// Optional identity string that replaces the agent's default persona.
        #[serde(default)]
        identity: Option<String>,
        /// Sections appended to the default system instructions.
        #[serde(default)]
        sections: Vec<SystemInstructionSection>,
    },
}

impl SystemInstructions {
    /// Create custom system instructions from a plain text string.
    #[must_use]
    pub fn custom(text: impl Into<String>) -> Self {
        Self::Custom(text.into())
    }
}

impl From<&str> for SystemInstructions {
    fn from(s: &str) -> Self {
        Self::custom(s)
    }
}

impl From<String> for SystemInstructions {
    fn from(s: String) -> Self {
        Self::custom(s)
    }
}

// ─── JSON Schema newtype ──────────────────────────────────────────────────────────────────

/// A JSON Schema definition for structured output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonSchema(serde_json::Value);

impl JsonSchema {
    #[must_use]
    /// Wrap a raw `serde_json::Value` as a `JsonSchema`.
    pub const fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    #[must_use]
    /// Return a reference to the inner JSON value.
    pub const fn as_value(&self) -> &serde_json::Value {
        &self.0
    }

    /// Validate that the schema is structurally sound.
    ///
    /// Currently checks that the top-level value is a JSON object (i.e.
    /// `serde_json::Value::Object`), which is the minimum requirement for a
    /// valid JSON Schema.
    ///
    /// # Errors
    ///
    /// Returns a static error message if the schema is not an object.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.0.is_object() {
            Ok(())
        } else {
            Err("JSON Schema must be a JSON object at the top level")
        }
    }
}

// ─── AgentConfig ─────────────────────────────────────────────────────────────

/// Full configuration for creating an agent.
///
/// Covers model selection, system instructions, capabilities, tools,
/// policies, hooks, MCP servers, structured output, and Gemini backend
/// settings. All fields have sensible defaults.
///
/// # Construction patterns
///
/// `AgentConfig` deliberately supports **two** construction paths:
///
/// 1. **[`TypedBuilder`]** — ergonomic chained construction with `impl
///    IntoIterator` setters for collection fields. Preferred for
///    programmatic use:
///    ```
///    # use agy_bridge::config::AgentConfig;
///    let config = AgentConfig::builder()
///        .model("gemini-3.5-flash")
///        .build();
///    ```
///
/// 2. **Struct literal with `..Default::default()`** — convenient for
///    deserialization (`serde`), config files, and framework code that
///    already has fully-formed values:
///    ```
///    # use agy_bridge::config::AgentConfig;
///    let config = AgentConfig {
///        model: "gemini-3.5-flash".into(),
///        ..AgentConfig::default()
///    };
///    ```
///
/// Both paths are supported intentionally. The builder provides ergonomic
/// setters (e.g. accepting `impl IntoIterator` for collection fields),
/// while struct literals enable direct field access for serialization
/// roundtrips and downstream framework integration.
#[derive(Debug, Clone, Serialize, Deserialize, TypedBuilder)]
#[builder(field_defaults(default))]
pub struct AgentConfig {
    /// The model name (e.g. `"gemini-3.5-flash"`).
    #[serde(default = "default_model_name")]
    #[builder(default = DEFAULT_MODEL.to_owned(), setter(into))]
    pub model: String,
    /// API key. Falls back to `GEMINI_API_KEY` env var if `None`.
    #[serde(default)]
    #[builder(setter(into, strip_option))]
    pub api_key: Option<String>,
    /// Optional system instructions (custom text or templated sections).
    #[builder(setter(into, strip_option))]
    pub system_instructions: Option<SystemInstructions>,
    #[serde(default)]
    /// Agent capability toggles (tool lists, subagents, compaction).
    #[builder(setter(strip_option))]
    pub capabilities: Option<CapabilitiesConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Workspace directories the agent can access.
    ///
    /// When empty (the default), the Python SDK's own default of
    /// `[os.getcwd()]` applies. Set explicitly to override.
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<PathBuf>>| v.into_iter().map(Into::into).collect()))]
    pub workspaces: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Custom tool definitions exposed to the agent.
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<ToolDefinition>>| v.into_iter().map(Into::into).collect()))]
    pub tools: Vec<ToolDefinition>,
    #[serde(default = "default_policies")]
    /// Policy rules evaluated before each tool call.
    #[builder(default = default_policies(), setter(transform = |v: impl IntoIterator<Item = impl Into<PolicyRule>>| v.into_iter().map(Into::into).collect()))]
    pub policies: Vec<PolicyRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Event-driven triggers attached to this agent.
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<TriggerEntry>>| v.into_iter().map(Into::into).collect()))]
    pub triggers: Vec<TriggerEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Lifecycle hooks (pre-turn, post-turn, pre-tool, etc.).
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<HookEntry>>| v.into_iter().map(Into::into).collect()))]
    pub hooks: Vec<HookEntry>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "skills_paths"
    )]
    /// Paths to skill instruction files loaded into the agent.
    ///
    /// Serializes as `"skills_paths"` to match the Python SDK field name.
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<PathBuf>>| v.into_iter().map(Into::into).collect()))]
    pub skills: Vec<PathBuf>,

    /// MCP server configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<McpServer>>| v.into_iter().map(Into::into).collect()))]
    pub mcp_servers: Vec<McpServer>,
    /// Pre-existing conversation ID to resume.
    #[serde(default)]
    #[builder(setter(into, strip_option))]
    pub conversation_id: Option<String>,
    /// Directory where conversation state is saved.
    #[serde(default)]
    #[builder(setter(into, strip_option))]
    pub save_dir: Option<PathBuf>,
    /// Application data directory.
    #[serde(default)]
    #[builder(setter(into, strip_option))]
    pub app_data_dir: Option<PathBuf>,
    /// Optional JSON schema for structured responses.
    #[serde(default)]
    #[builder(setter(strip_option))]
    pub response_schema: Option<JsonSchema>,
    /// Gemini model backend configuration.
    ///
    /// Controls per-model API keys, model selection per capability,
    /// and generation parameters such as `thinking_level`.
    ///
    /// Serializes as `"gemini_config"` to match the Python SDK field name.
    #[serde(default, rename = "gemini_config")]
    #[builder(setter(strip_option))]
    pub gemini: Option<GeminiConfig>,
    /// Maximum number of quota retry attempts before giving up.
    ///
    /// If `None`, defaults to 0 (no retries).
    #[serde(default)]
    #[builder(setter(into, strip_option))]
    pub max_quota_retries: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl AgentConfig {
    /// Resolve the effective API key using the same priority chain as the
    /// Python SDK's `LocalAgentConfig` → `_build_harness_config`:
    ///
    /// 1. Per-model key (`gemini.models.default.api_key`)
    /// 2. Shared `GeminiConfig` key (`gemini.api_key`)
    /// 3. Top-level shorthand (`api_key`)
    /// 4. `$GEMINI_API_KEY` environment variable
    #[must_use]
    pub fn effective_api_key(&self) -> Option<String> {
        self.gemini
            .as_ref()
            .and_then(|g| g.models.default.api_key.clone())
            .or_else(|| self.gemini.as_ref().and_then(|g| g.api_key.clone()))
            .or_else(|| self.api_key.clone())
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
    }

    /// Returns the names of all explicitly registered custom tools.
    /// To get the full list of tools including built-ins, combine this
    /// with `capabilities.enabled_tools` or examine `tools` + default semantics.
    #[must_use]
    pub fn custom_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }
}

// ─── LocalAgentConfig ────────────────────────────────────────────────────────

/// Configuration for a local (on-device) agent, mirroring the Python SDK's
/// `LocalAgentConfig`.
///
/// Wraps the standard [`AgentConfig`] via `#[serde(flatten)]`. The Python SDK's
/// `LocalAgentConfig` extends the base `AgentConfig` with override defaults
/// (e.g. `policies = confirm_run_command()`, `workspaces = [cwd]`), which
/// our `AgentConfig` already matches.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalAgentConfig {
    /// The base agent configuration.
    #[serde(flatten)]
    pub agent: AgentConfig,
}

impl LocalAgentConfig {
    /// Create a new `LocalAgentConfig` wrapping the given agent configuration.
    #[must_use]
    pub const fn new(agent: AgentConfig) -> Self {
        Self { agent }
    }
}

impl From<AgentConfig> for LocalAgentConfig {
    fn from(agent: AgentConfig) -> Self {
        Self::new(agent)
    }
}

/// Matches the Python SDK's `LocalAgentConfig` default: block `run_command`,
/// allow all other tools.
fn default_policies() -> Vec<PolicyRule> {
    vec![
        PolicyRule::Deny("run_command".to_string()),
        PolicyRule::AllowAll,
    ]
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyAnyMethods;

    use super::{
        super::{
            DEFAULT_IMAGE_GENERATION_MODEL,
            capabilities::BuiltinTools,
            models::{
                GenerationConfig, ModelConfig, ModelEntry, ThinkingLevel, default_image_model_entry,
            },
        },
        *,
    };

    #[derive(schemars::JsonSchema)]
    struct CustomToolParams {}

    #[test]
    fn test_roundtrip_serialization() {
        let config = AgentConfig {
            system_instructions: Some(SystemInstructions::Custom("Be helpful".to_string())),
            capabilities: Some(CapabilitiesConfig {
                enable_subagents: true,
                enabled_tools: Some(vec![BuiltinTools::ListDir]),
                compaction_threshold: Some(4000),
                ..CapabilitiesConfig::default()
            }),
            workspaces: vec![PathBuf::from("/tmp")],
            ..AgentConfig::default()
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspaces.len(), 1);
        assert_eq!(
            parsed.capabilities.unwrap().enabled_tools.unwrap()[0],
            BuiltinTools::ListDir
        );
    }

    #[test]
    fn agent_config_builder_with_gemini() {
        let gemini = GeminiConfig {
            api_key: Some("test-key".to_string()),
            base_url: None,
            models: ModelConfig::default(),
        };
        let config = AgentConfig::builder().gemini(gemini).build();
        let gemini_cfg = config.gemini.expect("gemini should be Some");
        assert_eq!(gemini_cfg.api_key.as_deref(), Some("test-key"));
        assert_eq!(gemini_cfg.models.default.name, DEFAULT_MODEL);
    }

    #[test]
    fn agent_config_builder_gemini_with_thinking_level() {
        let gemini = GeminiConfig {
            api_key: None,
            base_url: None,
            models: ModelConfig {
                default: ModelEntry {
                    name: "gemini-3.5-flash".to_string(),
                    api_key: None,
                    generation: GenerationConfig {
                        thinking_level: Some(ThinkingLevel::High),
                    },
                },
                image_generation: default_image_model_entry(),
            },
        };
        let config = AgentConfig::builder().gemini(gemini).build();
        let gemini_cfg = config.gemini.expect("gemini should be Some");
        assert_eq!(
            gemini_cfg.models.default.generation.thinking_level,
            Some(ThinkingLevel::High)
        );
        assert_eq!(gemini_cfg.models.default.name, "gemini-3.5-flash");
    }

    #[test]
    fn agent_config_gemini_none_by_default() {
        let config = AgentConfig::default();
        assert!(config.gemini.is_none());
    }

    #[test]
    fn agent_config_gemini_serde_roundtrip() {
        let config = AgentConfig {
            gemini: Some(GeminiConfig {
                api_key: Some("roundtrip-key".to_string()),
                base_url: None,
                models: ModelConfig {
                    default: ModelEntry {
                        name: "gemini-3.5-flash".to_string(),
                        api_key: Some("model-key".to_string()),
                        generation: GenerationConfig {
                            thinking_level: Some(ThinkingLevel::Medium),
                        },
                    },
                    image_generation: default_image_model_entry(),
                },
            }),
            ..AgentConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
        let gemini_cfg = parsed.gemini.expect("gemini should survive roundtrip");
        assert_eq!(gemini_cfg.api_key.as_deref(), Some("roundtrip-key"));
        assert_eq!(gemini_cfg.models.default.name, "gemini-3.5-flash");
        assert_eq!(
            gemini_cfg.models.default.api_key.as_deref(),
            Some("model-key")
        );
        assert_eq!(
            gemini_cfg.models.default.generation.thinking_level,
            Some(ThinkingLevel::Medium)
        );
    }

    #[test]
    fn system_instructions_custom_serde() {
        let instr = SystemInstructions::Custom("Be a helpful assistant".to_string());
        let json = serde_json::to_string(&instr).unwrap();
        let parsed: SystemInstructions = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemInstructions::Custom(text) => assert_eq!(text, "Be a helpful assistant"),
            SystemInstructions::Templated { .. } => {
                panic!("Expected Custom, got Templated")
            }
        }
    }

    #[test]
    fn system_instructions_templated_serde() {
        let instr = SystemInstructions::Templated {
            identity: Some("a security analyst".to_string()),
            sections: vec![SystemInstructionSection {
                content: "Always check permissions".to_string(),
                title: "security".to_string(),
            }],
        };
        let json = serde_json::to_string(&instr).unwrap();
        let parsed: SystemInstructions = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemInstructions::Templated { identity, sections } => {
                assert_eq!(identity.as_deref(), Some("a security analyst"));
                assert_eq!(sections.len(), 1);
                assert_eq!(sections[0].content, "Always check permissions");
            }
            SystemInstructions::Custom(_) => {
                panic!("Expected Templated, got Custom")
            }
        }
    }

    #[test]
    fn agent_config_fully_populated_serde() {
        let config = AgentConfig {
            system_instructions: Some(SystemInstructions::Templated {
                identity: Some("test-identity".to_string()),
                sections: vec![],
            }),
            capabilities: Some(CapabilitiesConfig {
                enable_subagents: true,
                disabled_tools: Some(vec![BuiltinTools::RunCommand]),
                compaction_threshold: Some(1000),
                ..CapabilitiesConfig::default()
            }),
            workspaces: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            tools: vec![crate::tools::ToolDefinition {
                name: "custom_tool".to_owned(),
                description: "A custom tool".to_owned(),
                parameter_schema: serde_json::to_value(schemars::schema_for!(CustomToolParams))
                    .unwrap(),
            }],
            policies: vec![PolicyRule::DenyAll],
            triggers: vec![TriggerEntry {
                name: "poll".to_owned(),
                config: crate::triggers::TriggerConfig::every_secs(30),
                message_template: "time to poll".to_owned(),
            }],
            hooks: vec![HookEntry {
                name: "pre_gate".to_owned(),
                point: crate::hooks::HookPoint::PreTurn,
                callback_id: "cb_pre".to_owned(),
            }],
            skills: vec![PathBuf::from("/skills/foo")],
            ..AgentConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspaces.len(), 2);
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.policies.len(), 1);
        assert_eq!(parsed.triggers.len(), 1);
        assert_eq!(parsed.hooks.len(), 1);
        assert_eq!(parsed.skills.len(), 1);
    }

    #[test]
    fn agent_config_empty_defaults_serde() {
        let json = r#"{"system_instructions":null}"#;
        let parsed: AgentConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.system_instructions.is_none());
        assert!(parsed.capabilities.is_none());
        assert!(parsed.workspaces.is_empty());
        assert!(parsed.tools.is_empty());
        assert_eq!(
            parsed.policies,
            vec![
                PolicyRule::Deny("run_command".to_string()),
                PolicyRule::AllowAll,
            ]
        );
        assert!(parsed.triggers.is_empty());
        assert!(parsed.hooks.is_empty());
        assert!(parsed.skills.is_empty());

        assert!(parsed.gemini.is_none());
    }

    #[test]
    fn agent_config_all_optional_fields_roundtrip() {
        let config = AgentConfig {
            workspaces: vec![PathBuf::from("/ws")],
            skills: vec![PathBuf::from("/skills/test")],

            conversation_id: Some("conv-123".to_string()),
            save_dir: Some(PathBuf::from("/save")),
            app_data_dir: Some(PathBuf::from("/app")),
            response_schema: Some(JsonSchema::new(serde_json::json!({"type": "object"}))),
            ..AgentConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspaces.len(), 1);
        assert_eq!(parsed.conversation_id.as_deref(), Some("conv-123"));
        assert_eq!(parsed.save_dir.as_ref().unwrap(), &PathBuf::from("/save"));
        assert!(parsed.response_schema.is_some());
    }

    #[test]
    fn agent_config_custom_tools_and_builtin_tools_coexist() {
        // Audit 3: Verify that an AgentConfig can carry both custom tool
        // definitions (for the ToolRegistry) AND SDK built-in tools
        // (via CapabilitiesConfig.enabled_tools) at the same time.
        let custom_tool = crate::tools::ToolDefinition {
            name: "my_custom_tool".to_owned(),
            description: "Does something custom".to_owned(),
            parameter_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        let config = AgentConfig {
            tools: vec![custom_tool],
            capabilities: Some(CapabilitiesConfig {
                enabled_tools: Some(vec![BuiltinTools::ViewFile, BuiltinTools::RunCommand]),
                ..CapabilitiesConfig::default()
            }),
            ..AgentConfig::default()
        };

        // Serialize and deserialize to prove the combined config survives a roundtrip.
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentConfig = serde_json::from_str(&json).unwrap();

        // Custom tools are preserved.
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.tools[0].name, "my_custom_tool");

        // Built-in tool selection is preserved.
        let caps = parsed.capabilities.as_ref().unwrap();
        let enabled = caps.enabled_tools.as_ref().unwrap();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.contains(&BuiltinTools::ViewFile));
        assert!(enabled.contains(&BuiltinTools::RunCommand));

        // Validate the config is internally consistent.
        assert!(caps.validate().is_ok());
    }

    #[test]
    fn agent_config_custom_tools_only_no_builtins() {
        // Verify custom_tools_only() + custom tools is valid.
        let config = AgentConfig {
            tools: vec![crate::tools::ToolDefinition {
                name: "fetch_data".to_owned(),
                description: "Fetches data".to_owned(),
                parameter_schema: serde_json::json!({"type": "object"}),
            }],
            capabilities: Some(CapabilitiesConfig::custom_tools_only()),
            ..AgentConfig::default()
        };

        let caps = config.capabilities.as_ref().unwrap();
        assert!(caps.enabled_tools.as_ref().unwrap().is_empty());
        assert!(caps.validate().is_ok());
        assert_eq!(config.tools.len(), 1);
    }

    // ── LocalAgentConfig tests ───────────────────────────────────────

    #[test]
    fn local_agent_config_default() {
        let config = LocalAgentConfig::default();
        assert_eq!(config.agent.model, DEFAULT_MODEL);
    }

    #[test]
    fn local_agent_config_from_agent_config() {
        let agent_cfg = AgentConfig {
            model: "gemini-3.5-flash".to_string(),
            ..AgentConfig::default()
        };
        let local: LocalAgentConfig = agent_cfg.into();
        assert_eq!(local.agent.model, "gemini-3.5-flash");
    }

    #[test]
    fn local_agent_config_serde_roundtrip() {
        let config = LocalAgentConfig::new(AgentConfig::default());
        let json = serde_json::to_string(&config).unwrap();
        let parsed: LocalAgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent.model, DEFAULT_MODEL);
    }

    // ── SDK field name alignment tests ────────────────────────────────
    //
    // Verify that serde serializes field names to match the Python SDK's
    // expected JSON keys.

    #[test]
    fn skills_serializes_as_skills_paths() {
        let config = AgentConfig::builder()
            .skills(vec![PathBuf::from("/skill/a.md")])
            .build();
        let json = serde_json::to_string(&config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("skills_paths").is_some(),
            "Expected JSON key 'skills_paths', got: {json}"
        );
        assert!(
            v.get("skills").is_none(),
            "Should not have 'skills' key in JSON"
        );
    }

    #[test]
    fn skills_paths_deserializes_to_skills_field() {
        let json = r#"{"skills_paths": ["/skill/a.md"]}"#;
        let config: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.skills.len(), 1);
        assert_eq!(config.skills[0], PathBuf::from("/skill/a.md"));
    }

    #[test]
    fn gemini_serializes_as_gemini_config() {
        let config = AgentConfig::builder()
            .gemini(super::super::GeminiConfig::default())
            .build();
        let json = serde_json::to_string(&config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("gemini_config").is_some(),
            "Expected JSON key 'gemini_config', got: {json}"
        );
        assert!(
            v.get("gemini").is_none(),
            "Should not have 'gemini' key in JSON"
        );
    }

    #[test]
    fn gemini_config_deserializes_to_gemini_field() {
        let json = r#"{"gemini_config": {"api_key": "test-key"}}"#;
        let config: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.gemini.as_ref().unwrap().api_key.as_deref(),
            Some("test-key")
        );
    }

    // ── skip_serializing_if tests ─────────────────────────────────────

    #[test]
    fn empty_vecs_omitted_from_json() {
        let config = AgentConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // These should all be absent when empty.
        for key in &[
            "workspaces",
            "tools",
            "triggers",
            "hooks",
            "skills_paths",
            "mcp_servers",
        ] {
            assert!(
                v.get(key).is_none(),
                "Empty vec field '{key}' should be omitted from JSON, got: {json}"
            );
        }
        // policies should always be present (non-empty default)
        assert!(
            v.get("policies").is_some(),
            "policies should always be serialized"
        );
    }

    #[test]
    fn populated_vecs_included_in_json() {
        let config = AgentConfig::builder()
            .skills(vec![PathBuf::from("/skill.md")])
            .workspaces(vec![PathBuf::from("/ws")])
            .build();
        let json = serde_json::to_string(&config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("skills_paths").is_some(),
            "Non-empty skills should be present"
        );
        assert!(
            v.get("workspaces").is_some(),
            "Non-empty workspaces should be present"
        );
    }

    // ── Default policy tests ──────────────────────────────────────────

    #[test]
    fn default_policies_deny_run_command_allow_rest() {
        let config = AgentConfig::default();
        assert_eq!(config.policies.len(), 2);
        assert_eq!(
            config.policies[0],
            PolicyRule::Deny("run_command".to_string())
        );
        assert_eq!(config.policies[1], PolicyRule::AllowAll);
    }

    // ── Python SDK mirror tests ────────────────────────────────────────
    //
    // These tests import the live Python SDK and verify that our Rust
    // constants haven't drifted from the canonical Python values.  They
    // require `pyo3::Python::initialize()` and a venv with the
    // SDK installed.

    /// Helper: extract a Python module-level attribute as a `String`.
    fn py_str_attr(module: &str, attr: &str) -> String {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::runtime::venv::configure_python_sys_path(py)
                .unwrap_or_else(|e| panic!("Failed to configure python sys.path: {e}"));
            let m = py
                .import(module)
                .unwrap_or_else(|e| panic!("Failed to import {module}: {e}"));
            m.getattr(attr)
                .unwrap_or_else(|e| panic!("Failed to get {module}.{attr}: {e}"))
                .extract::<String>()
                .unwrap_or_else(|e| panic!("Failed to extract {module}.{attr} as String: {e}"))
        })
    }

    #[test]
    fn default_model_matches_python_sdk() {
        let py_val = py_str_attr("google.antigravity.types", "DEFAULT_MODEL");
        assert_eq!(
            DEFAULT_MODEL, py_val,
            "Rust DEFAULT_MODEL ({DEFAULT_MODEL}) != Python SDK ({py_val})"
        );
    }

    #[test]
    fn default_image_model_matches_python_sdk() {
        let py_val = py_str_attr("google.antigravity.types", "DEFAULT_IMAGE_GENERATION_MODEL");
        assert_eq!(
            DEFAULT_IMAGE_GENERATION_MODEL, py_val,
            "Rust DEFAULT_IMAGE_GENERATION_MODEL ({DEFAULT_IMAGE_GENERATION_MODEL}) != Python SDK ({py_val})"
        );
    }

    // ── effective_api_key tests ──────────────────────────────────────

    #[test]
    fn effective_api_key_prefers_per_model_key() {
        let config = AgentConfig::builder()
            .api_key("top-level-key")
            .gemini(super::super::GeminiConfig {
                api_key: Some("shared-key".into()),
                base_url: None,
                models: super::super::ModelConfig {
                    default: super::super::ModelEntry {
                        name: "gemini-3.5-flash".into(),
                        api_key: Some("per-model-key".into()),
                        generation: super::super::GenerationConfig::default(),
                    },
                    image_generation: super::super::ModelEntry {
                        name: "imagen-4.0-generate-preview-06-03".into(),
                        api_key: None,
                        generation: super::super::GenerationConfig::default(),
                    },
                },
            })
            .build();
        assert_eq!(config.effective_api_key().as_deref(), Some("per-model-key"));
    }

    #[test]
    fn effective_api_key_falls_back_to_gemini_shared_key() {
        let config = AgentConfig::builder()
            .gemini(super::super::GeminiConfig {
                api_key: Some("shared-key".into()),
                ..Default::default()
            })
            .build();
        assert_eq!(config.effective_api_key().as_deref(), Some("shared-key"));
    }

    #[test]
    fn effective_api_key_falls_back_to_top_level() {
        let config = AgentConfig::builder().api_key("top-level-key").build();
        assert_eq!(config.effective_api_key().as_deref(), Some("top-level-key"));
    }

    #[test]
    fn effective_api_key_none_without_any_key() {
        // Build a config with no API key set at any level.
        // We can't safely manipulate env vars in multi-threaded tests,
        // so we test the chain up to the env-var fallback: if all config
        // keys are None and the env var isn't set, the result is None.
        // If GEMINI_API_KEY happens to be set, we verify it's returned.
        let config = AgentConfig::builder().build();
        let result = config.effective_api_key();
        match std::env::var("GEMINI_API_KEY").ok() {
            Some(env_key) => assert_eq!(result.as_deref(), Some(env_key.as_str())),
            None => assert!(result.is_none()),
        }
    }
}

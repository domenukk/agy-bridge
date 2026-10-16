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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Construct a `JsonSchema` directly from a type implementing [`schemars::JsonSchema`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::InvalidConfig`] if the generated schema cannot be serialized to JSON.
    pub fn from_schema<T: schemars::JsonSchema>() -> Result<Self, crate::error::Error> {
        let schema_root = schemars::schema_for!(T);
        let value =
            serde_json::to_value(schema_root).map_err(|e| crate::error::Error::InvalidConfig {
                message: format!("Failed to serialize JSON schema: {e}"),
            })?;
        Ok(Self(value))
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
    /// Inline skills provided directly in memory (mutually exclusive with `skills` on `LocalHarness`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default, setter(transform = |v: impl IntoIterator<Item = impl Into<super::subagents::InlineSkill>>| v.into_iter().map(Into::into).collect()))]
    pub inline_skills: Vec<super::subagents::InlineSkill>,

    /// MCP server configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(setter(transform = |v: impl IntoIterator<Item = impl Into<McpServer>>| v.into_iter().map(Into::into).collect()))]
    pub mcp_servers: Vec<McpServer>,
    /// Resume an existing conversation by its SDK id.
    ///
    /// Set this to an id previously returned by
    /// [`crate::agent::AgentHandle::conversation_id`]
    /// to resume that conversation. The SDK (local harness) uses it as the
    /// trajectory `cascade_id` to reload, so it **must** reference a conversation
    /// already persisted under [`Self::save_dir`]; passing an unknown id fails
    /// agent startup with "conversation not found".
    ///
    /// Leave unset to start a fresh conversation — the harness assigns a new id,
    /// which is then observable via
    /// [`crate::agent::AgentHandle::conversation_id`]
    /// and custom-tool [`crate::tools::ToolContext`].
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
    ///
    /// Omitted entirely when `None` rather than serialized as `null`: the
    /// runtime injects a resolved `api_key` / `base_url` into this object
    /// when the caller did not supply one, and a JSON `null` is neither a
    /// missing key (so `entry().or_insert_with()` would not replace it) nor
    /// an object (so `get_mut` would not match) — which silently defeated
    /// that injection.
    #[serde(
        default,
        rename = "gemini_config",
        skip_serializing_if = "Option::is_none"
    )]
    #[builder(setter(strip_option))]
    pub gemini: Option<GeminiConfig>,
    /// Optional initial conversation history to inject after agent creation.
    ///
    /// When set, these messages are inserted into the SDK's internal
    /// `_history` list before the first chat turn, enabling warm-start
    /// scenarios such as safety recovery (re-creating an agent with
    /// curated prior context).
    ///
    /// Serialized as `"initial_history"` and consumed by the Python init
    /// script. Skipped from serialization when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default, setter(transform = |v: impl IntoIterator<Item = impl Into<crate::types::ConversationMessage>>| v.into_iter().map(Into::into).collect()))]
    pub initial_history: Vec<crate::types::ConversationMessage>,
    /// Optional retry configuration for API calls and model outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub retry_config: Option<RetryConfig>,
    /// Optional execution budget limits (model calls, token ceilings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub budget_config: Option<super::budget::BudgetConfig>,
    /// Optional configuration for conversation trajectory compaction and context limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub compaction_config: Option<super::compaction::CompactionConfig>,
    /// Custom subagent definitions available to this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default, setter(transform = |v: impl IntoIterator<Item = impl Into<super::subagents::SubagentConfig>>| v.into_iter().map(Into::into).collect()))]
    pub subagents: Vec<super::subagents::SubagentConfig>,
    /// Session continuation strategy for conversations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub session_continuation_mode: Option<SessionContinuationMode>,
}

/// Controls whether an agent session resumes an existing conversation or starts fresh.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionContinuationMode {
    /// Resume an existing conversation trajectory. Fails if the conversation does not exist.
    Resume,
    /// Resume an existing conversation if found, or create a new one.
    #[default]
    CreateOrResume,
    /// Always create a new conversation trajectory. Fails if one already exists.
    CreateOnly,
}

impl SessionContinuationMode {
    /// Returns the string representation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::CreateOrResume => "create_or_resume",
            Self::CreateOnly => "create_only",
        }
    }

    /// Convert to protobuf numeric value for localharness.
    #[must_use]
    pub const fn to_proto_i32(self) -> i32 {
        match self {
            Self::Resume => 1,
            Self::CreateOrResume => 2,
            Self::CreateOnly => 3,
        }
    }

    /// Convert from protobuf numeric value.
    #[must_use]
    pub const fn from_proto_i32(val: i32) -> Option<Self> {
        match val {
            1 => Some(Self::Resume),
            2 => Some(Self::CreateOrResume),
            3 => Some(Self::CreateOnly),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionContinuationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SessionContinuationMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "resume" => Ok(Self::Resume),
            "create_or_resume" => Ok(Self::CreateOrResume),
            "create_only" => Ok(Self::CreateOnly),
            other => Err(format!("Unrecognized SessionContinuationMode: {other}")),
        }
    }
}

/// Configuration for API retry behavior with exponential backoff.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct ModelAPIRetryConfig {
    /// The maximum number of retries for transient API errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub max_retries: Option<u32>,
    /// The initial sleep duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub initial_sleep_duration_ms: Option<u32>,
    /// The multiplier for exponential backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub exponential_multiplier: Option<f64>,
    /// The range for jitter when calculating backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub jitter_range: Option<f64>,
}

/// Configuration for model output retry behavior.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct ModelOutputRetryConfig {
    /// The maximum number of retries for empty or invalid model outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub max_retries: Option<u32>,
}

/// Configuration for model and API retry behavior.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypedBuilder)]
pub struct RetryConfig {
    /// Retry configuration for backend API calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub api_retry: Option<ModelAPIRetryConfig>,
    /// Retry configuration for model outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub model_output_retry: Option<ModelOutputRetryConfig>,
}

impl RetryConfig {
    /// Create a retry configuration that disables retries (fails fast on first error).
    #[must_use]
    pub fn no_retries() -> Self {
        Self {
            api_retry: Some(ModelAPIRetryConfig {
                max_retries: Some(0),
                initial_sleep_duration_ms: None,
                exponential_multiplier: None,
                jitter_range: None,
            }),
            model_output_retry: Some(ModelOutputRetryConfig {
                max_retries: Some(0),
            }),
        }
    }

    /// Optimized for evaluation suites, automated benchmarks, and load testing.
    ///
    /// Uses unbounded retry tolerance (`u32::MAX` attempts, `1000ms` initial sleep)
    /// for transient API errors (429 rate limits, 503 service throttling) while
    /// relying on the default model output retry behavior.
    #[must_use]
    pub const fn benchmark() -> Self {
        Self {
            api_retry: Some(ModelAPIRetryConfig {
                max_retries: Some(u32::MAX),
                initial_sleep_duration_ms: Some(1000),
                exponential_multiplier: None,
                jitter_range: None,
            }),
            model_output_retry: None,
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl AgentConfig {
    /// Returns a copy of this configuration with lightweight presets applied
    /// (`BuiltinTools::minimal()`, `AgentBehavior::Minimal`, `enable_subagents = false`),
    /// preserving any caller-provided capability overrides.
    #[must_use]
    pub fn lightweight(mut self) -> Self {
        let caps = match self.capabilities.take() {
            None => super::CapabilitiesConfig::minimal(),
            Some(mut user_caps) => {
                if user_caps.enabled_tools.is_none() {
                    if let Some(disabled) = user_caps.disabled_tools.take() {
                        user_caps.enabled_tools = Some(
                            super::BuiltinTools::minimal()
                                .iter()
                                .copied()
                                .filter(|t| !disabled.contains(t))
                                .collect(),
                        );
                    } else {
                        user_caps.enabled_tools = Some(super::BuiltinTools::minimal().to_vec());
                    }
                }
                user_caps.agent_behavior = super::AgentBehavior::Minimal;
                user_caps.enable_subagents = false;
                user_caps
            }
        };
        self.capabilities = Some(caps);
        self
    }
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
            .or_else(|| crate::env_var(super::ENV_GEMINI_API_KEY))
    }

    /// Resolve the effective base URL using the priority chain:
    ///
    /// 1. Explicit `GeminiConfig` base URL (`gemini.base_url`)
    /// 2. `$GEMINI_API_BASE_URL` environment variable or `.env` fallback
    #[must_use]
    pub fn effective_base_url(&self) -> Option<String> {
        self.gemini
            .as_ref()
            .and_then(|g| g.base_url.clone())
            .or_else(|| crate::env_var(super::ENV_GEMINI_API_BASE_URL))
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
mod tests;

//! Harness, compaction, retry, and workspace configuration messages (`antigravity.localharness`).

use super::{
    AgentBehavior, CustomAgent, HarnessSideTools, LifecycleHook, McpServerConfig, ModelConfig,
    PolicyConfig, SystemInstructions, Tool, ToolOutputTruncation,
};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InputConfig {
    #[serde(
        alias = "storage_directory",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub storage_directory: String,
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub port: u32,
    #[serde(
        alias = "bind_address",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub bind_address: String,
    #[serde(
        alias = "client_info",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub client_info: Option<ClientInfo>,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(
        alias = "use_interactions_api",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub use_interactions_api: bool,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeConversationEvent {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub config: Option<HarnessConfig>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientInfo {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub language: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub version: String,
    #[serde(
        alias = "language_version",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub language_version: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub os: String,
    #[serde(
        alias = "os_version",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub os_version: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HarnessConfig {
    #[serde(
        alias = "cascade_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub cascade_id: String,
    #[serde(
        alias = "session_continuation_mode",
        with = "crate::proto::serde_helpers::ProtoEnumField::<harness_config::SessionContinuationMode>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub session_continuation_mode: i32,
    #[serde(
        alias = "system_instructions",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub system_instructions: Option<SystemInstructions>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<Tool>,
    #[serde(
        alias = "harness_side_tools",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub harness_side_tools: Option<HarnessSideTools>,
    #[serde(
        alias = "compaction_threshold",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub compaction_threshold: u32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub workspaces: Vec<Workspace>,
    #[serde(alias = "skills_paths", skip_serializing_if = "Vec::is_empty", default)]
    pub skills_paths: Vec<String>,
    #[serde(
        alias = "finish_tool_schema_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub finish_tool_schema_json: String,
    #[serde(
        alias = "initial_trajectory",
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub initial_trajectory: Vec<u8>,
    #[serde(
        alias = "app_data_dir",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub app_data_dir: String,
    #[serde(alias = "mcp_servers", skip_serializing_if = "Vec::is_empty", default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub models: Vec<ModelConfig>,
    #[serde(
        alias = "enabled_hooks",
        with = "crate::proto::serde_helpers::ProtoEnumVec::<LifecycleHook>",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub enabled_hooks: Vec<i32>,
    #[serde(
        alias = "custom_subagents",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub custom_subagents: Vec<CustomAgent>,
    #[serde(
        alias = "tool_output_truncation",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub tool_output_truncation: Option<ToolOutputTruncation>,
    #[serde(
        alias = "retry_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub retry_config: Option<RetryConfig>,
    #[serde(
        alias = "policy_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub policy_config: Option<PolicyConfig>,
    #[serde(
        alias = "agent_behavior",
        with = "crate::proto::serde_helpers::ProtoEnumField::<AgentBehavior>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub agent_behavior: i32,
    #[serde(
        alias = "budget_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub budget_config: Option<BudgetConfig>,
    #[serde(
        alias = "compaction_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub compaction_config: Option<CompactionConfig>,
    #[serde(
        alias = "skills_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub skills_config: Option<SkillsConfig>,
    #[serde(
        alias = "mcp_max_output_bytes",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mcp_max_output_bytes: i32,
}
/// Nested message and enum types in `HarnessConfig`.
pub mod harness_config {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum SessionContinuationMode {
        #[default]
        Unspecified = 0,
        Resume = 1,
        CreateOrResume = 2,
        CreateOnly = 3,
    }
    impl SessionContinuationMode {
        /// String value of the enum field names used in the Protobuf definition.
        ///
        /// The values are not transformed in any way and thus are considered stable
        /// (if the Protobuf definition does not change) and safe for programmatic use.
        #[must_use]
        pub fn as_str_name(&self) -> &'static str {
            match self {
                Self::Unspecified => "SESSION_CONTINUATION_MODE_UNSPECIFIED",
                Self::Resume => "RESUME",
                Self::CreateOrResume => "CREATE_OR_RESUME",
                Self::CreateOnly => "CREATE_ONLY",
            }
        }
        /// Creates an enum from field names used in the Protobuf definition.
        #[must_use]
        pub fn from_str_name(value: &str) -> Option<Self> {
            match value {
                "SESSION_CONTINUATION_MODE_UNSPECIFIED" => Some(Self::Unspecified),
                "RESUME" => Some(Self::Resume),
                "CREATE_OR_RESUME" => Some(Self::CreateOrResume),
                "CREATE_ONLY" => Some(Self::CreateOnly),
                _ => None,
            }
        }
    }
    impl TryFrom<i32> for SessionContinuationMode {
        type Error = i32;
        fn try_from(value: i32) -> std::result::Result<Self, i32> {
            match value {
                0 => Ok(Self::Unspecified),
                1 => Ok(Self::Resume),
                2 => Ok(Self::CreateOrResume),
                3 => Ok(Self::CreateOnly),
                other => Err(other),
            }
        }
    }
    impl crate::proto::serde_helpers::ProtoEnum for SessionContinuationMode {
        fn as_str_name(&self) -> &'static str {
            Self::as_str_name(self)
        }
        fn from_str_name(value: &str) -> Option<Self> {
            Self::from_str_name(value)
        }
        fn into_i32(self) -> i32 {
            self as i32
        }
    }
    impl serde::Serialize for SessionContinuationMode {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
        }
    }
    impl<'de> serde::Deserialize<'de> for SessionContinuationMode {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillSource {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub source: Option<skill_source::Source>,
}
/// Nested message and enum types in `SkillSource`.
pub mod skill_source {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Source {
        Skill(Box<super::super::super::genai::skills::Skill>),
        #[serde(alias = "directory_path")]
        DirectoryPath(String),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillsConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub skills: Vec<SkillSource>,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct BudgetConfig {
    #[serde(
        alias = "max_model_calls",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_model_calls: i32,
    #[serde(
        alias = "max_tool_calls",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_tool_calls: i32,
    #[serde(
        alias = "max_input_tokens",
        with = "crate::proto::serde_helpers::i64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_input_tokens: i64,
    #[serde(
        alias = "max_output_tokens",
        with = "crate::proto::serde_helpers::i64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_output_tokens: i64,
    #[serde(
        alias = "max_total_tokens",
        with = "crate::proto::serde_helpers::i64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_total_tokens: i64,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<budget_config::BudgetScope>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub scope: i32,
}
/// Nested message and enum types in `BudgetConfig`.
pub mod budget_config {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum BudgetScope {
            #[default]
            Unspecified = 0 => "BUDGET_SCOPE_UNSPECIFIED",
            Lifetime = 1 => "BUDGET_SCOPE_LIFETIME",
            ForwardLooking = 2 => "BUDGET_SCOPE_FORWARD_LOOKING",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompactionConfig {
    #[serde(
        alias = "token_threshold",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub token_threshold: u32,
    #[serde(
        alias = "checkpoint_interval_tokens",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub checkpoint_interval_tokens: u32,
    #[serde(
        alias = "max_context_tokens",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_context_tokens: u32,
    #[serde(
        alias = "summary_prompt_override",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub summary_prompt_override: String,
    #[serde(
        alias = "use_model_reported_token_count",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub use_model_reported_token_count: Option<bool>,
    #[serde(
        alias = "include_last_user_message",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub include_last_user_message: Option<bool>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelApiRetryConfig {
    #[serde(
        alias = "max_retries",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub max_retries: Option<u32>,
    #[serde(
        alias = "initial_sleep_duration_ms",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub initial_sleep_duration_ms: Option<u32>,
    #[serde(
        alias = "exponential_multiplier",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exponential_multiplier: Option<f64>,
    #[serde(
        alias = "jitter_range",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub jitter_range: Option<f64>,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelOutputRetryConfig {
    #[serde(
        alias = "max_retries",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub max_retries: Option<u32>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetryConfig {
    #[serde(alias = "api_retry", skip_serializing_if = "Option::is_none", default)]
    pub api_retry: Option<ModelApiRetryConfig>,
    #[serde(
        alias = "model_output_retry",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub model_output_retry: Option<ModelOutputRetryConfig>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    #[serde(
        alias = "workspace_type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub workspace_type: Option<workspace::WorkspaceType>,
}
/// Nested message and enum types in `Workspace`.
pub mod workspace {
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum WorkspaceType {
        #[serde(alias = "filesystem_workspace")]
        FilesystemWorkspace(super::FilesystemWorkspace),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FilesystemWorkspace {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub directory: String,
}

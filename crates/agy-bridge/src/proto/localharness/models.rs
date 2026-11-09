//! Model endpoints, system instructions, and capabilities configuration messages (`antigravity.localharness`).

use super::{ModelType, ToolExecutionMode};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GeminiModelOptions {
    #[serde(
        alias = "thinking_level",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub thinking_level: String,
    #[serde(
        alias = "service_tier",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub service_tier: String,
    #[serde(
        alias = "max_output_tokens",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_output_tokens: i32,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GeminiApiEndpoint {
    #[serde(alias = "base_url", skip_serializing_if = "String::is_empty", default)]
    pub base_url: String,
    #[serde(
        alias = "http_headers",
        skip_serializing_if = "std::collections::HashMap::is_empty",
        default
    )]
    pub http_headers: std::collections::HashMap<String, String>,
    #[serde(alias = "api_key", skip_serializing_if = "String::is_empty", default)]
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub options: Option<GeminiModelOptions>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VertexEndpoint {
    #[serde(alias = "base_url", skip_serializing_if = "String::is_empty", default)]
    pub base_url: String,
    #[serde(
        alias = "http_headers",
        skip_serializing_if = "std::collections::HashMap::is_empty",
        default
    )]
    pub http_headers: std::collections::HashMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub options: Option<GeminiModelOptions>,
    #[serde(alias = "api_key", skip_serializing_if = "String::is_empty", default)]
    pub api_key: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelConfig {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumVec::<ModelType>",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub types: Vec<i32>,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub endpoint: Option<model_config::Endpoint>,
}
/// Nested message and enum types in `ModelConfig`.
pub mod model_config {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Endpoint {
        #[serde(rename = "geminiApiEndpoint", alias = "gemini_api_endpoint")]
        GeminiApi(super::GeminiApiEndpoint),
        #[serde(rename = "vertexEndpoint", alias = "vertex_endpoint")]
        Vertex(super::VertexEndpoint),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemInstructions {
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<system_instructions::Type>,
}
/// Nested message and enum types in `SystemInstructions`.
pub mod system_instructions {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        Custom(super::CustomSystemInstructions),
        Appended(super::AppendedSystemInstructions),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomSystemInstructions {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub part: Vec<custom_system_instructions::Part>,
}
/// Nested message and enum types in `CustomSystemInstructions`.
pub mod custom_system_instructions {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Part {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub title: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
        pub part: Option<part::Part>,
    }
    /// Nested message and enum types in `Part`.
    pub mod part {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Part {
            Text(String),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppendedSystemInstructions {
    #[serde(
        alias = "custom_identity",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub custom_identity: String,
    #[serde(
        alias = "appended_sections",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub appended_sections: Vec<appended_system_instructions::Section>,
}
/// Nested message and enum types in `AppendedSystemInstructions`.
pub mod appended_system_instructions {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Section {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub title: String,
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub content: String,
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tool {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(
        alias = "parameters_json_schema",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub parameters_json_schema: String,
    #[serde(
        alias = "response_json_schema",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub response_json_schema: String,
    #[serde(
        alias = "defer_loading",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub defer_loading: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HarnessSideTools {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub find: Option<FindToolConfig>,
    #[serde(
        alias = "run_command",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub run_command: Option<RunCommandToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subagents: Option<SubagentsConfig>,
    #[serde(
        alias = "user_questions",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub user_questions: Option<UserQuestionsConfig>,
    #[serde(alias = "file_edit", skip_serializing_if = "Option::is_none", default)]
    pub file_edit: Option<FileEditToolConfig>,
    #[serde(alias = "view_file", skip_serializing_if = "Option::is_none", default)]
    pub view_file: Option<ViewFileToolConfig>,
    #[serde(
        alias = "write_to_file",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub write_to_file: Option<WriteToFileToolConfig>,
    #[serde(
        alias = "grep_search",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub grep_search: Option<GrepSearchToolConfig>,
    #[serde(alias = "list_dir", skip_serializing_if = "Option::is_none", default)]
    pub list_dir: Option<ListDirToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permissions: Option<PermissionsConfig>,
    #[serde(
        alias = "generate_image",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub generate_image: Option<GenerateImageToolConfig>,
    #[serde(alias = "search_web", skip_serializing_if = "Option::is_none", default)]
    pub search_web: Option<SearchWebToolConfig>,
    #[serde(
        alias = "read_url_content",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub read_url_content: Option<ReadUrlContentToolConfig>,
    #[serde(
        alias = "tool_search_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub tool_search_config: Option<ToolSearchConfig>,
    #[serde(
        alias = "manage_task",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub manage_task: Option<ManageTaskToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub schedule: Option<ScheduleToolConfig>,
    #[serde(
        alias = "run_workflow",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub run_workflow: Option<RunWorkflowToolConfig>,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct RunWorkflowToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
    #[serde(
        alias = "execution_mode",
        with = "crate::proto::serde_helpers::ProtoEnumField::<ToolExecutionMode>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub execution_mode: i32,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ManageTaskToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ScheduleToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FindToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct RunCommandToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
    #[serde(
        alias = "max_timeout_ms",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub max_timeout_ms: u32,
    #[serde(
        alias = "enable_daemon_commands",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enable_daemon_commands: bool,
    #[serde(
        alias = "enable_sandbox",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enable_sandbox: bool,
    #[serde(
        alias = "execution_mode",
        with = "crate::proto::serde_helpers::ProtoEnumField::<ToolExecutionMode>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub execution_mode: i32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentsConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
    #[serde(
        alias = "max_nesting_depth",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub max_nesting_depth: Option<i32>,
    #[serde(
        alias = "allowed_subagents",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub allowed_subagents: Vec<String>,
    #[serde(
        alias = "disable_define_subagent_tool",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub disable_define_subagent_tool: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct UserQuestionsConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FileEditToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewFileToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct WriteToFileToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct GrepSearchToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ListDirToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateImageToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchWebToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadUrlContentToolConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolSearchConfig {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enabled: bool,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsConfig {
    #[serde(
        alias = "enforce_workspace_validation",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub enforce_workspace_validation: bool,
}

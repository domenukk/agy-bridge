//! Lifecycle hooks, policy evaluation, and subagent configuration messages (`antigravity.localharness`).

use super::{
    AgentBehavior, HarnessSideTools, LifecycleHook, ModelConfig, PolicyDecision,
    PolicyEvaluationOutcome, SkillSource, SystemInstructions, Tool, UserInput,
};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallHookRequest {
    #[serde(
        alias = "request_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub request_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        rename = "type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<LifecycleHook>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub r#type: i32,
    #[serde(alias = "client_id", skip_serializing_if = "String::is_empty", default)]
    pub client_id: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub args: Option<call_hook_request::Args>,
}
/// Nested message and enum types in `CallHookRequest`.
pub mod call_hook_request {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Args {
        #[serde(rename = "preTurnArgs", alias = "pre_turn_args")]
        PreTurn(super::PreTurnArgs),
        #[serde(rename = "postTurnArgs", alias = "post_turn_args")]
        PostTurn(super::PostTurnArgs),
        #[serde(rename = "preToolArgs", alias = "pre_tool_args")]
        PreTool(super::PreToolArgs),
        #[serde(rename = "postToolArgs", alias = "post_tool_args")]
        PostTool(super::PostToolArgs),
        #[serde(rename = "onToolErrorArgs", alias = "on_tool_error_args")]
        OnToolError(super::OnToolErrorArgs),
        #[serde(rename = "onCompactionArgs", alias = "on_compaction_args")]
        OnCompaction(super::OnCompactionArgs),
        #[serde(rename = "stopArgs", alias = "stop_args")]
        Stop(super::StopArgs),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallHookResponse {
    #[serde(
        alias = "request_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub request_id: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub result: Option<call_hook_response::Result>,
}
/// Nested message and enum types in `CallHookResponse`.
pub mod call_hook_response {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Result {
        #[serde(rename = "preTurnResult", alias = "pre_turn_result")]
        PreTurn(super::PreTurnResult),
        #[serde(rename = "preToolResult", alias = "pre_tool_result")]
        PreTool(super::PreToolResult),
        #[serde(rename = "emptyResult", alias = "empty_result")]
        Empty(super::EmptyResult),
        #[serde(alias = "error_message")]
        ErrorMessage(String),
        #[serde(rename = "onToolErrorResult", alias = "on_tool_error_result")]
        OnToolError(super::OnToolErrorResult),
        #[serde(rename = "stopResult", alias = "stop_result")]
        Stop(super::StopResult),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreToolArgs {
    #[serde(alias = "tool_name", skip_serializing_if = "String::is_empty", default)]
    pub tool_name: String,
    #[serde(
        alias = "arguments_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub arguments_json: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: i32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PostToolArgs {
    #[serde(alias = "tool_name", skip_serializing_if = "String::is_empty", default)]
    pub tool_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub result: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: i32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnToolErrorArgs {
    #[serde(alias = "tool_name", skip_serializing_if = "String::is_empty", default)]
    pub tool_name: String,
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: i32,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreTurnArgs {
    #[serde(alias = "user_input", skip_serializing_if = "Option::is_none", default)]
    pub user_input: Option<UserInput>,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PostTurnArgs {
    #[serde(
        alias = "response_text",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub response_text: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnCompactionArgs {
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StopArgs {
    #[serde(
        alias = "response_text",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub response_text: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "continuation_count",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub continuation_count: i32,
    #[serde(
        alias = "stop_reason",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub stop_reason: String,
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StopResult {
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub decision: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct EmptyResult {}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnToolErrorResult {
    #[serde(
        alias = "custom_error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub custom_error_message: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreToolResult {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<pre_tool_result::Decision>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub decision: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
    #[serde(
        alias = "modified_arguments_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub modified_arguments_json: String,
    #[serde(
        alias = "modified_args",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub modified_args: Option<super::super::genai::Struct>,
}
/// Nested message and enum types in `PreToolResult`.
pub mod pre_tool_result {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Decision {
            #[default]
            Unspecified = 0 => "DECISION_UNSPECIFIED",
            Allow = 1 => "ALLOW",
            Deny = 2 => "DENY",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreTurnResult {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<pre_turn_result::Decision>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub decision: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
}
/// Nested message and enum types in `PreTurnResult`.
pub mod pre_turn_result {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Decision {
            #[default]
            Unspecified = 0 => "DECISION_UNSPECIFIED",
            Allow = 1 => "ALLOW",
            Deny = 2 => "DENY",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentSkillsConfig {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub mode: Option<subagent_skills_config::Mode>,
}
/// Nested message and enum types in `SubagentSkillsConfig`.
pub mod subagent_skills_config {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct InheritConfig {
        #[serde(
            alias = "extra_skills_paths",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub extra_skills_paths: Vec<String>,
        #[serde(alias = "skill_names", skip_serializing_if = "Vec::is_empty", default)]
        pub skill_names: Vec<String>,
    }
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
    )]
    #[serde(rename_all = "camelCase", default)]
    pub struct NoneConfig {}
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct OverrideConfig {
        #[serde(alias = "skills_paths", skip_serializing_if = "Vec::is_empty", default)]
        pub skills_paths: Vec<String>,
        #[serde(
            alias = "skill_sources",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub skill_sources: Vec<super::SkillSource>,
    }
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Mode {
        #[serde(rename = "inheritConfig", alias = "inherit_config")]
        Inherit(InheritConfig),
        #[serde(rename = "noneConfig", alias = "none_config")]
        None(NoneConfig),
        #[serde(rename = "overrideConfig", alias = "override_config")]
        Override(OverrideConfig),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomAgent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(
        alias = "system_instructions",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub system_instructions: Option<SystemInstructions>,
    #[serde(
        alias = "harness_side_tools",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub harness_side_tools: Option<HarnessSideTools>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<Tool>,
    #[serde(
        alias = "skills_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub skills_config: Option<SubagentSkillsConfig>,
    #[serde(
        alias = "agent_behavior",
        with = "crate::proto::serde_helpers::ProtoEnumField::<AgentBehavior>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub agent_behavior: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<ModelConfig>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolOutputTruncation {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub strategy: Option<tool_output_truncation::Strategy>,
}
/// Nested message and enum types in `ToolOutputTruncation`.
pub mod tool_output_truncation {
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
    )]
    #[serde(rename_all = "camelCase", default)]
    pub struct TruncateStrategy {
        #[serde(
            alias = "max_tokens",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub max_tokens: i32,
        #[serde(
            alias = "save_full_output",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub save_full_output: bool,
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ErrorStrategy {
        #[serde(
            alias = "max_tokens",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub max_tokens: i32,
        #[serde(
            alias = "error_message",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub error_message: String,
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Strategy {
        Truncate(TruncateStrategy),
        Error(ErrorStrategy),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyConfig {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub rules: Vec<PolicyRule>,
    #[serde(
        alias = "workspace_containment",
        with = "crate::proto::serde_helpers::ProtoEnumField::<policy_config::WorkspaceContainment>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub workspace_containment: i32,
    #[serde(
        alias = "auto_config",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auto_config: Option<AutoPolicyConfig>,
}
/// Nested message and enum types in `PolicyConfig`.
pub mod policy_config {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum WorkspaceContainment {
            #[default]
            Unspecified = 0 => "WORKSPACE_CONTAINMENT_UNSPECIFIED",
            Enabled = 1 => "WORKSPACE_CONTAINMENT_ENABLED",
            Disabled = 2 => "WORKSPACE_CONTAINMENT_DISABLED",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoPolicyConfig {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub prompt: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyRule {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tool: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<PolicyDecision>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub decision: i32,
    #[serde(
        alias = "deny_reason",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub deny_reason: String,
    #[serde(
        alias = "is_dynamic",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_dynamic: bool,
    #[serde(alias = "rule_id", skip_serializing_if = "String::is_empty", default)]
    pub rule_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyDecisionRequest {
    #[serde(
        alias = "request_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub request_id: String,
    #[serde(alias = "rule_id", skip_serializing_if = "String::is_empty", default)]
    pub rule_id: String,
    #[serde(alias = "tool_args", skip_serializing_if = "Option::is_none", default)]
    pub tool_args: Option<PreToolArgs>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyDecisionResponse {
    #[serde(
        alias = "request_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub request_id: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<PolicyEvaluationOutcome>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub outcome: i32,
    #[serde(
        alias = "deny_reason",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub deny_reason: String,
}

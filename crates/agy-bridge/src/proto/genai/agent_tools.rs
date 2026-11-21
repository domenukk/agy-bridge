//! Agent orchestration tool call and result messages (`genai`, `agent_tools.proto`).

use super::Content;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentToolCallStep {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub call: Option<agent_tool_call_step::Call>,
}
/// Nested message and enum types in `AgentToolCallStep`.
pub mod agent_tool_call_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Call {
        #[serde(alias = "invoke_subagent")]
        InvokeSubagent(super::InvokeSubagentCall),
        #[serde(alias = "send_message")]
        SendMessage(super::SendMessageCall),
        #[serde(alias = "manage_subagents")]
        ManageSubagents(super::ManageSubagentsCall),
        #[serde(alias = "define_subagent")]
        DefineSubagent(super::DefineSubagentCall),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentToolResultStep {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub result: Option<agent_tool_result_step::Result>,
}
/// Nested message and enum types in `AgentToolResultStep`.
pub mod agent_tool_result_step {
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Result {
        #[serde(alias = "invoke_subagent")]
        InvokeSubagent(super::InvokeSubagentResult),
        #[serde(alias = "send_message")]
        SendMessage(super::SendMessageResult),
        #[serde(alias = "manage_subagents")]
        ManageSubagents(super::ManageSubagentsResult),
        #[serde(alias = "define_subagent")]
        DefineSubagent(super::DefineSubagentResult),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InvokeSubagentCall {
    #[serde(alias = "type_name", skip_serializing_if = "String::is_empty", default)]
    pub type_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub role: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub prompt: Vec<Content>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InvokeSubagentResult {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub subagents: Vec<invoke_subagent_result::Subagent>,
}
/// Nested message and enum types in `InvokeSubagentResult`.
pub mod invoke_subagent_result {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Subagent {
        #[serde(
            alias = "interaction_id",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub interaction_id: String,
        #[serde(alias = "type_name", skip_serializing_if = "String::is_empty", default)]
        pub type_name: String,
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub role: String,
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendMessageCall {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub recipient: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub message: Vec<Content>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct SendMessageResult {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManageSubagentsCall {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<manage_subagents_call::Action>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub action: i32,
    #[serde(
        alias = "interaction_ids",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub interaction_ids: Vec<String>,
}
/// Nested message and enum types in `ManageSubagentsCall`.
pub mod manage_subagents_call {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Action {
            #[default]
            Unspecified = 0 => "ACTION_UNSPECIFIED",
            List = 1 => "ACTION_LIST",
            Kill = 2 => "ACTION_KILL",
            KillAll = 3 => "ACTION_KILL_ALL",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManageSubagentsResult {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub subagents: Vec<ManagedSubagent>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManagedSubagent {
    #[serde(
        alias = "interaction_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub interaction_id: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<managed_subagent::Status>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub status: i32,
}
/// Nested message and enum types in `ManagedSubagent`.
pub mod managed_subagent {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Status {
            #[default]
            Unspecified = 0 => "STATUS_UNSPECIFIED",
            InProgress = 1 => "STATUS_IN_PROGRESS",
            Idle = 2 => "STATUS_IDLE",
            RequiresAction = 3 => "STATUS_REQUIRES_ACTION",
            Failed = 4 => "STATUS_FAILED",
            Cancelled = 5 => "STATUS_CANCELLED",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefineSubagentCall {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(
        alias = "system_prompt",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub system_prompt: String,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct DefineSubagentResult {}

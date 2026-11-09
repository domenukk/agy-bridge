//! Step update and builtin action messages (`antigravity.localharness`).

use super::{
    ActionCustomTool, ActionMcpTool, CallHookRequest, PolicyDecisionRequest, ToolCall,
    TrajectoryStateUpdate, TrajectoryUsageEntry, UsageMetadata, UsageUpdate,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputConfig {
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub port: i32,
    #[serde(alias = "api_key", skip_serializing_if = "String::is_empty", default)]
    pub api_key: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputEvent {
    #[serde(
        alias = "seq_num",
        with = "crate::proto::serde_helpers::i64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub seq_num: i64,
    #[serde(
        alias = "timestamp_micros",
        with = "crate::proto::serde_helpers::i64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub timestamp_micros: i64,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub event: Option<output_event::Event>,
}
/// Nested message and enum types in `OutputEvent`.
pub mod output_event {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Event {
        #[serde(alias = "step_update")]
        StepUpdate(Box<super::StepUpdate>),
        #[serde(alias = "trajectory_state_update")]
        TrajectoryStateUpdate(super::TrajectoryStateUpdate),
        #[serde(alias = "tool_call")]
        ToolCall(super::ToolCall),
        #[serde(alias = "initialize_conversation_response")]
        InitializeConversationResponse(super::InitializeConversationResponse),
        #[serde(alias = "call_hook_request")]
        CallHookRequest(super::CallHookRequest),
        #[serde(alias = "session_end_response")]
        SessionEndResponse(bool),
        #[serde(alias = "policy_decision_request")]
        PolicyDecisionRequest(super::PolicyDecisionRequest),
        #[serde(alias = "usage_update")]
        UsageUpdate(super::UsageUpdate),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SandboxStatus {
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub available: bool,
    #[serde(
        alias = "unavailable_reason",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub unavailable_reason: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeConversationResponse {
    #[serde(
        alias = "cascade_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub cascade_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub history: Vec<StepUpdate>,
    #[serde(
        alias = "cumulative_usage",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub cumulative_usage: Option<UsageMetadata>,
    #[serde(
        alias = "trajectory_usage",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub trajectory_usage: Vec<TrajectoryUsageEntry>,
    #[serde(
        alias = "sandbox_status",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub sandbox_status: Option<SandboxStatus>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepUpdate {
    #[serde(
        alias = "cascade_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub cascade_id: String,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "parent_trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub parent_trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: u32,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<step_update::State>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub state: i32,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<step_update::Source>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub source: i32,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<step_update::Target>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub target: i32,
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub thinking: String,
    #[serde(
        alias = "text_delta",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub text_delta: String,
    #[serde(
        alias = "thinking_delta",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub thinking_delta: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(
        alias = "list_directory",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub list_directory: Option<ActionListDirectory>,
    #[serde(alias = "find_file", skip_serializing_if = "Option::is_none", default)]
    pub find_file: Option<ActionFindFile>,
    #[serde(
        alias = "search_directory",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub search_directory: Option<ActionSearchDirectory>,
    #[serde(alias = "view_file", skip_serializing_if = "Option::is_none", default)]
    pub view_file: Option<ActionViewFile>,
    #[serde(
        alias = "create_file",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub create_file: Option<ActionCreateFile>,
    #[serde(alias = "edit_file", skip_serializing_if = "Option::is_none", default)]
    pub edit_file: Option<ActionEditFile>,
    #[serde(
        alias = "run_command",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub run_command: Option<ActionRunCommand>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compaction: Option<ActionCompaction>,
    #[serde(
        alias = "invoke_subagent",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub invoke_subagent: Option<ActionInvokeSubagent>,
    #[serde(
        alias = "generate_image",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub generate_image: Option<ActionGenerateImage>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<ActionFinish>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<ActionError>,
    #[serde(alias = "mcp_tool", skip_serializing_if = "Option::is_none", default)]
    pub mcp_tool: Option<ActionMcpTool>,
    #[serde(alias = "search_web", skip_serializing_if = "Option::is_none", default)]
    pub search_web: Option<ActionSearchWeb>,
    #[serde(
        alias = "read_url_content",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub read_url_content: Option<ActionReadUrlContent>,
    #[serde(
        alias = "custom_tool",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub custom_tool: Option<ActionCustomTool>,
    #[serde(
        alias = "skill_lookup",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub skill_lookup: Option<ActionSkillLookup>,
    #[serde(
        alias = "run_workflow",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub run_workflow: Option<ActionRunWorkflow>,
    #[serde(
        alias = "request_text",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub request_text: String,
    #[serde(
        alias = "tool_confirmation_request",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub tool_confirmation_request: Option<ToolConfirmationRequest>,
    #[serde(
        alias = "questions_request",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub questions_request: Option<UserQuestionsRequest>,
}
/// Nested message and enum types in `StepUpdate`.
pub mod step_update {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum State {
            #[default]
            Unspecified = 0 => "STATE_UNSPECIFIED",
            Active = 1 => "STATE_ACTIVE",
            Done = 2 => "STATE_DONE",
            WaitingForUser = 3 => "STATE_WAITING_FOR_USER",
            Error = 4 => "STATE_ERROR",
        }
    }

    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Source {
            #[default]
            Unspecified = 0 => "SOURCE_UNSPECIFIED",
            System = 1 => "SOURCE_SYSTEM",
            User = 2 => "SOURCE_USER",
            Model = 3 => "SOURCE_MODEL",
        }
    }

    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Target {
            #[default]
            Unspecified = 0 => "TARGET_UNSPECIFIED",
            User = 1 => "TARGET_USER",
            Model = 2 => "TARGET_MODEL",
            Environment = 3 => "TARGET_ENVIRONMENT",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionSkillLookup {
    #[serde(
        alias = "skill_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub skill_name: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionRunWorkflow {
    #[serde(
        alias = "script_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub script_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub script: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub output: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionGenerateImage {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub prompt: String,
    #[serde(alias = "image_paths", skip_serializing_if = "Vec::is_empty", default)]
    pub image_paths: Vec<String>,
    #[serde(
        alias = "image_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub image_name: String,
    #[serde(
        alias = "aspect_ratio",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub aspect_ratio: String,
    #[serde(
        alias = "output_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub output_path: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionSearchWeb {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub query: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub domain: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionReadUrlContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
    #[serde(
        alias = "content_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub content_path: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionFinish {
    #[serde(
        alias = "output_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub output_string: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionError {
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
    #[serde(
        alias = "http_code",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub http_code: u32,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionListDirectory {
    #[serde(
        alias = "directory_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub directory_path: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub results: Vec<action_list_directory::Result>,
}
/// Nested message and enum types in `ActionListDirectory`.
pub mod action_list_directory {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Result {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub name: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
        pub info: Option<result::Info>,
    }
    /// Nested message and enum types in `Result`.
    pub mod result {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Info {
            #[serde(alias = "is_directory")]
            IsDirectory(bool),
            #[serde(alias = "file_size")]
            FileSize(#[serde(with = "crate::proto::serde_helpers::u64_str")] u64),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionFindFile {
    #[serde(
        alias = "directory_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub directory_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub query: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub output: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionSearchDirectory {
    #[serde(
        alias = "directory_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub directory_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub query: String,
    #[serde(
        alias = "num_results",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub num_results: i32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionViewFile {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
    #[serde(
        alias = "start_line",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub start_line: u32,
    #[serde(
        alias = "end_line",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub end_line: u32,
    #[serde(
        alias = "content_offset",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub content_offset: i32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionCreateFile {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub contents: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionEditFile {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
    #[serde(alias = "diff_block", skip_serializing_if = "Vec::is_empty", default)]
    pub diff_block: Vec<action_edit_file::DiffBlock>,
}
/// Nested message and enum types in `ActionEditFile`.
pub mod action_edit_file {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct DiffLine {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub text: String,
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<diff_line::LineAction>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub action: i32,
    }
    /// Nested message and enum types in `DiffLine`.
    pub mod diff_line {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(i32)]
        pub enum LineAction {
            #[default]
            Unspecified = 0,
            Insert = 1,
            Delete = 2,
            None = 3,
        }
        impl LineAction {
            /// String value of the enum field names used in the Protobuf definition.
            ///
            /// The values are not transformed in any way and thus are considered stable
            /// (if the Protobuf definition does not change) and safe for programmatic use.
            #[must_use]
            pub fn as_str_name(&self) -> &'static str {
                match self {
                    Self::Unspecified => "LINE_ACTION_UNSPECIFIED",
                    Self::Insert => "LINE_ACTION_INSERT",
                    Self::Delete => "LINE_ACTION_DELETE",
                    Self::None => "LINE_ACTION_NONE",
                }
            }
            /// Creates an enum from field names used in the Protobuf definition.
            #[must_use]
            pub fn from_str_name(value: &str) -> Option<Self> {
                match value {
                    "LINE_ACTION_UNSPECIFIED" => Some(Self::Unspecified),
                    "LINE_ACTION_INSERT" => Some(Self::Insert),
                    "LINE_ACTION_DELETE" => Some(Self::Delete),
                    "LINE_ACTION_NONE" => Some(Self::None),
                    _ => None,
                }
            }
        }
        impl TryFrom<i32> for LineAction {
            type Error = i32;
            fn try_from(value: i32) -> std::result::Result<Self, i32> {
                match value {
                    0 => Ok(Self::Unspecified),
                    1 => Ok(Self::Insert),
                    2 => Ok(Self::Delete),
                    3 => Ok(Self::None),
                    other => Err(other),
                }
            }
        }
        impl crate::proto::serde_helpers::ProtoEnum for LineAction {
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
        impl serde::Serialize for LineAction {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
            }
        }
        impl<'de> serde::Deserialize<'de> for LineAction {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
            }
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct DiffBlock {
        #[serde(
            alias = "start_line",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub start_line: i32,
        #[serde(
            alias = "end_line",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub end_line: i32,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub lines: Vec<DiffLine>,
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionRunCommand {
    #[serde(
        alias = "command_line",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub command_line: String,
    #[serde(
        alias = "working_dir",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub working_dir: String,
    #[serde(
        alias = "exit_code",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub exit_code: i32,
    #[serde(
        alias = "combined_output",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub combined_output: String,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionCompaction {}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionInvokeSubagent {}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolConfirmationRequest {}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserQuestionsRequest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub questions: Vec<UserQuestion>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserQuestion {
    #[serde(
        alias = "question_type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub question_type: Option<user_question::QuestionType>,
}
/// Nested message and enum types in `UserQuestion`.
pub mod user_question {
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum QuestionType {
        #[serde(alias = "multiple_choice")]
        MultipleChoice(super::MultipleChoice),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MultipleChoice {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub question: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub choices: Vec<String>,
    #[serde(
        alias = "is_multi_select",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_multi_select: bool,
}

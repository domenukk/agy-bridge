//! Conversation input/output events, MCP transport, and usage metadata (`antigravity.localharness`).

use super::{CallHookResponse, Modality, PolicyDecisionResponse};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InputEvent {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub event: Option<input_event::Event>,
}
/// Nested message and enum types in `InputEvent`.
pub mod input_event {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Event {
        #[serde(alias = "user_input")]
        UserInput(super::UserInput),
        #[serde(alias = "tool_confirmation")]
        ToolConfirmation(super::ToolConfirmation),
        #[serde(alias = "tool_response")]
        ToolResponse(super::ToolResponse),
        #[serde(alias = "question_response")]
        QuestionResponse(super::UserQuestionsResponse),
        #[serde(alias = "halt_request")]
        HaltRequest(bool),
        #[serde(alias = "automated_trigger")]
        AutomatedTrigger(String),
        #[serde(alias = "call_hook_response")]
        CallHookResponse(super::CallHookResponse),
        #[serde(alias = "session_end_request")]
        SessionEndRequest(bool),
        #[serde(alias = "policy_decision_response")]
        PolicyDecisionResponse(super::PolicyDecisionResponse),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserInput {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub parts: Vec<user_input::Part>,
}
/// Nested message and enum types in `UserInput`.
pub mod user_input {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Media {
        #[serde(alias = "mime_type", skip_serializing_if = "String::is_empty", default)]
        pub mime_type: String,
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub description: String,
        #[serde(
            with = "crate::proto::serde_helpers::base64_bytes",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub data: Vec<u8>,
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct SlashCommand {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub name: String,
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Part {
        #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
        pub part: Option<part::Part>,
    }
    /// Nested message and enum types in `Part`.
    pub mod part {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Part {
            Text(String),
            Media(super::Media),
            #[serde(alias = "slash_command")]
            SlashCommand(super::SlashCommand),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolConfirmation {
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: u32,
    #[serde(
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub accepted: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrajectoryStateUpdate {
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
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub depth: i32,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<trajectory_state_update::State>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub state: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    #[serde(
        alias = "error_code",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub error_code: u32,
    #[serde(
        alias = "stop_reason",
        with = "crate::proto::serde_helpers::ProtoEnumField::<trajectory_state_update::StopReason>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub stop_reason: i32,
}
/// Nested message and enum types in `TrajectoryStateUpdate`.
pub mod trajectory_state_update {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum State {
        #[default]
        Unspecified = 0,
        Running = 1,
        FullyIdle = 2,
        Cancelled = 3,
        WaitingForTasks = 4,
        WaitingForSubagents = 5,
    }
    impl State {
        /// String value of the enum field names used in the Protobuf definition.
        ///
        /// The values are not transformed in any way and thus are considered stable
        /// (if the Protobuf definition does not change) and safe for programmatic use.
        #[must_use]
        pub fn as_str_name(&self) -> &'static str {
            match self {
                Self::Unspecified => "STATE_UNSPECIFIED",
                Self::Running => "STATE_RUNNING",
                Self::FullyIdle => "STATE_FULLY_IDLE",
                Self::Cancelled => "STATE_CANCELLED",
                Self::WaitingForTasks => "STATE_WAITING_FOR_TASKS",
                Self::WaitingForSubagents => "STATE_WAITING_FOR_SUBAGENTS",
            }
        }
        /// Creates an enum from field names used in the Protobuf definition.
        #[must_use]
        pub fn from_str_name(value: &str) -> Option<Self> {
            match value {
                "STATE_UNSPECIFIED" => Some(Self::Unspecified),
                "STATE_RUNNING" => Some(Self::Running),
                "STATE_FULLY_IDLE" => Some(Self::FullyIdle),
                "STATE_CANCELLED" => Some(Self::Cancelled),
                "STATE_WAITING_FOR_TASKS" => Some(Self::WaitingForTasks),
                "STATE_WAITING_FOR_SUBAGENTS" => Some(Self::WaitingForSubagents),
                _ => None,
            }
        }
    }
    impl TryFrom<i32> for State {
        type Error = i32;
        fn try_from(value: i32) -> std::result::Result<Self, i32> {
            match value {
                0 => Ok(Self::Unspecified),
                1 => Ok(Self::Running),
                2 => Ok(Self::FullyIdle),
                3 => Ok(Self::Cancelled),
                4 => Ok(Self::WaitingForTasks),
                5 => Ok(Self::WaitingForSubagents),
                other => Err(other),
            }
        }
    }
    impl crate::proto::serde_helpers::ProtoEnum for State {
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
    impl serde::Serialize for State {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
        }
    }
    impl<'de> serde::Deserialize<'de> for State {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
        }
    }
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum StopReason {
        #[default]
        Unspecified = 0,
        MaxModelCallsExceeded = 1,
        MaxToolCallsExceeded = 2,
        MaxInputTokensExceeded = 3,
        MaxOutputTokensExceeded = 4,
        MaxTotalTokensExceeded = 5,
        QuotaExhausted = 6,
    }
    impl StopReason {
        /// String value of the enum field names used in the Protobuf definition.
        ///
        /// The values are not transformed in any way and thus are considered stable
        /// (if the Protobuf definition does not change) and safe for programmatic use.
        #[must_use]
        pub fn as_str_name(&self) -> &'static str {
            match self {
                Self::Unspecified => "STOP_REASON_UNSPECIFIED",
                Self::MaxModelCallsExceeded => "STOP_REASON_MAX_MODEL_CALLS_EXCEEDED",
                Self::MaxToolCallsExceeded => "STOP_REASON_MAX_TOOL_CALLS_EXCEEDED",
                Self::MaxInputTokensExceeded => "STOP_REASON_MAX_INPUT_TOKENS_EXCEEDED",
                Self::MaxOutputTokensExceeded => "STOP_REASON_MAX_OUTPUT_TOKENS_EXCEEDED",
                Self::MaxTotalTokensExceeded => "STOP_REASON_MAX_TOTAL_TOKENS_EXCEEDED",
                Self::QuotaExhausted => "STOP_REASON_QUOTA_EXHAUSTED",
            }
        }
        /// Creates an enum from field names used in the Protobuf definition.
        #[must_use]
        pub fn from_str_name(value: &str) -> Option<Self> {
            match value {
                "STOP_REASON_UNSPECIFIED" => Some(Self::Unspecified),
                "STOP_REASON_MAX_MODEL_CALLS_EXCEEDED" => Some(Self::MaxModelCallsExceeded),
                "STOP_REASON_MAX_TOOL_CALLS_EXCEEDED" => Some(Self::MaxToolCallsExceeded),
                "STOP_REASON_MAX_INPUT_TOKENS_EXCEEDED" => Some(Self::MaxInputTokensExceeded),
                "STOP_REASON_MAX_OUTPUT_TOKENS_EXCEEDED" => Some(Self::MaxOutputTokensExceeded),
                "STOP_REASON_MAX_TOTAL_TOKENS_EXCEEDED" => Some(Self::MaxTotalTokensExceeded),
                "STOP_REASON_QUOTA_EXHAUSTED" => Some(Self::QuotaExhausted),
                _ => None,
            }
        }
    }
    impl TryFrom<i32> for StopReason {
        type Error = i32;
        fn try_from(value: i32) -> std::result::Result<Self, i32> {
            match value {
                0 => Ok(Self::Unspecified),
                1 => Ok(Self::MaxModelCallsExceeded),
                2 => Ok(Self::MaxToolCallsExceeded),
                3 => Ok(Self::MaxInputTokensExceeded),
                4 => Ok(Self::MaxOutputTokensExceeded),
                5 => Ok(Self::MaxTotalTokensExceeded),
                6 => Ok(Self::QuotaExhausted),
                other => Err(other),
            }
        }
    }
    impl crate::proto::serde_helpers::ProtoEnum for StopReason {
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
    impl serde::Serialize for StopReason {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
        }
    }
    impl<'de> serde::Deserialize<'de> for StopReason {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrajectoryUsageEntry {
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<UsageMetadata>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageUpdate {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub agents: Vec<TrajectoryUsageEntry>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total: Option<UsageMetadata>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCall {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "arguments_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub arguments_json: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<super::super::genai::Struct>,
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolResponse {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub id: String,
    #[serde(
        alias = "response_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub response_json: String,
    #[serde(
        alias = "supplemental_media",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub supplemental_media: Vec<Media>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response: Option<super::super::genai::Struct>,
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserQuestionsResponse {
    #[serde(
        alias = "trajectory_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub trajectory_id: String,
    #[serde(
        alias = "step_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_u32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub step_index: u32,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub result: Option<user_questions_response::Result>,
}
/// Nested message and enum types in `UserQuestionsResponse`.
pub mod user_questions_response {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct QuestionsResponse {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub answers: Vec<super::UserQuestionAnswer>,
    }
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Result {
        Cancelled(bool),
        Response(QuestionsResponse),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserQuestionAnswer {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub answer: Option<user_question_answer::Answer>,
}
/// Nested message and enum types in `UserQuestionAnswer`.
pub mod user_question_answer {
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Answer {
        Unanswered(bool),
        #[serde(alias = "multiple_choice_answer")]
        MultipleChoiceAnswer(super::MultipleChoiceAnswer),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MultipleChoiceAnswer {
    #[serde(
        alias = "selected_choice_indices",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub selected_choice_indices: Vec<i32>,
    #[serde(
        alias = "freeform_response",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub freeform_response: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Media {
    #[serde(alias = "mime_type", skip_serializing_if = "String::is_empty", default)]
    pub mime_type: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub data: Vec<u8>,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ModalityTokenCount {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<Modality>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub modality: i32,
    #[serde(
        alias = "token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub token_count: u64,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageMetadata {
    #[serde(
        alias = "prompt_token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub prompt_token_count: u64,
    #[serde(
        alias = "cached_content_token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub cached_content_token_count: u64,
    #[serde(
        alias = "candidates_token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub candidates_token_count: u64,
    #[serde(
        alias = "thoughts_token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub thoughts_token_count: u64,
    #[serde(
        alias = "total_token_count",
        with = "crate::proto::serde_helpers::u64_str",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub total_token_count: u64,
    #[serde(
        alias = "service_tier",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub service_tier: String,
    #[serde(
        alias = "prompt_tokens_details",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub prompt_tokens_details: Vec<ModalityTokenCount>,
    #[serde(
        alias = "cache_tokens_details",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub cache_tokens_details: Vec<ModalityTokenCount>,
    #[serde(
        alias = "candidates_tokens_details",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub candidates_tokens_details: Vec<ModalityTokenCount>,
    #[serde(
        alias = "tool_use_prompt_tokens_details",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub tool_use_prompt_tokens_details: Vec<ModalityTokenCount>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerConfig {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "enabled_tools",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub enabled_tools: Vec<String>,
    #[serde(
        alias = "disabled_tools",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub disabled_tools: Vec<String>,
    #[serde(
        alias = "auth_provider_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<mcp_server_config::AuthProviderType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub auth_provider_type: i32,
    #[serde(
        alias = "timeout_seconds",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub timeout_seconds: i32,
    #[serde(
        alias = "force_all_tools_eager",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub force_all_tools_eager: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub transport: Option<mcp_server_config::Transport>,
}
/// Nested message and enum types in `McpServerConfig`.
pub mod mcp_server_config {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum AuthProviderType {
        #[default]
        Unspecified = 0,
        GoogleCredentials = 1,
    }
    impl AuthProviderType {
        /// String value of the enum field names used in the Protobuf definition.
        ///
        /// The values are not transformed in any way and thus are considered stable
        /// (if the Protobuf definition does not change) and safe for programmatic use.
        #[must_use]
        pub fn as_str_name(&self) -> &'static str {
            match self {
                Self::Unspecified => "AUTH_PROVIDER_TYPE_UNSPECIFIED",
                Self::GoogleCredentials => "AUTH_PROVIDER_TYPE_GOOGLE_CREDENTIALS",
            }
        }
        /// Creates an enum from field names used in the Protobuf definition.
        #[must_use]
        pub fn from_str_name(value: &str) -> Option<Self> {
            match value {
                "AUTH_PROVIDER_TYPE_UNSPECIFIED" => Some(Self::Unspecified),
                "AUTH_PROVIDER_TYPE_GOOGLE_CREDENTIALS" => Some(Self::GoogleCredentials),
                _ => None,
            }
        }
    }
    impl TryFrom<i32> for AuthProviderType {
        type Error = i32;
        fn try_from(value: i32) -> std::result::Result<Self, i32> {
            match value {
                0 => Ok(Self::Unspecified),
                1 => Ok(Self::GoogleCredentials),
                other => Err(other),
            }
        }
    }
    impl crate::proto::serde_helpers::ProtoEnum for AuthProviderType {
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
    impl serde::Serialize for AuthProviderType {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
        }
    }
    impl<'de> serde::Deserialize<'de> for AuthProviderType {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
        }
    }
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Transport {
        Stdio(super::McpStdioTransport),
        Http(super::McpHttpTransport),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpStdioTransport {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub env: std::collections::HashMap<String, String>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpHttpTransport {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub headers: std::collections::HashMap<String, String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionMcpTool {
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(alias = "tool_name", skip_serializing_if = "String::is_empty", default)]
    pub tool_name: String,
    #[serde(
        alias = "arguments_json",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub arguments_json: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionCustomTool {
    #[serde(alias = "tool_call", skip_serializing_if = "Option::is_none", default)]
    pub tool_call: Option<ToolCall>,
    #[serde(
        alias = "tool_response",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub tool_response: Option<ToolResponse>,
}
